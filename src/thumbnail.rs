//! Rendered thumbnails for the Project window's tiles.
//!
//! Without them a folder browses as a wall of identical font glyphs, which
//! makes a 300-entry kit unusable. This module photographs a model, a
//! material or a prefab off-screen once and caches the result as a PNG under
//! the project's `.jackdaw/thumbnails/`, so the second visit to a folder
//! costs a file read rather than a scene load and a GPU pass. A scene is not
//! photographed here: its picture is the viewport capture the editor takes
//! when the scene is saved.
//!
//! The off-screen setup copies [`crate::material_preview`]: a camera with
//! `RenderTarget::Image`, its own [`RenderLayers`] so nothing leaks into a
//! viewport, and its own [`EnvironmentMapLight`] because lighting is
//! per-view rather than global. It differs in two ways: one target image is
//! reused for every subject (the pixels are read back and written to disk, so
//! nothing needs to stay resident), and the whole thing is driven by a
//! bounded work queue rather than by a selection.
//!
//! Three rules the queue exists to keep:
//!
//! - **Never stall the editor.** At most one subject is rendered at a time
//!   and at most `DISK_LOADS_PER_FRAME` cached PNGs are decoded per frame.
//! - **Never re-render what is already on disk.** The cache is keyed by path
//!   *and* source mtime, so an edited file regenerates and an untouched one
//!   never does, across editor restarts. Only the subject's own file is
//!   keyed, so a material whose textures are edited under it keeps its
//!   picture until the material file itself is written again.
//! - **Never retry a file that failed.** A subject that cannot load is marked
//!   failed for that mtime and skipped until the file changes.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bevy::{
    asset::{RenderAssetUsages, embedded_asset, load_embedded_asset},
    camera::{RenderTarget, visibility::RenderLayers},
    gltf::GltfAssetLabel,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    prelude::*,
    reflect::TypePath,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    ui::UiGlobalTransform,
    world_serialization::{WorldAsset, WorldAssetRoot},
};
use jackdaw_bsn::{BsnPatch, SceneBsnAst, apply_component_patch, patch_type_path};
use jackdaw_feathers::tokens;
use path_slash::PathExt as _;

use crate::asset_index::AssetIndex;
use crate::entity_ops::GltfSource;

/// Render layer the thumbnail stage owns exclusively. Layer 0 is the world,
/// layer 1 is the material preview, and per-viewport grids start at layer 3
/// (`crate::viewport::ViewportLayerCounter`); this one sits between them so
/// a subject being photographed never appears in a viewport, and a viewport's
/// grid never appears in a thumbnail.
pub(crate) const THUMBNAIL_LAYER: usize = 2;

/// Edge of the square render target, in pixels. The browser draws the result
/// at [`THUMBNAIL_DISPLAY_SIZE`]; rendering larger keeps it crisp on a
/// high-DPI display and costs about 8 KiB of PNG per subject on disk.
const THUMBNAIL_SIZE: u32 = 128;

/// Edge of the thumbnail as drawn in a browser tile. Close to the glyph it
/// replaces (`tokens::ICON_LG_PX` at 24 px plus its line box) so a folder of
/// mixed files keeps one grid rhythm. The browser sizes the glyph's slot to
/// match, so nothing reflows when a thumbnail arrives.
pub(crate) const THUMBNAIL_DISPLAY_SIZE: f32 = 40.0;

/// Cached PNGs decoded per frame. Decoding a 128x128 PNG takes well under a
/// millisecond, so this is generous; the point of the cap is that opening a
/// 364-entry folder cannot turn one frame into a 364-file read.
const DISK_LOADS_PER_FRAME: usize = 8;

/// Frames a single render job may spend in one state before it is written
/// off as failed. A subject whose asset load never resolves, or whose scene
/// never spawns a mesh, must not pin the queue forever.
const JOB_TIMEOUT_FRAMES: u32 = 600;

/// Frames to let the thumbnail camera draw the framed subject before the
/// read-back is queued. `Screenshot` swaps the target's output attachment
/// and captures the *next* render into it, so there has to be at least one
/// more render after the subject is in place.
const SETTLE_FRAMES: u32 = 8;

/// How far outside the browser's scroll viewport a tile still counts as
/// visible, in pixels. Roughly two extra rows above and below, so a slow
/// scroll meets thumbnails that are already there.
const VISIBLE_MARGIN_PX: f32 = 160.0;

/// How much bigger than the subject the framing is. At 1.0 the bounding
/// sphere exactly touches the frustum, which clips the corners of a boxy
/// model; the excess is the margin around the subject.
const FRAMING_MARGIN: f32 = 1.25;

/// Distance floor, so a degenerate or empty subject does not put the camera
/// inside itself.
const MIN_CAMERA_DISTANCE: f32 = 0.25;

/// The direction the camera looks from, in subject space. A three-quarter
/// view reads as a solid object where a face-on view reads as a silhouette.
const VIEW_DIRECTION: Vec3 = Vec3::new(1.0, 0.75, 1.0);

/// Radius of the sphere a material is photographed on.
const MATERIAL_SPHERE_RADIUS: f32 = 1.0;

/// How deep a prefab document is walked while its models are gathered.
const MAX_PREFAB_DEPTH: usize = 64;

pub(crate) fn plugin(app: &mut App) {
    // Registered here as well as in `viewport`/`material_preview` so this
    // module owns its own dependency rather than inheriting whichever
    // plugin happened to run first. Both resolve to the same asset path.
    embedded_asset!(
        app,
        "../assets/environment_maps/voortrekker_interior_1k_diffuse.ktx2"
    );
    embedded_asset!(
        app,
        "../assets/environment_maps/voortrekker_interior_1k_specular.ktx2"
    );
    app.init_resource::<Thumbnails>()
        .add_systems(OnEnter(crate::AppState::Editor), setup_thumbnail_stage)
        .add_systems(
            Update,
            (
                drive_thumbnail_queue,
                update_thumbnail_slots,
                capture_pending_scene,
            )
                .run_if(in_state(crate::AppState::Editor)),
        );
}

/// True when `path` names a glTF model the browser should photograph.
pub fn is_model_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("glb") || e.eq_ignore_ascii_case("gltf"))
}

/// What a tile's picture is made from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Subject {
    /// A glTF file, spawned and photographed.
    Model,
    /// A material file, photographed on a sphere.
    Material,
    /// A prefab document, whose models are spawned and photographed.
    Prefab,
    /// A scene document, whose picture is the viewport capture its last save
    /// wrote.
    Scene,
}

// -- Cache -------------------------------------------------------------------

/// What is known about one file's thumbnail.
#[derive(Clone, Debug)]
enum ThumbState {
    /// Photographed (or read back from disk) and ready to draw.
    Ready(Handle<Image>),
    /// The subject could not be loaded or photographed. Not retried until the
    /// file changes on disk.
    Failed,
}

/// A cache entry is only valid for the source mtime it was produced from.
#[derive(Clone, Debug)]
struct CacheEntry {
    mtime: SystemTime,
    state: ThumbState,
}

/// Mtime-keyed memo of thumbnail results plus the pending work queue.
///
/// Split out from the ECS resource so the interesting rules -- mtime
/// invalidation and "failed once, do not retry" -- are testable without a
/// GPU or an `App`.
#[derive(Default)]
struct ThumbnailCache {
    // Grows for the life of the process: entries are replaced on a stale
    // mtime but never evicted for a path that stops being referenced
    // (deleted file, browser navigated elsewhere for good). A project
    // with a very large, high-churn asset set could grow this
    // unboundedly over a long editor session. Not a problem this round
    // (one `Handle<Image>` + a mtime per entry), but a real cache would
    // want an eviction policy if that ever changes.
    entries: HashMap<PathBuf, CacheEntry>,
    queue: VecDeque<(PathBuf, Subject)>,
    queued: HashSet<PathBuf>,
}

impl ThumbnailCache {
    /// The ready thumbnail for `path`, if one was produced for the mtime
    /// `path` currently has. A stale or failed entry reads as `None`.
    fn ready(&self, path: &Path, mtime: SystemTime) -> Option<Handle<Image>> {
        match self.entries.get(path) {
            Some(entry) if entry.mtime == mtime => match &entry.state {
                ThumbState::Ready(handle) => Some(handle.clone()),
                ThumbState::Failed => None,
            },
            _ => None,
        }
    }

    /// Whether `path` is known to have failed at the mtime it currently
    /// has. `false` for "never tried" as well as for a stale failure (the
    /// file changed since), both of which still deserve a fresh attempt.
    fn is_failed(&self, path: &Path, mtime: SystemTime) -> bool {
        matches!(
            self.entries.get(path),
            Some(entry) if entry.mtime == mtime && matches!(entry.state, ThumbState::Failed)
        )
    }

    /// Ask for a thumbnail. A no-op when one is already known for this mtime
    /// (ready *or* failed) or when the path is already queued; a stale entry
    /// is dropped so the subject is photographed again.
    fn request(&mut self, path: &Path, mtime: SystemTime, subject: Subject) {
        match self.entries.get(path) {
            Some(entry) if entry.mtime == mtime => return,
            Some(_) => {
                self.entries.remove(path);
            }
            None => {}
        }
        if self.queued.insert(path.to_path_buf()) {
            self.queue.push_back((path.to_path_buf(), subject));
        }
    }

    fn is_queued(&self, path: &Path) -> bool {
        self.queued.contains(path)
    }

    fn take_next(&mut self) -> Option<(PathBuf, Subject)> {
        let next = self.queue.pop_front()?;
        self.queued.remove(&next.0);
        Some(next)
    }

    fn record(&mut self, path: &Path, mtime: SystemTime, state: ThumbState) {
        self.entries
            .insert(path.to_path_buf(), CacheEntry { mtime, state });
    }

    fn forget(&mut self, path: &Path) {
        self.entries.remove(path);
    }
}

/// The thumbnail cache, the pending queue, and the one in-flight render.
#[derive(Resource, Default)]
pub struct Thumbnails {
    cache: ThumbnailCache,
    job: Option<Job>,
    /// Where cached PNGs live. `None` until the editor knows its project,
    /// which also disables the whole feature -- there is nowhere to cache.
    cache_dir: Option<PathBuf>,
    /// The project's assets directory, which scene pictures and material
    /// lookups are both named relative to.
    assets_dir: Option<PathBuf>,
    /// Set by the read-back observer, consumed by [`drive_thumbnail_queue`].
    /// The observer runs outside this system's ordering, so its result is
    /// parked here rather than applied from inside it.
    capture_result: Option<bool>,
    /// Set by the command that spawns a prefab's models, for the same reason.
    subject_result: Option<bool>,
    /// Scenes a save has asked for a picture of, taken on a later frame so
    /// the viewport has drawn what was saved.
    pending_scenes: Vec<PathBuf>,
    /// Paths a failure has already been reported for, so a folder of broken
    /// files logs each of them once rather than once per attempt.
    warned: HashSet<PathBuf>,
    /// Thumbnails photographed since the editor started. Reported in the log
    /// so a first pass over a large kit can be measured.
    rendered: usize,
}

impl Thumbnails {
    /// The ready thumbnail for `path`, or `None` while it is pending, has
    /// failed, or the file is unreadable.
    pub fn ready(&self, path: &Path) -> Option<Handle<Image>> {
        self.cache.ready(path, source_mtime(path)?)
    }

    /// Whether `path` has a cached failure at its current mtime.
    ///
    /// One `fs::metadata` call, same as [`Self::ready`]. Callers driving
    /// a per-frame slot (`update_thumbnail_slots`) should call this only
    /// until it returns `true` once, then stop asking entirely: without
    /// that latch, a permanently-broken file on screen paid this
    /// syscall (and `ready`'s) every single frame for as long as it
    /// stayed visible.
    pub fn is_failed(&self, path: &Path) -> bool {
        match source_mtime(path) {
            Some(mtime) => self.cache.is_failed(path, mtime),
            // Cannot even stat it: treat like a permanent failure so the
            // caller latches and stops asking, same as a render failure.
            None => true,
        }
    }

    /// Queue `path` for a thumbnail of `subject` if one is not already known
    /// or pending.
    pub fn request(&mut self, path: &Path, subject: Subject) {
        if self.cache.is_queued(path) {
            return;
        }
        let Some(mtime) = source_mtime(path) else {
            return;
        };
        self.cache.request(path, mtime, subject);
    }

    /// Drop what is known about `path`, so the next request looks again.
    pub fn forget(&mut self, path: &Path) {
        self.cache.forget(path);
        self.warned.remove(path);
    }

    /// Where the picture of the scene at `path` is kept, or `None` when the
    /// editor has no project to keep it under.
    pub fn scene_file(&self, path: &Path) -> Option<PathBuf> {
        Some(scene_thumbnail_file(
            self.cache_dir.as_deref()?,
            self.assets_dir.as_deref(),
            path,
        ))
    }

    fn fail(&mut self, path: &Path, mtime: SystemTime, reason: &str) {
        if self.warned.insert(path.to_path_buf()) {
            warn!("thumbnail: {} {reason}", path.display());
        }
        self.cache.record(path, mtime, ThumbState::Failed);
    }
}

/// The file's modification time, or `None` when it cannot be read -- an
/// unreadable file is not worth queueing.
fn source_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// File name of the cached PNG for `path` at `mtime`.
///
/// The mtime is part of the *name*, not just of the in-memory key, so a
/// stale PNG can never be mistaken for a fresh one across editor restarts;
/// the superseded file is orphaned. The path is folded with FNV-1a
/// rather than [`std::hash::DefaultHasher`] because a disk cache outlives
/// the process that wrote it and `DefaultHasher` is explicitly not stable
/// across releases.
fn cache_file_name(path: &Path, mtime: SystemTime) -> String {
    let stamp = mtime
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!(
        "{:016x}-{:x}.png",
        fnv1a64(path.to_string_lossy().as_bytes()),
        stamp
    )
}

/// Where a scene's picture is kept: named after the scene's own path under
/// the project's assets, so a save can write it without having to know what
/// the file's modification time will end up being.
fn scene_thumbnail_file(cache_dir: &Path, assets_dir: Option<&Path>, scene: &Path) -> PathBuf {
    let relative = assets_dir.and_then(|root| scene.strip_prefix(root).ok());
    match relative {
        Some(relative) => {
            let mut name = relative.as_os_str().to_string_lossy().into_owned();
            name.push_str(".png");
            cache_dir.join("scenes").join(name)
        }
        None => cache_dir.join("scenes").join(format!(
            "{:016x}.png",
            fnv1a64(scene.to_string_lossy().as_bytes())
        )),
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

// -- Framing -----------------------------------------------------------------

/// How far the camera has to sit from the centre of a bounding sphere of
/// `radius` for the whole sphere to fit inside a `fov_y` frustum.
///
/// The target is square, so the vertical field of view is also the
/// horizontal one and a single distance fits both axes.
fn fit_distance(radius: f32, fov_y: f32, margin: f32) -> f32 {
    let half_fov = (fov_y * 0.5).clamp(1e-3, core::f32::consts::FRAC_PI_2 - 1e-3);
    (radius * margin / half_fov.sin()).max(MIN_CAMERA_DISTANCE)
}

/// Where to put the camera, and what to aim it at, to frame the
/// axis-aligned box `min..max`. Returns `(position, target)`.
fn frame_bounds(min: Vec3, max: Vec3, fov_y: f32) -> (Vec3, Vec3) {
    let center = (min + max) * 0.5;
    let radius = (max - min).length() * 0.5;
    let distance = fit_distance(radius, fov_y, FRAMING_MARGIN);
    (center + VIEW_DIRECTION.normalize() * distance, center)
}

// -- Stage -------------------------------------------------------------------

#[derive(Component)]
struct ThumbnailCamera;

/// Root of the subject currently being photographed. Despawned when the job
/// ends, whichever way it ends.
#[derive(Component)]
struct ThumbnailSubject;

/// Handle of the shared render target, kept in its own resource so the
/// read-back does not have to go through the camera.
#[derive(Resource)]
struct ThumbnailTarget(Handle<Image>);

/// The image the thumbnail stage renders into, in a format `write_png` reads
/// back.
pub(crate) fn thumbnail_target_image() -> Image {
    Image::new_target_texture(
        THUMBNAIL_SIZE,
        THUMBNAIL_SIZE,
        crate::viewport::EDITOR_VIEW_FORMAT,
        None,
    )
}

fn setup_thumbnail_stage(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut thumbnails: ResMut<Thumbnails>,
    project: Option<Res<crate::project::ProjectRoot>>,
    assets: Res<AssetServer>,
) {
    let layer = RenderLayers::layer(THUMBNAIL_LAYER);

    let target = images.add(thumbnail_target_image());
    commands.insert_resource(ThumbnailTarget(target.clone()));

    thumbnails.cache_dir = project
        .as_deref()
        .map(|project| project.jackdaw_dir().join("thumbnails"));
    thumbnails.assets_dir = project
        .as_deref()
        .map(crate::project::ProjectRoot::assets_dir);
    if let Some(dir) = &thumbnails.cache_dir {
        let _ = std::fs::create_dir_all(dir);
    }

    commands.spawn((
        ThumbnailCamera,
        crate::EditorEntity,
        Camera3d::default(),
        Camera {
            order: -1,
            // Solid, not transparent. `write_png` drops alpha (under HDR it
            // carries brightness, not opacity), so a transparent clear would
            // encode as black. Clearing to the panel colour instead makes the
            // tile read as a cutout against the browser background.
            clear_color: ClearColorConfig::Custom(tokens::PANEL_BG),
            ..default()
        },
        // Per-view, for the same reason the material preview carries its own:
        // there is no global "current environment map" resource.
        EnvironmentMapLight {
            diffuse_map: load_embedded_asset!(
                &*assets,
                "../assets/environment_maps/voortrekker_interior_1k_diffuse.ktx2"
            ),
            specular_map: load_embedded_asset!(
                &*assets,
                "../assets/environment_maps/voortrekker_interior_1k_specular.ktx2"
            ),
            intensity: 1500.0,
            ..default()
        },
        RenderTarget::Image(target.into()),
        Transform::from_translation(Vec3::splat(3.0)).looking_at(Vec3::ZERO, Vec3::Y),
        layer.clone(),
    ));

    // A key light on top of the environment map: image-based lighting alone
    // flattens the untextured, flat-shaded models these kits are made of.
    commands.spawn((
        crate::EditorEntity,
        DirectionalLight {
            illuminance: 4000.0,
            shadow_maps_enabled: false,
            contact_shadows_enabled: false,
            ..default()
        },
        Transform::from_translation(Vec3::new(4.0, 6.0, 4.0)).looking_at(Vec3::ZERO, Vec3::Y),
        layer,
    ));
}

// -- Work queue --------------------------------------------------------------

/// One file's journey from a queued path to a PNG on disk.
struct Job {
    path: PathBuf,
    mtime: SystemTime,
    root: Option<Entity>,
    frames: u32,
    stage: Stage,
}

/// How far along the stage a job has got.
enum Stage {
    /// Waiting on a glTF asset load.
    Loading(Handle<WorldAsset>),
    /// A prefab's models are being spawned by a queued command.
    Building,
    /// A material's sphere is up; waiting on the textures it draws with.
    Dressing(Handle<StandardMaterial>),
    /// The subject is spawned; waiting for its meshes to exist so the bounds
    /// can be measured.
    Spawned,
    /// Framed and rendering; counting down to the read-back.
    Settling,
    /// Read-back queued; waiting for the observer to report.
    Capturing,
}

/// What one step of the state machine decided to do.
enum Step {
    /// Stay where you are; try again next frame.
    Wait,
    /// Move to this stage.
    Advance(Stage),
    /// Move to this stage, under this newly spawned subject root.
    Rooted(Entity, Stage),
    /// The job is over. Despawn the root if there is one and record `state`.
    Finish(ThumbState),
}

/// Advance the one in-flight render job, or start a new one if the queue has
/// work and nothing is in flight. Cached PNGs are picked off the queue first
/// and never reach the GPU at all.
#[expect(
    clippy::too_many_arguments,
    reason = "one system owns the whole job state machine; splitting it would \
              mean sharing the in-flight job across systems that must not interleave"
)]
fn drive_thumbnail_queue(
    mut commands: Commands,
    mut thumbnails: ResMut<Thumbnails>,
    mut images: ResMut<Assets<Image>>,
    assets: Res<AssetServer>,
    meshes: Res<Assets<Mesh>>,
    materials: Res<Assets<StandardMaterial>>,
    shapes: Option<Res<crate::material_preview::PreviewShapeMeshes>>,
    index: Option<Res<AssetIndex>>,
    children_query: Query<&Children>,
    mesh_query: Query<(&Mesh3d, &GlobalTransform)>,
    model_query: Query<&WorldAssetRoot>,
    view_dependent: Query<(), With<crate::ViewDependentBounds>>,
    mut camera_query: Query<(&mut Transform, &Projection), With<ThumbnailCamera>>,
    target: Option<Res<ThumbnailTarget>>,
) {
    let Some(target) = target else {
        return;
    };
    if thumbnails.cache_dir.is_none() {
        return;
    }

    // Taking the job out of the resource for the duration of the step is
    // what lets the step borrow the resource mutably (to record a result)
    // without fighting the borrow on the job itself.
    if let Some(mut job) = thumbnails.job.take() {
        job.frames += 1;
        let step = if job.frames > JOB_TIMEOUT_FRAMES {
            thumbnails.fail(&job.path, job.mtime, "timed out");
            Step::Finish(ThumbState::Failed)
        } else {
            step_job(
                &mut commands,
                &mut thumbnails,
                &mut images,
                &assets,
                &meshes,
                &materials,
                &children_query,
                &mesh_query,
                &model_query,
                &view_dependent,
                &mut camera_query,
                &target.0,
                &job,
            )
        };
        match step {
            Step::Wait => thumbnails.job = Some(job),
            Step::Advance(stage) => {
                job.stage = stage;
                job.frames = 0;
                thumbnails.job = Some(job);
            }
            Step::Rooted(root, stage) => {
                job.root = Some(root);
                job.stage = stage;
                job.frames = 0;
                thumbnails.job = Some(job);
            }
            Step::Finish(state) => {
                if let Some(root) = job.root
                    && let Ok(mut entity) = commands.get_entity(root)
                {
                    entity.try_despawn();
                }
                match state {
                    ThumbState::Ready(_) => thumbnails.cache.record(&job.path, job.mtime, state),
                    ThumbState::Failed => {
                        thumbnails.fail(&job.path, job.mtime, "could not be photographed");
                    }
                }
            }
        }
        return;
    }

    // Nothing in flight: drain cached entries, and start a render on the
    // first path that has no PNG on disk.
    for _ in 0..DISK_LOADS_PER_FRAME {
        let Some((path, subject)) = thumbnails.cache.take_next() else {
            return;
        };
        let Some(mtime) = source_mtime(&path) else {
            continue;
        };
        if subject == Subject::Scene {
            match thumbnails
                .scene_file(&path)
                .and_then(|file| load_file(&file, &mut images))
            {
                Some(handle) => thumbnails
                    .cache
                    .record(&path, mtime, ThumbState::Ready(handle)),
                None => thumbnails.cache.record(&path, mtime, ThumbState::Failed),
            }
            continue;
        }
        if let Some(handle) = load_cached(&thumbnails, &path, mtime, &mut images) {
            thumbnails
                .cache
                .record(&path, mtime, ThumbState::Ready(handle));
            continue;
        }
        if let Some(job) = start_job(
            &mut commands,
            &mut thumbnails,
            &assets,
            shapes.as_deref(),
            index.as_deref(),
            &path,
            mtime,
            subject,
        ) {
            thumbnails.job = Some(job);
            return;
        }
    }
}

/// Put the subject for `path` on the stage, or report that it has none.
#[expect(
    clippy::too_many_arguments,
    reason = "each kind of subject is built out of a different one of these"
)]
fn start_job(
    commands: &mut Commands,
    thumbnails: &mut Thumbnails,
    assets: &AssetServer,
    shapes: Option<&crate::material_preview::PreviewShapeMeshes>,
    index: Option<&AssetIndex>,
    path: &Path,
    mtime: SystemTime,
    subject: Subject,
) -> Option<Job> {
    let job = |root, stage| Job {
        path: path.to_path_buf(),
        mtime,
        root,
        frames: 0,
        stage,
    };
    match subject {
        Subject::Scene => None,
        Subject::Model => {
            let scene = assets.load(GltfAssetLabel::Scene(0).from_asset(to_asset_path(path)));
            Some(job(None, Stage::Loading(scene)))
        }
        Subject::Material => {
            let shapes = shapes?;
            let Some(material) = material_handle(thumbnails, index, path) else {
                thumbnails.fail(path, mtime, "holds no material the editor has loaded");
                return None;
            };
            let root = commands
                .spawn((
                    ThumbnailSubject,
                    crate::EditorEntity,
                    Mesh3d(shapes.sphere.clone()),
                    MeshMaterial3d(material.clone()),
                    Transform::IDENTITY,
                    Visibility::Visible,
                    RenderLayers::layer(THUMBNAIL_LAYER),
                ))
                .id();
            Some(job(Some(root), Stage::Dressing(material)))
        }
        Subject::Prefab => {
            let root = commands
                .spawn((
                    ThumbnailSubject,
                    crate::EditorEntity,
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    RenderLayers::layer(THUMBNAIL_LAYER),
                ))
                .id();
            let document = path.to_path_buf();
            thumbnails.subject_result = None;
            commands.queue(move |world: &mut World| {
                let built = build_prefab_subject(world, root, &document);
                if let Some(mut thumbs) = world.get_resource_mut::<Thumbnails>() {
                    thumbs.subject_result = Some(built);
                }
            });
            Some(job(Some(root), Stage::Building))
        }
    }
}

/// The material the file at `path` was loaded into, as the asset index knows
/// it.
fn material_handle(
    thumbnails: &Thumbnails,
    index: Option<&AssetIndex>,
    path: &Path,
) -> Option<Handle<StandardMaterial>> {
    let index = index?;
    let relative = thumbnails
        .assets_dir
        .as_deref()
        .and_then(|root| path.strip_prefix(root).ok())
        .unwrap_or(path);
    let handle = index.get(relative)?.value.handle()?;
    (handle.type_id() == std::any::TypeId::of::<StandardMaterial>())
        .then(|| handle.clone().typed::<StandardMaterial>())
}

#[expect(
    clippy::too_many_arguments,
    reason = "state-machine step; every argument is one stage's dependency"
)]
fn step_job(
    commands: &mut Commands,
    thumbnails: &mut Thumbnails,
    images: &mut Assets<Image>,
    assets: &AssetServer,
    meshes: &Assets<Mesh>,
    materials: &Assets<StandardMaterial>,
    children_query: &Query<&Children>,
    mesh_query: &Query<(&Mesh3d, &GlobalTransform)>,
    model_query: &Query<&WorldAssetRoot>,
    view_dependent: &Query<(), With<crate::ViewDependentBounds>>,
    camera_query: &mut Query<(&mut Transform, &Projection), With<ThumbnailCamera>>,
    target: &Handle<Image>,
    job: &Job,
) -> Step {
    match &job.stage {
        Stage::Loading(scene) => {
            let state = assets.load_state(scene.id());
            if state.is_failed() {
                return Step::Finish(ThumbState::Failed);
            }
            if !state.is_loaded() {
                return Step::Wait;
            }
            // Spawned hidden: bevy resolves `RenderLayers` per entity and
            // does not inherit it, so the glTF's own entities land on layer
            // 0 -- the main viewport -- until they are tagged a frame later.
            // `Visibility` *is* inherited, so hiding the root hides them all
            // until the layer is right.
            let root = commands
                .spawn((
                    ThumbnailSubject,
                    crate::EditorEntity,
                    WorldAssetRoot(scene.clone()),
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    RenderLayers::layer(THUMBNAIL_LAYER),
                ))
                .id();
            Step::Rooted(root, Stage::Spawned)
        }
        Stage::Building => match thumbnails.subject_result.take() {
            None => Step::Wait,
            Some(false) => Step::Finish(ThumbState::Failed),
            Some(true) => Step::Advance(Stage::Spawned),
        },
        Stage::Dressing(material) => {
            if !material_is_dressed(assets, materials, material) {
                return Step::Wait;
            }
            let extent = Vec3::splat(MATERIAL_SPHERE_RADIUS);
            let (position, look_at) = frame_bounds(-extent, extent, camera_fov(camera_query));
            aim_camera(camera_query, position, look_at);
            Step::Advance(Stage::Settling)
        }
        Stage::Spawned => {
            let Some(root) = job.root else {
                return Step::Finish(ThumbState::Failed);
            };
            let mut vertices = Vec::new();
            crate::viewport_overlays::collect_descendant_mesh_world_vertices(
                root,
                children_query,
                mesh_query,
                view_dependent,
                meshes,
                &mut vertices,
            );
            if vertices.is_empty() {
                if every_model_failed(assets, root, children_query, model_query) {
                    return Step::Finish(ThumbState::Failed);
                }
                return Step::Wait; // scene not spawned, or meshes not loaded
            }

            let mut min = Vec3::splat(f32::INFINITY);
            let mut max = Vec3::splat(f32::NEG_INFINITY);
            for vertex in &vertices {
                min = min.min(*vertex);
                max = max.max(*vertex);
            }

            let (position, look_at) = frame_bounds(min, max, camera_fov(camera_query));
            aim_camera(camera_query, position, look_at);

            apply_layer_recursive(commands, root, children_query);
            commands.entity(root).insert(Visibility::Visible);

            Step::Advance(Stage::Settling)
        }
        Stage::Settling => {
            if job.frames < SETTLE_FRAMES {
                return Step::Wait;
            }
            let Some(file) = thumbnails
                .cache_dir
                .as_ref()
                .map(|dir| dir.join(cache_file_name(&job.path, job.mtime)))
            else {
                return Step::Finish(ThumbState::Failed);
            };
            thumbnails.capture_result = None;
            commands.spawn(Screenshot::image(target.clone())).observe(
                move |capture: On<ScreenshotCaptured>, mut thumbs: ResMut<Thumbnails>| {
                    thumbs.capture_result =
                        Some(crate::screenshot::write_png(&capture.image, &file));
                },
            );
            Step::Advance(Stage::Capturing)
        }
        Stage::Capturing => {
            let Some(wrote) = thumbnails.capture_result.take() else {
                return Step::Wait;
            };
            if !wrote {
                return Step::Finish(ThumbState::Failed);
            }
            // Read the thumbnail back through the same path a cache hit
            // takes, so a freshly rendered tile and a restored one are the
            // same image asset in the same format -- and so a write that
            // cannot be read back is caught here rather than next session.
            match load_cached(thumbnails, &job.path, job.mtime, images) {
                Some(handle) => {
                    thumbnails.rendered += 1;
                    debug!(
                        "thumbnail: rendered {} ({} this session)",
                        job.path.display(),
                        thumbnails.rendered
                    );
                    Step::Finish(ThumbState::Ready(handle))
                }
                None => Step::Finish(ThumbState::Failed),
            }
        }
    }
}

/// Whether the subject names models and every one of them failed to load, so a
/// prefab naming a file that has gone is written off rather than waiting out
/// the job timeout. A subject naming no model at all is not a failure here:
/// its meshes may still be coming.
fn every_model_failed(
    assets: &AssetServer,
    root: Entity,
    children_query: &Query<&Children>,
    model_query: &Query<&WorldAssetRoot>,
) -> bool {
    let mut named = 0usize;
    let mut failed = 0usize;
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if let Ok(model) = model_query.get(entity) {
            named += 1;
            if assets.load_state(model.0.id()).is_failed() {
                failed += 1;
            }
        }
        if let Ok(children) = children_query.get(entity) {
            stack.extend(children.iter());
        }
    }
    named > 0 && named == failed
}

/// Whether every texture the material draws with has settled, so the sphere
/// is not photographed while it is still untextured.
fn material_is_dressed(
    assets: &AssetServer,
    materials: &Assets<StandardMaterial>,
    handle: &Handle<StandardMaterial>,
) -> bool {
    let Some(material) = materials.get(handle) else {
        return false;
    };
    [
        material.base_color_texture.as_ref(),
        material.metallic_roughness_texture.as_ref(),
        material.normal_map_texture.as_ref(),
        material.occlusion_texture.as_ref(),
        material.emissive_texture.as_ref(),
    ]
    .into_iter()
    .flatten()
    .all(|texture| {
        let state = assets.load_state(texture.id());
        state.is_loaded() || state.is_failed()
    })
}

fn camera_fov(camera_query: &Query<(&mut Transform, &Projection), With<ThumbnailCamera>>) -> f32 {
    camera_query
        .iter()
        .next()
        .and_then(|(_, projection)| match projection {
            Projection::Perspective(perspective) => Some(perspective.fov),
            _ => None,
        })
        .unwrap_or(core::f32::consts::FRAC_PI_4)
}

fn aim_camera(
    camera_query: &mut Query<(&mut Transform, &Projection), With<ThumbnailCamera>>,
    position: Vec3,
    look_at: Vec3,
) {
    if let Some((mut transform, _)) = camera_query.iter_mut().next() {
        *transform = Transform::from_translation(position).looking_at(look_at, Vec3::Y);
    }
}

/// Spawn the models a prefab document names, under `root`. Returns whether it
/// named any: a prefab that names none keeps its icon rather than being
/// photographed as an empty frame.
fn build_prefab_subject(world: &mut World, root: Entity, path: &Path) -> bool {
    let assets_root = crate::prefab::save_load::source_root_of(world, path);
    let Ok(document) = crate::prefab::save_load::read_prefab_ast(path, &assets_root) else {
        return false;
    };
    let mut walk = PrefabWalk {
        models: 0,
        follow: true,
    };
    for node in document.roots.clone() {
        spawn_prefab_node(world, &document, node, root, 0, &mut walk);
    }
    walk.models > 0
}

/// What one walk of a prefab document has found, and whether a node naming
/// another prefab is still read.
struct PrefabWalk {
    models: usize,
    follow: bool,
}

fn spawn_prefab_node(
    world: &mut World,
    document: &SceneBsnAst,
    node: Entity,
    parent: Entity,
    depth: usize,
    walk: &mut PrefabWalk,
) {
    if depth >= MAX_PREFAB_DEPTH {
        return;
    }
    let patches: Vec<BsnPatch> = document
        .get_patches(node)
        .map(|patches| {
            patches
                .0
                .iter()
                .filter_map(|patch| document.get_patch(*patch).cloned())
                .collect()
        })
        .unwrap_or_default();

    let entity = world
        .spawn((
            ChildOf(parent),
            Transform::IDENTITY,
            Visibility::Inherited,
            RenderLayers::layer(THUMBNAIL_LAYER),
        ))
        .id();

    let mut children = Vec::new();
    let mut inherited = None;
    for patch in &patches {
        if let BsnPatch::Children(list) = patch {
            children.extend(list.iter().copied());
            continue;
        }
        let Some(type_path) = patch_type_path(patch) else {
            continue;
        };
        if type_path == jackdaw_prefab::ISA_TYPE {
            inherited = jackdaw_prefab::read_isa_source(document, node);
            continue;
        }
        if type_path == GltfSource::type_path() {
            walk.models += 1;
        } else if type_path != Transform::type_path() {
            continue;
        }
        apply_component_patch(world, entity, patch);
    }

    if let Some(inherited) = inherited.filter(|_| walk.follow) {
        spawn_inherited_prefab(world, &inherited, entity, depth + 1, walk);
    }

    for child in children {
        spawn_prefab_node(world, document, child, entity, depth + 1, walk);
    }
}

/// Spawn the models of the prefab a node inherits from, so a prefab built out
/// of other prefabs is photographed with what they bring.
///
/// One level deep: the document that names this one is the picture's subject,
/// and a chain of them is a scene rather than a tile.
fn spawn_inherited_prefab(
    world: &mut World,
    source: &Path,
    parent: Entity,
    depth: usize,
    walk: &mut PrefabWalk,
) {
    walk.follow = false;
    let mut spawned = false;
    if world.contains_resource::<crate::prefab::PrefabAstCache>() {
        world.resource_scope(|world, cache: Mut<crate::prefab::PrefabAstCache>| {
            if let Some(document) = cache.get(source) {
                for node in document.roots.clone() {
                    spawn_prefab_node(world, document, node, parent, depth, walk);
                }
                spawned = true;
            }
        });
    }
    let assets_root = crate::prefab::save_load::source_root_of(world, source);
    if !spawned
        && let Ok(document) = crate::prefab::save_load::read_prefab_ast(source, &assets_root)
    {
        for node in document.roots.clone() {
            spawn_prefab_node(world, &document, node, parent, depth, walk);
        }
    }
    walk.follow = true;
}

/// Read the cached PNG for `path` at `mtime` back into an image asset, if
/// one was written by this or an earlier session. This is the whole point of
/// the disk cache: reopening a 364-model folder costs 364 file reads rather
/// than 364 scene loads and GPU passes.
fn load_cached(
    thumbnails: &Thumbnails,
    path: &Path,
    mtime: SystemTime,
    images: &mut Assets<Image>,
) -> Option<Handle<Image>> {
    let file = thumbnails
        .cache_dir
        .as_ref()?
        .join(cache_file_name(path, mtime));
    load_file(&file, images)
}

fn load_file(file: &Path, images: &mut Assets<Image>) -> Option<Handle<Image>> {
    let bytes = std::fs::read(file).ok()?;
    decode_thumbnail(&bytes).map(|image| images.add(image))
}

/// Decode thumbnail PNG bytes into an image asset.
///
/// `RenderAssetUsages::RENDER_WORLD` only: a browser tile never reads these
/// pixels back, and keeping the CPU copy of a 364-model kit resident would
/// cost about 24 MiB for nothing.
fn decode_thumbnail(bytes: &[u8]) -> Option<Image> {
    Image::from_buffer(
        bytes,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::linear(),
        RenderAssetUsages::RENDER_WORLD,
    )
    .ok()
}

/// Map an absolute file path to the asset path the `AssetServer` wants.
fn to_asset_path(path: &Path) -> String {
    crate::entity_ops::to_asset_path(&path.to_slash_lossy())
}

/// Put every entity in the spawned hierarchy on the thumbnail layer.
/// Bevy resolves `RenderLayers` per entity and does not inherit it down the
/// hierarchy, so tagging the root alone would leave the meshes on layer 0.
fn apply_layer_recursive(commands: &mut Commands, entity: Entity, children: &Query<&Children>) {
    commands
        .entity(entity)
        .insert(RenderLayers::layer(THUMBNAIL_LAYER));
    if let Ok(kids) = children.get(entity) {
        for child in kids.iter() {
            apply_layer_recursive(commands, child, children);
        }
    }
}

// -- Scene pictures ----------------------------------------------------------

/// Ask for a picture of the scene just saved to `path`.
///
/// The picture is taken on a later frame, from the viewport, so what it shows
/// is the scene that was saved rather than whatever was on screen midway
/// through a round of saves.
pub fn capture_scene_thumbnail(world: &mut World, path: &Path) {
    let Some(mut thumbnails) = world.get_resource_mut::<Thumbnails>() else {
        return;
    };
    if !thumbnails.pending_scenes.iter().any(|asked| asked == path) {
        thumbnails.pending_scenes.push(path.to_path_buf());
    }
}

/// The one scene a round of saves can picture: the one the viewport is
/// showing, because a frame can only be photographed once.
fn picture_target(pending: &[PathBuf], shown: Option<&Path>) -> Option<PathBuf> {
    let shown = shown?;
    pending.iter().find(|path| path.as_path() == shown).cloned()
}

/// Photograph what the viewport is showing as the picture of the scene it is
/// showing, for a save that asked for one.
///
/// A no-op when no viewport is open, and when the editor has no project to
/// keep the picture under; nothing is ever written beside the scene itself.
fn capture_pending_scene(world: &mut World) {
    let pending = match world.get_resource_mut::<Thumbnails>() {
        Some(mut thumbnails) if !thumbnails.pending_scenes.is_empty() => {
            std::mem::take(&mut thumbnails.pending_scenes)
        }
        _ => return,
    };
    let shown = world
        .get_resource::<crate::scene_io::SceneFilePath>()
        .and_then(|scene| scene.path.clone())
        .map(PathBuf::from);
    let Some(scene) = picture_target(&pending, shown.as_deref()) else {
        return;
    };
    let Some(file) = world
        .get_resource::<Thumbnails>()
        .and_then(|thumbnails| thumbnails.scene_file(&scene))
    else {
        return;
    };
    let Some(camera) = crate::screenshot::viewport_camera(world) else {
        debug!("thumbnail: no viewport is open to picture the scene with");
        return;
    };
    let Some(target) = world
        .get::<RenderTarget>(camera)
        .and_then(RenderTarget::as_image)
        .cloned()
    else {
        return;
    };
    world.spawn(Screenshot::image(target)).observe(
        move |capture: On<ScreenshotCaptured>,
              mut thumbs: ResMut<Thumbnails>,
              project: Option<ResMut<crate::project_window::ProjectWindowState>>| {
            if !crate::screenshot::write_scaled_png(&capture.image, &file, THUMBNAIL_SIZE) {
                return;
            }
            thumbs.forget(&scene);
            if let Some(mut project) = project {
                project.needs_refresh = true;
            }
        },
    );
}

// -- Browser tiles -----------------------------------------------------------

/// The square of a tile that holds either the fallback glyph or the rendered
/// thumbnail. Placed by the Project window; driven from here so the browser
/// does not have to know about render state.
#[derive(Component)]
pub struct ThumbnailSlot {
    pub path: PathBuf,
    pub subject: Subject,
    /// Set once the image is installed, which takes the slot out of the
    /// per-frame work entirely: scrolling back over a generated thumbnail
    /// costs nothing.
    applied: bool,
}

impl ThumbnailSlot {
    pub fn new(path: PathBuf, subject: Subject) -> Self {
        Self {
            path,
            subject,
            applied: false,
        }
    }
}

/// Request thumbnails for the tiles the user can actually see, and install
/// the ones that are ready.
///
/// Visibility is decided against the browser's scroll viewport rather than
/// against the directory listing, so opening a 364-entry folder queues the
/// tiles on screen, not all 364. The on-screen test is pure arithmetic and
/// runs first, so an off-screen tile costs no filesystem call at all.
fn update_thumbnail_slots(
    mut commands: Commands,
    mut thumbnails: ResMut<Thumbnails>,
    mut slots: Query<(
        Entity,
        &mut ThumbnailSlot,
        &UiGlobalTransform,
        &ComputedNode,
    )>,
    content: Query<
        (&UiGlobalTransform, &ComputedNode),
        With<crate::project_window::ProjectFileGrid>,
    >,
    children: Query<&Children>,
) {
    let visible_band = content.iter().next().map(|(transform, node)| {
        let half = node.size().y * 0.5 + VISIBLE_MARGIN_PX;
        let center = transform.translation.y;
        (center - half, center + half)
    });

    for (entity, mut slot, transform, node) in &mut slots {
        if slot.applied {
            continue;
        }
        let on_screen = visible_band.is_none_or(|(top, bottom)| {
            let half = node.size().y * 0.5;
            let center = transform.translation.y;
            center + half >= top && center - half <= bottom
        });
        if !on_screen {
            continue;
        }

        let Some(handle) = thumbnails.ready(&slot.path) else {
            if thumbnails.is_failed(&slot.path) {
                // Latch: `applied` takes the slot out of this loop
                // entirely (see its doc), same as a successful install.
                // Without it, a file that fails to render paid an
                // fs::metadata call here, and another inside `ready`
                // above, every single frame for as long as it stayed
                // visible.
                slot.applied = true;
                continue;
            }
            let subject = slot.subject;
            thumbnails.request(&slot.path, subject);
            continue;
        };
        if let Ok(kids) = children.get(entity) {
            for child in kids.iter() {
                commands.entity(child).try_despawn();
            }
        }
        commands.spawn((
            ImageNode::new(handle),
            Node {
                width: Val::Px(THUMBNAIL_DISPLAY_SIZE),
                height: Val::Px(THUMBNAIL_DISPLAY_SIZE),
                ..default()
            },
            // The tile above owns the click, the drag and the context menu;
            // the picture must not swallow any of them.
            Pickable::IGNORE,
            ChildOf(entity),
        ));
        slot.applied = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::system::RunSystemOnce as _;
    use core::time::Duration;

    // The render half of this module needs a live wgpu backend, which
    // `tests/util/mod.rs`'s `headless_app()` deliberately does not have
    // (`WgpuSettings { backends: None }` renders nothing). Everything below
    // is the pure half: the cache rules, the framing arithmetic and the slot
    // bookkeeping. The rendering is covered by the screenshot evidence run
    // instead.

    fn epoch(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn model_paths_are_gltf_and_glb_only() {
        assert!(is_model_path(Path::new("/a/tree.glb")));
        assert!(is_model_path(Path::new("/a/tree.GLTF")));
        assert!(!is_model_path(Path::new("/a/tree.png")));
        assert!(!is_model_path(Path::new("/a/tree")));
    }

    #[test]
    fn cache_key_changes_with_path_and_with_mtime() {
        let a = Path::new("/kit/tree.glb");
        let b = Path::new("/kit/rock.glb");
        assert_ne!(cache_file_name(a, epoch(1)), cache_file_name(b, epoch(1)));
        assert_ne!(cache_file_name(a, epoch(1)), cache_file_name(a, epoch(2)));
        assert_eq!(cache_file_name(a, epoch(1)), cache_file_name(a, epoch(1)));
        assert!(cache_file_name(a, epoch(1)).ends_with(".png"));
    }

    #[test]
    fn a_ready_thumbnail_is_not_requeued() {
        let mut cache = ThumbnailCache::default();
        let path = Path::new("/kit/tree.glb");
        cache.record(path, epoch(1), ThumbState::Ready(Handle::default()));
        cache.request(path, epoch(1), Subject::Model);
        assert!(cache.take_next().is_none(), "no work was queued");
        assert!(cache.ready(path, epoch(1)).is_some());
    }

    #[test]
    fn a_changed_mtime_invalidates_and_requeues() {
        let mut cache = ThumbnailCache::default();
        let path = Path::new("/kit/tree.glb");
        cache.record(path, epoch(1), ThumbState::Ready(Handle::default()));
        assert!(
            cache.ready(path, epoch(2)).is_none(),
            "the cached thumbnail is stale for the new mtime"
        );
        cache.request(path, epoch(2), Subject::Model);
        assert_eq!(
            cache.take_next(),
            Some((path.to_path_buf(), Subject::Model))
        );
    }

    /// A material saved again is a new modification time, which is what puts
    /// its tile back in the queue instead of keeping the old picture.
    #[test]
    fn a_material_saved_again_is_photographed_again() {
        let mut cache = ThumbnailCache::default();
        let path = Path::new("/project/assets/materials/bark.material.bsn");
        cache.record(path, epoch(1), ThumbState::Ready(Handle::default()));
        cache.request(path, epoch(1), Subject::Material);
        assert!(cache.take_next().is_none(), "nothing changed on disk");

        cache.request(path, epoch(2), Subject::Material);
        assert_eq!(
            cache.take_next(),
            Some((path.to_path_buf(), Subject::Material)),
            "the saved file is photographed again"
        );
    }

    /// A subject whose models cannot load is written off on the failure rather
    /// than holding the queue for the whole job timeout.
    #[test]
    fn a_subject_whose_models_have_all_failed_is_written_off() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::world_serialization::WorldSerializationPlugin,
        ));
        let missing: Handle<WorldAsset> = app
            .world()
            .resource::<AssetServer>()
            .load("nowhere/missing.glb#Scene0");
        let root = app.world_mut().spawn(WorldAssetRoot(missing.clone())).id();
        let bare = app.world_mut().spawn_empty().id();
        for _ in 0..200 {
            app.update();
            if app
                .world()
                .resource::<AssetServer>()
                .load_state(missing.id())
                .is_failed()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }

        let written_off = app
            .world_mut()
            .run_system_once(
                move |assets: Res<AssetServer>,
                      children: Query<&Children>,
                      models: Query<&WorldAssetRoot>| {
                    (
                        every_model_failed(&assets, root, &children, &models),
                        every_model_failed(&assets, bare, &children, &models),
                    )
                },
            )
            .expect("the check ran");

        assert!(written_off.0, "the model it names cannot load");
        assert!(
            !written_off.1,
            "a subject naming no model is still waiting for its meshes"
        );
    }

    /// A prefab built out of other prefabs is photographed with the models
    /// they bring, rather than as an empty frame.
    #[test]
    fn a_prefab_takes_the_models_the_prefab_it_names_brings() {
        let dir = tempfile::tempdir().expect("tempdir");
        let model = GltfSource::type_path();
        std::fs::create_dir_all(dir.path().join("parts")).expect("a parts folder");
        std::fs::write(
            dir.path().join("parts/tree.bsn"),
            format!(
                "jackdaw::prefab::components::Prefab\n\
                 jackdaw::prefab::components::PrefabEntityId(0)\n\
                 {model} {{ path: \"models/tree.glb\", scene_index: 0 }}\n"
            ),
        )
        .expect("the prefab is written");
        std::fs::write(
            dir.path().join("grove.bsn"),
            "jackdaw::prefab::components::Prefab\n\
             jackdaw::prefab::components::PrefabEntityId(0)\n\
             jackdaw::prefab::components::IsA { source: \"parts/tree.bsn\", deleted: [] }\n",
        )
        .expect("the prefab is written");
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        registry.write().register::<GltfSource>();
        world.insert_resource(registry);
        let root = world.spawn_empty().id();

        let built = build_prefab_subject(&mut world, root, &dir.path().join("grove.bsn"));

        assert!(built, "the prefab it names brings a model to photograph");
    }

    #[test]
    fn a_failed_model_is_never_retried_at_the_same_mtime() {
        let mut cache = ThumbnailCache::default();
        let path = Path::new("/kit/broken.gltf");
        cache.record(path, epoch(1), ThumbState::Failed);
        cache.request(path, epoch(1), Subject::Model);
        assert!(cache.take_next().is_none(), "a failure is not retried");
        assert!(cache.ready(path, epoch(1)).is_none(), "and draws no image");

        // Fixing the file on disk is what makes it eligible again.
        cache.request(path, epoch(2), Subject::Model);
        assert_eq!(
            cache.take_next(),
            Some((path.to_path_buf(), Subject::Model))
        );
    }

    /// M12 pinning test: `ThumbnailCache::is_failed` (which
    /// `update_thumbnail_slots` uses to latch `slot.applied` and stop
    /// hitting the filesystem every frame) must read `true` for a
    /// failure at the current mtime and `false` for both "never tried"
    /// and "failed at a stale mtime" -- both of the latter deserve
    /// another attempt, not a permanent latch.
    #[test]
    fn is_failed_reads_true_only_for_a_failure_at_the_current_mtime() {
        let mut cache = ThumbnailCache::default();
        let path = Path::new("/kit/broken.gltf");
        assert!(!cache.is_failed(path, epoch(1)), "never attempted yet");

        cache.record(path, epoch(1), ThumbState::Failed);
        assert!(cache.is_failed(path, epoch(1)));
        assert!(
            !cache.is_failed(path, epoch(2)),
            "the file changed since the failure; it deserves another try"
        );

        cache.record(path, epoch(1), ThumbState::Ready(Handle::default()));
        assert!(
            !cache.is_failed(path, epoch(1)),
            "a later success clears it"
        );
    }

    #[test]
    fn requesting_the_same_path_twice_queues_it_once() {
        let mut cache = ThumbnailCache::default();
        let path = Path::new("/kit/tree.glb");
        cache.request(path, epoch(1), Subject::Model);
        cache.request(path, epoch(1), Subject::Model);
        assert_eq!(
            cache.take_next(),
            Some((path.to_path_buf(), Subject::Model))
        );
        assert!(cache.take_next().is_none());
    }

    #[test]
    fn a_failure_is_reported_once_per_file() {
        let mut thumbnails = Thumbnails::default();
        let path = Path::new("/kit/broken.gltf");
        thumbnails.fail(path, epoch(1), "could not be photographed");
        assert!(
            !thumbnails.warned.insert(path.to_path_buf()),
            "the file is remembered, so a second attempt stays quiet"
        );
        thumbnails.forget(path);
        assert!(
            thumbnails.warned.insert(path.to_path_buf()),
            "forgetting the file lets it report again"
        );
    }

    #[test]
    fn a_scene_picture_is_named_after_the_scene_under_the_project() {
        let cache_dir = Path::new("/project/.jackdaw/thumbnails");
        let assets = Path::new("/project/assets");
        let scene = Path::new("/project/assets/world/zone.bsn");
        let file = scene_thumbnail_file(cache_dir, Some(assets), scene);
        assert_eq!(
            file,
            cache_dir.join("scenes").join("world").join("zone.bsn.png")
        );
        assert!(
            !file.starts_with(assets),
            "a picture is never written into the project's assets"
        );
    }

    /// One frame can be photographed once, so a round of saves pictures the
    /// scene the viewport is showing rather than writing that one frame under
    /// the name of every scene it wrote.
    #[test]
    fn saving_every_tab_pictures_only_the_scene_on_screen() {
        let zone = PathBuf::from("/project/assets/zone.bsn");
        let draft = PathBuf::from("/project/assets/scratch/draft.bsn");
        let pending = [zone.clone(), draft.clone()];

        assert_eq!(picture_target(&pending, Some(&draft)), Some(draft));
        assert_eq!(
            picture_target(&pending, Some(Path::new("/project/assets/other.bsn"))),
            None,
            "a scene the viewport is not showing is not pictured from its frame"
        );
        assert_eq!(picture_target(&pending, None), None);
    }

    #[test]
    fn a_scene_outside_the_assets_folder_still_gets_a_file_name() {
        let cache_dir = Path::new("/project/.jackdaw/thumbnails");
        let file = scene_thumbnail_file(cache_dir, None, Path::new("/elsewhere/zone.bsn"));
        assert!(file.starts_with(cache_dir.join("scenes")), "{file:?}");
        assert_eq!(file.extension().and_then(|e| e.to_str()), Some("png"));
    }

    #[test]
    fn fit_distance_scales_with_radius_and_shrinks_with_field_of_view() {
        let fov = core::f32::consts::FRAC_PI_4;
        let near = fit_distance(1.0, fov, 1.0);
        let far = fit_distance(2.0, fov, 1.0);
        assert!((far - near * 2.0).abs() < 1e-4, "{near} {far}");
        // A 1-unit sphere at a 45-degree fov sits at 1 / sin(22.5 degrees).
        assert!((near - 1.0 / (fov * 0.5).sin()).abs() < 1e-4, "{near}");
        assert!(
            fit_distance(1.0, fov * 2.0, 1.0) < near,
            "a wider lens can stand closer"
        );
    }

    #[test]
    fn a_degenerate_model_still_gets_a_usable_camera() {
        let (position, target) = frame_bounds(Vec3::ZERO, Vec3::ZERO, core::f32::consts::FRAC_PI_4);
        assert_eq!(target, Vec3::ZERO);
        assert!(position.is_finite(), "{position}");
        assert!(
            position.length() >= MIN_CAMERA_DISTANCE,
            "the camera is not inside its subject: {position}"
        );
    }

    #[test]
    fn framing_centres_an_off_origin_model() {
        let min = Vec3::new(10.0, 0.0, -4.0);
        let max = Vec3::new(12.0, 3.0, -2.0);
        let (position, target) = frame_bounds(min, max, core::f32::consts::FRAC_PI_4);
        assert_eq!(target, (min + max) * 0.5);
        let radius = (max - min).length() * 0.5;
        let expected = fit_distance(radius, core::f32::consts::FRAC_PI_4, FRAMING_MARGIN);
        assert!(
            ((position - target).length() - expected).abs() < 1e-3,
            "{position} {expected}"
        );
    }

    // -- Tiles ---------------------------------------------------------------

    fn slot_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .init_resource::<Thumbnails>()
            .add_systems(Update, update_thumbnail_slots);
        app
    }

    fn spawn_slot(app: &mut App, path: &Path, subject: Subject, y: f32) -> Entity {
        app.world_mut()
            .spawn((
                ThumbnailSlot::new(path.to_path_buf(), subject),
                Node::default(),
                UiGlobalTransform::from(bevy::math::Affine2::from_translation(Vec2::new(0.0, y))),
                ComputedNode::default(),
            ))
            .id()
    }

    fn drawn_images(app: &App, slot: Entity) -> usize {
        app.world()
            .get::<Children>(slot)
            .map(|kids| {
                kids.iter()
                    .filter(|kid| app.world().get::<ImageNode>(*kid).is_some())
                    .count()
            })
            .unwrap_or(0)
    }

    /// A material tile draws the picture the stage produced, in place of the
    /// kind icon it started with.
    #[test]
    fn a_material_tile_carries_a_rendered_image_once_the_stage_has_run() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("bark.material.bsn");
        std::fs::write(&path, "material").expect("the file is written");
        let mtime = source_mtime(&path).expect("the file has a modification time");

        let mut app = slot_app();
        let slot = spawn_slot(&mut app, &path, Subject::Material, 0.0);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .reserve_handle();
        app.world_mut().resource_mut::<Thumbnails>().cache.record(
            &path,
            mtime,
            ThumbState::Ready(handle),
        );

        app.update();

        assert_eq!(
            drawn_images(&app, slot),
            1,
            "the tile draws the rendered material"
        );
    }

    const PREFAB_WITH_A_MODEL: &str = r#"jackdaw::prefab::components::Prefab
#lamp_post
bevy_transform::components::transform::Transform
bevy_ecs::hierarchy::Children [
    #LampPost
    jackdaw_scene_types::types::GltfSource {
        path: "models/lantern.glb",
        scene_index: 0,
    }
]
"#;

    const PREFAB_WITHOUT_A_MODEL: &str = r#"jackdaw::prefab::components::Prefab
#marker
bevy_transform::components::transform::Transform
"#;

    fn prefab_app() -> App {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, bevy::asset::AssetPlugin::default()))
            .init_asset::<Image>()
            .register_type::<Transform>()
            .register_type::<GltfSource>();
        app
    }

    /// A prefab is pictured by the models it names; one that names none has
    /// nothing to photograph and keeps the icon it started with.
    #[test]
    fn a_prefab_with_meshes_gets_a_thumbnail_and_one_without_keeps_its_icon() {
        let temp = tempfile::tempdir().expect("tempdir");
        let with = temp.path().join("lamp_post.bsn");
        let without = temp.path().join("marker.bsn");
        std::fs::write(&with, PREFAB_WITH_A_MODEL).expect("the prefab is written");
        std::fs::write(&without, PREFAB_WITHOUT_A_MODEL).expect("the prefab is written");

        let mut app = prefab_app();
        let root = app.world_mut().spawn(Transform::IDENTITY).id();
        assert!(
            build_prefab_subject(app.world_mut(), root, &with),
            "the prefab names a model to photograph"
        );

        let bare = app.world_mut().spawn(Transform::IDENTITY).id();
        assert!(
            !build_prefab_subject(app.world_mut(), bare, &without),
            "a prefab with no models keeps its icon"
        );
    }

    /// A scene's picture is the capture its last save wrote; one that was
    /// never saved in this editor has none and keeps its icon.
    #[test]
    fn a_saved_scene_shows_its_picture_and_an_unsaved_one_keeps_its_icon() {
        let temp = tempfile::tempdir().expect("tempdir");
        let cache_dir = temp.path().join("thumbnails");
        let assets = temp.path().join("assets");
        std::fs::create_dir_all(&assets).expect("the folder is made");
        let saved = assets.join("zone.bsn");
        let never = assets.join("draft.bsn");
        std::fs::write(&saved, "scene").expect("the scene is written");
        std::fs::write(&never, "scene").expect("the scene is written");

        let thumbnails = Thumbnails {
            cache_dir: Some(cache_dir.clone()),
            assets_dir: Some(assets.clone()),
            ..Default::default()
        };

        let picture = thumbnails
            .scene_file(&saved)
            .expect("the project has a cache");
        std::fs::create_dir_all(picture.parent().expect("a folder")).expect("the folder is made");
        ::image::RgbImage::new(4, 4)
            .save_with_format(&picture, ::image::ImageFormat::Png)
            .expect("the picture is written");

        let mut images = Assets::<Image>::default();
        assert!(
            load_file(&picture, &mut images).is_some(),
            "the saved scene's tile draws the capture the save wrote"
        );
        let missing = thumbnails
            .scene_file(&never)
            .expect("the project has a cache");
        assert!(
            load_file(&missing, &mut images).is_none(),
            "a scene never saved in this editor has no picture"
        );
    }

    /// Only what is on screen costs a render: a tile scrolled far below the
    /// grid is never asked for.
    #[test]
    fn a_thumbnail_is_not_requested_for_an_undrawn_tile() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("tree.glb");
        std::fs::write(&path, "model").expect("the file is written");

        let mut app = slot_app();
        app.world_mut().spawn((
            crate::project_window::ProjectFileGrid,
            Node::default(),
            UiGlobalTransform::default(),
            ComputedNode::default(),
        ));
        spawn_slot(&mut app, &path, Subject::Model, 10_000.0);

        app.update();

        assert!(
            app.world_mut()
                .resource_mut::<Thumbnails>()
                .cache
                .take_next()
                .is_none(),
            "an off-screen tile queues no work"
        );
    }
}
