//! Rendered thumbnails for `.glb` / `.gltf` entries in the asset browser.
//!
//! A folder of models used to browse as a wall of identical font glyphs,
//! which makes a 300-model kit unusable. This module photographs each model
//! off-screen once and caches the result as a PNG under the project's
//! `.jackdaw/thumbnails/`, so the second visit to a folder costs a file read
//! rather than a scene load and a GPU pass.
//!
//! The off-screen setup copies [`crate::material_preview`]: a camera with
//! `RenderTarget::Image`, its own [`RenderLayers`] so nothing leaks into a
//! viewport, and its own [`EnvironmentMapLight`] because lighting is
//! per-view rather than global. It differs in two ways: one target image is
//! reused for every model (the pixels are read back and written to disk, so
//! nothing needs to stay resident), and the whole thing is driven by a
//! bounded work queue rather than by a selection.
//!
//! Three rules the queue exists to keep:
//!
//! - **Never stall the editor.** At most one model is rendered at a time and
//!   at most `DISK_LOADS_PER_FRAME` cached PNGs are decoded per frame.
//! - **Never re-render what is already on disk.** The cache is keyed by path
//!   *and* source mtime, so an edited model regenerates and an untouched one
//!   never does, across editor restarts.
//! - **Never retry a model that failed.** A `.gltf` that cannot load is
//!   marked failed for that mtime and skipped until the file changes.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use bevy::{
    asset::{RenderAssetUsages, embedded_asset, load_embedded_asset},
    camera::{RenderTarget, visibility::RenderLayers},
    gltf::GltfAssetLabel,
    image::{CompressedImageFormats, ImageSampler, ImageType},
    prelude::*,
    render::render_resource::TextureFormat,
    render::view::screenshot::{Screenshot, ScreenshotCaptured},
    ui::UiGlobalTransform,
    world_serialization::{WorldAsset, WorldAssetRoot},
};
use jackdaw_feathers::tokens;

/// Render layer the thumbnail stage owns exclusively. Layer 0 is the world,
/// layer 1 is the material preview, and per-viewport grids start at layer 3
/// (`crate::viewport::ViewportLayerCounter`); this one sits between them so
/// a model being photographed never appears in a viewport, and a viewport's
/// grid never appears in a thumbnail.
pub(crate) const THUMBNAIL_LAYER: usize = 2;

/// Edge of the square render target, in pixels. The browser draws the result
/// at [`THUMBNAIL_DISPLAY_SIZE`]; rendering larger keeps it crisp on a
/// high-DPI display and costs about 8 KiB of PNG per model on disk.
const THUMBNAIL_SIZE: u32 = 128;

/// Edge of the thumbnail as drawn in a browser tile. Close to the glyph it
/// replaces (`tokens::ICON_LG_PX` at 24 px plus its line box) so a folder of
/// mixed models and other files keeps one grid rhythm. The browser sizes the
/// glyph's slot to match, so nothing reflows when a thumbnail arrives.
pub(crate) const THUMBNAIL_DISPLAY_SIZE: f32 = 40.0;

/// Cached PNGs decoded per frame. Decoding a 128x128 PNG takes well under a
/// millisecond, so this is generous; the point of the cap is that opening a
/// 364-entry folder cannot turn one frame into a 364-file read.
const DISK_LOADS_PER_FRAME: usize = 8;

/// Frames a single render job may spend in one state before it is written
/// off as failed. A model whose asset load never resolves, or whose scene
/// never spawns a mesh, must not pin the queue forever.
const JOB_TIMEOUT_FRAMES: u32 = 600;

/// Frames to let the thumbnail camera draw the framed model before the
/// read-back is queued. `Screenshot` swaps the target's output attachment
/// and captures the *next* render into it, so there has to be at least one
/// more render after the model is in place.
const SETTLE_FRAMES: u32 = 2;

/// How far outside the browser's scroll viewport a tile still counts as
/// visible, in pixels. Roughly two extra rows above and below, so a slow
/// scroll meets thumbnails that are already there.
const VISIBLE_MARGIN_PX: f32 = 160.0;

/// How much bigger than the model the framing is. At 1.0 the bounding
/// sphere exactly touches the frustum, which clips the corners of a boxy
/// model; the excess is the margin around the subject.
const FRAMING_MARGIN: f32 = 1.25;

/// Distance floor, so a degenerate or empty model does not put the camera
/// inside its own subject.
const MIN_CAMERA_DISTANCE: f32 = 0.25;

/// The direction the camera looks from, in model space. A three-quarter
/// view reads as a solid object where a face-on view reads as a silhouette.
const VIEW_DIRECTION: Vec3 = Vec3::new(1.0, 0.75, 1.0);

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
    app.init_resource::<ModelThumbnails>()
        .add_systems(OnEnter(crate::AppState::Editor), setup_thumbnail_stage)
        .add_systems(
            Update,
            (drive_thumbnail_queue, update_thumbnail_slots)
                .run_if(in_state(crate::AppState::Editor)),
        );
}

/// True when `path` names a glTF model the browser should photograph.
pub fn is_model_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("glb") || e.eq_ignore_ascii_case("gltf"))
}

// -- Cache -------------------------------------------------------------------

/// What is known about one model's thumbnail.
#[derive(Clone, Debug)]
enum ThumbState {
    /// Photographed (or read back from disk) and ready to draw.
    Ready(Handle<Image>),
    /// The model could not be loaded or photographed. Not retried until the
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
/// GPU or an `App`. Same shape as `AssetBrowserState`'s `PrefabCheckCache`,
/// which memoizes prefab detection the same way.
#[derive(Default)]
struct ThumbnailCache {
    // Grows for the life of the process: entries are replaced on a stale
    // mtime but never evicted for a path that stops being referenced
    // (deleted model, browser navigated elsewhere for good). A project
    // with a very large, high-churn asset set could grow this
    // unboundedly over a long editor session. Not a problem this round
    // (one `Handle<Image>` + a mtime per entry), but a real cache would
    // want an eviction policy if that ever changes.
    entries: HashMap<PathBuf, CacheEntry>,
    queue: VecDeque<PathBuf>,
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
    /// is dropped so the model is photographed again.
    fn request(&mut self, path: &Path, mtime: SystemTime) {
        match self.entries.get(path) {
            Some(entry) if entry.mtime == mtime => return,
            Some(_) => {
                self.entries.remove(path);
            }
            None => {}
        }
        if self.queued.insert(path.to_path_buf()) {
            self.queue.push_back(path.to_path_buf());
        }
    }

    fn is_queued(&self, path: &Path) -> bool {
        self.queued.contains(path)
    }

    fn take_next(&mut self) -> Option<PathBuf> {
        let path = self.queue.pop_front()?;
        self.queued.remove(&path);
        Some(path)
    }

    fn record(&mut self, path: &Path, mtime: SystemTime, state: ThumbState) {
        self.entries
            .insert(path.to_path_buf(), CacheEntry { mtime, state });
    }
}

/// The thumbnail cache, the pending queue, and the one in-flight render.
#[derive(Resource, Default)]
pub struct ModelThumbnails {
    cache: ThumbnailCache,
    job: Option<Job>,
    /// Where cached PNGs live. `None` until the editor knows its project,
    /// which also disables the whole feature -- there is nowhere to cache.
    cache_dir: Option<PathBuf>,
    /// Set by the read-back observer, consumed by [`drive_thumbnail_queue`].
    /// The observer runs outside this system's ordering, so its result is
    /// parked here rather than applied from inside it.
    capture_result: Option<bool>,
    /// Thumbnails photographed since the editor started. Reported in the log
    /// so a first pass over a large kit can be measured.
    rendered: usize,
}

impl ModelThumbnails {
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
    /// that latch, a permanently-broken model on screen paid this
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

    /// Queue `path` for a thumbnail if one is not already known or pending.
    pub fn request(&mut self, path: &Path) {
        if self.cache.is_queued(path) {
            return;
        }
        let Some(mtime) = source_mtime(path) else {
            return;
        };
        self.cache.request(path, mtime);
    }
}

/// The file's modification time, or `None` when it cannot be read -- an
/// unreadable model is not worth queueing.
fn source_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// File name of the cached PNG for `path` at `mtime`.
///
/// The mtime is part of the *name*, not just of the in-memory key, so a
/// stale PNG can never be mistaken for a fresh one across editor restarts;
/// the superseded file is simply orphaned. The path is folded with FNV-1a
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

/// Root of the model currently being photographed. Despawned when the job
/// ends, whichever way it ends.
#[derive(Component)]
struct ThumbnailSubject;

/// Handle of the shared render target, kept in its own resource so the
/// read-back does not have to go through the camera.
#[derive(Resource)]
struct ThumbnailTarget(Handle<Image>);

fn setup_thumbnail_stage(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut thumbnails: ResMut<ModelThumbnails>,
    project: Option<Res<crate::project::ProjectRoot>>,
    assets: Res<AssetServer>,
) {
    let layer = RenderLayers::layer(THUMBNAIL_LAYER);

    // `Bgra8UnormSrgb` rather than the material preview's `Rgba8Unorm`:
    // these pixels come back through `ScreenshotCaptured`, and
    // `Image::try_into_dynamic` (which `screenshot::write_png` calls)
    // rejects a non-srgb `Rgba8Unorm`. This is the format the viewport's own
    // capture path already proves out.
    let target = images.add(Image::new_target_texture(
        THUMBNAIL_SIZE,
        THUMBNAIL_SIZE,
        TextureFormat::Bgra8UnormSrgb,
        None,
    ));
    commands.insert_resource(ThumbnailTarget(target.clone()));

    thumbnails.cache_dir = project.map(|p| p.jackdaw_dir().join("thumbnails"));
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

/// One model's journey from a queued path to a PNG on disk.
enum Job {
    /// Waiting on the glTF asset load.
    Loading {
        path: PathBuf,
        mtime: SystemTime,
        scene: Handle<WorldAsset>,
        frames: u32,
    },
    /// The scene is spawned; waiting for its meshes to exist so the bounds
    /// can be measured.
    Spawned {
        path: PathBuf,
        mtime: SystemTime,
        root: Entity,
        frames: u32,
    },
    /// Framed and rendering; counting down to the read-back.
    Settling {
        path: PathBuf,
        mtime: SystemTime,
        root: Entity,
        frames: u32,
    },
    /// Read-back queued; waiting for the observer to report.
    Capturing {
        path: PathBuf,
        mtime: SystemTime,
        root: Entity,
        frames: u32,
    },
}

impl Job {
    fn path(&self) -> &Path {
        match self {
            Job::Loading { path, .. }
            | Job::Spawned { path, .. }
            | Job::Settling { path, .. }
            | Job::Capturing { path, .. } => path,
        }
    }

    fn mtime(&self) -> SystemTime {
        match self {
            Job::Loading { mtime, .. }
            | Job::Spawned { mtime, .. }
            | Job::Settling { mtime, .. }
            | Job::Capturing { mtime, .. } => *mtime,
        }
    }

    fn root(&self) -> Option<Entity> {
        match self {
            Job::Loading { .. } => None,
            Job::Spawned { root, .. }
            | Job::Settling { root, .. }
            | Job::Capturing { root, .. } => Some(*root),
        }
    }

    fn frames(&self) -> u32 {
        match self {
            Job::Loading { frames, .. }
            | Job::Spawned { frames, .. }
            | Job::Settling { frames, .. }
            | Job::Capturing { frames, .. } => *frames,
        }
    }

    fn tick(&mut self) {
        match self {
            Job::Loading { frames, .. }
            | Job::Spawned { frames, .. }
            | Job::Settling { frames, .. }
            | Job::Capturing { frames, .. } => *frames += 1,
        }
    }
}

/// What one step of the state machine decided to do.
enum Step {
    /// Stay where you are; try again next frame.
    Wait,
    /// Move to this state.
    Advance(Job),
    /// The job is over. Despawn `root` if there is one and record `state`.
    Finish(Option<Entity>, ThumbState),
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
    mut thumbnails: ResMut<ModelThumbnails>,
    mut images: ResMut<Assets<Image>>,
    assets: Res<AssetServer>,
    meshes: Res<Assets<Mesh>>,
    children_query: Query<&Children>,
    mesh_query: Query<(&Mesh3d, &GlobalTransform)>,
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
        job.tick();
        let step = if job.frames() > JOB_TIMEOUT_FRAMES {
            warn!("model thumbnail: {} timed out", job.path().display());
            Step::Finish(job.root(), ThumbState::Failed)
        } else {
            step_job(
                &mut commands,
                &mut thumbnails,
                &mut images,
                &assets,
                &meshes,
                &children_query,
                &mesh_query,
                &view_dependent,
                &mut camera_query,
                &target.0,
                &job,
            )
        };
        match step {
            Step::Wait => thumbnails.job = Some(job),
            Step::Advance(next) => thumbnails.job = Some(next),
            Step::Finish(root, state) => {
                if let Some(root) = root
                    && let Ok(mut entity) = commands.get_entity(root)
                {
                    entity.try_despawn();
                }
                thumbnails.cache.record(job.path(), job.mtime(), state);
            }
        }
        return;
    }

    // Nothing in flight: drain cached entries, and start a render on the
    // first path that has no PNG on disk.
    for _ in 0..DISK_LOADS_PER_FRAME {
        let Some(path) = thumbnails.cache.take_next() else {
            return;
        };
        let Some(mtime) = source_mtime(&path) else {
            continue;
        };
        if let Some(handle) = load_cached(&thumbnails, &path, mtime, &mut images) {
            thumbnails
                .cache
                .record(&path, mtime, ThumbState::Ready(handle));
            continue;
        }
        let scene = assets.load(GltfAssetLabel::Scene(0).from_asset(to_asset_path(&path)));
        thumbnails.job = Some(Job::Loading {
            path,
            mtime,
            scene,
            frames: 0,
        });
        return;
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "state-machine step; every argument is one stage's dependency"
)]
fn step_job(
    commands: &mut Commands,
    thumbnails: &mut ModelThumbnails,
    images: &mut Assets<Image>,
    assets: &AssetServer,
    meshes: &Assets<Mesh>,
    children_query: &Query<&Children>,
    mesh_query: &Query<(&Mesh3d, &GlobalTransform)>,
    view_dependent: &Query<(), With<crate::ViewDependentBounds>>,
    camera_query: &mut Query<(&mut Transform, &Projection), With<ThumbnailCamera>>,
    target: &Handle<Image>,
    job: &Job,
) -> Step {
    match job {
        Job::Loading {
            path, mtime, scene, ..
        } => {
            let state = assets.load_state(scene.id());
            if state.is_failed() {
                warn!("model thumbnail: {} failed to load", path.display());
                return Step::Finish(None, ThumbState::Failed);
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
            Step::Advance(Job::Spawned {
                path: path.clone(),
                mtime: *mtime,
                root,
                frames: 0,
            })
        }
        Job::Spawned {
            path, mtime, root, ..
        } => {
            let mut vertices = Vec::new();
            crate::viewport_overlays::collect_descendant_mesh_world_vertices(
                *root,
                children_query,
                mesh_query,
                view_dependent,
                meshes,
                &mut vertices,
            );
            if vertices.is_empty() {
                return Step::Wait; // scene not spawned, or meshes not loaded
            }

            let mut min = Vec3::splat(f32::INFINITY);
            let mut max = Vec3::splat(f32::NEG_INFINITY);
            for vertex in &vertices {
                min = min.min(*vertex);
                max = max.max(*vertex);
            }

            let fov = camera_query
                .iter()
                .next()
                .and_then(|(_, projection)| match projection {
                    Projection::Perspective(perspective) => Some(perspective.fov),
                    _ => None,
                })
                .unwrap_or(core::f32::consts::FRAC_PI_4);
            let (position, look_at) = frame_bounds(min, max, fov);
            if let Some((mut transform, _)) = camera_query.iter_mut().next() {
                *transform = Transform::from_translation(position).looking_at(look_at, Vec3::Y);
            }

            apply_layer_recursive(commands, *root, children_query);
            commands.entity(*root).insert(Visibility::Visible);

            Step::Advance(Job::Settling {
                path: path.clone(),
                mtime: *mtime,
                root: *root,
                frames: 0,
            })
        }
        Job::Settling {
            path,
            mtime,
            root,
            frames,
        } => {
            if *frames < SETTLE_FRAMES {
                return Step::Wait;
            }
            let Some(file) = thumbnails
                .cache_dir
                .as_ref()
                .map(|dir| dir.join(cache_file_name(path, *mtime)))
            else {
                return Step::Finish(Some(*root), ThumbState::Failed);
            };
            thumbnails.capture_result = None;
            commands.spawn(Screenshot::image(target.clone())).observe(
                move |capture: On<ScreenshotCaptured>, mut thumbs: ResMut<ModelThumbnails>| {
                    thumbs.capture_result =
                        Some(crate::screenshot::write_png(&capture.image, &file));
                },
            );
            Step::Advance(Job::Capturing {
                path: path.clone(),
                mtime: *mtime,
                root: *root,
                frames: 0,
            })
        }
        Job::Capturing {
            path, mtime, root, ..
        } => {
            let Some(wrote) = thumbnails.capture_result.take() else {
                return Step::Wait;
            };
            if !wrote {
                return Step::Finish(Some(*root), ThumbState::Failed);
            }
            // Read the thumbnail back through the same path a cache hit
            // takes, so a freshly rendered tile and a restored one are the
            // same image asset in the same format -- and so a write that
            // cannot be read back is caught here rather than next session.
            match load_cached(thumbnails, path, *mtime, images) {
                Some(handle) => {
                    thumbnails.rendered += 1;
                    debug!(
                        "model thumbnail: rendered {} ({} this session)",
                        path.display(),
                        thumbnails.rendered
                    );
                    Step::Finish(Some(*root), ThumbState::Ready(handle))
                }
                None => Step::Finish(Some(*root), ThumbState::Failed),
            }
        }
    }
}

/// Read the cached PNG for `path` at `mtime` back into an image asset, if
/// one was written by this or an earlier session. This is the whole point of
/// the disk cache: reopening a 364-model folder costs 364 file reads rather
/// than 364 scene loads and GPU passes.
fn load_cached(
    thumbnails: &ModelThumbnails,
    path: &Path,
    mtime: SystemTime,
    images: &mut Assets<Image>,
) -> Option<Handle<Image>> {
    let file = thumbnails
        .cache_dir
        .as_ref()?
        .join(cache_file_name(path, mtime));
    let bytes = std::fs::read(&file).ok()?;
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
    crate::entity_ops::to_asset_path(&path.to_string_lossy().replace('\\', "/"))
}

/// Put every entity in the spawned glTF hierarchy on the thumbnail layer.
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

// -- Browser tiles -----------------------------------------------------------

/// The square of a model tile that holds either the fallback glyph or the
/// rendered thumbnail. Placed by the asset browser; driven from here so the
/// browser does not have to know about render state.
#[derive(Component)]
pub struct ModelThumbnailSlot {
    pub path: PathBuf,
    /// Set once the image is installed, which takes the slot out of the
    /// per-frame work entirely: scrolling back over a generated thumbnail
    /// costs nothing.
    applied: bool,
}

impl ModelThumbnailSlot {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            applied: false,
        }
    }
}

/// Request thumbnails for the model tiles the user can actually see, and
/// install the ones that are ready.
///
/// Visibility is decided against the browser's scroll viewport rather than
/// against the directory listing, so opening a 364-model folder queues the
/// tiles on screen, not all 364. The on-screen test is pure arithmetic and
/// runs first, so an off-screen tile costs no filesystem call at all.
fn update_thumbnail_slots(
    mut commands: Commands,
    mut thumbnails: ResMut<ModelThumbnails>,
    mut slots: Query<(
        Entity,
        &mut ModelThumbnailSlot,
        &UiGlobalTransform,
        &ComputedNode,
    )>,
    content: Query<
        (&UiGlobalTransform, &ComputedNode),
        With<crate::asset_browser::AssetBrowserContent>,
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
                // Without it, a model that fails to render paid an
                // fs::metadata call here, and another inside `ready`
                // above, every single frame for as long as it stayed
                // visible.
                slot.applied = true;
                continue;
            }
            thumbnails.request(&slot.path);
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
    use core::time::Duration;

    // The render half of this module needs a live wgpu backend, which
    // `tests/util/mod.rs`'s `headless_app()` deliberately does not have
    // (`WgpuSettings { backends: None }` renders nothing). Everything below
    // is the pure half: the cache rules and the framing arithmetic. The
    // rendering is covered by the screenshot evidence run instead.

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
        cache.request(path, epoch(1));
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
        cache.request(path, epoch(2));
        assert_eq!(cache.take_next().as_deref(), Some(path));
    }

    #[test]
    fn a_failed_model_is_never_retried_at_the_same_mtime() {
        let mut cache = ThumbnailCache::default();
        let path = Path::new("/kit/broken.gltf");
        cache.record(path, epoch(1), ThumbState::Failed);
        cache.request(path, epoch(1));
        assert!(cache.take_next().is_none(), "a failure is not retried");
        assert!(cache.ready(path, epoch(1)).is_none(), "and draws no image");

        // Fixing the file on disk is what makes it eligible again.
        cache.request(path, epoch(2));
        assert_eq!(cache.take_next().as_deref(), Some(path));
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
        cache.request(path, epoch(1));
        cache.request(path, epoch(1));
        assert_eq!(cache.take_next().as_deref(), Some(path));
        assert!(cache.take_next().is_none());
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
}
