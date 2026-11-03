//! Rendering a camera's view to a PNG at a chosen size, without the editor's own overlays.

use std::path::{Path, PathBuf};

use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::prelude::*;
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use jackdaw_api::prelude::*;

use crate::project::ProjectRoot;
use crate::viewport::MainViewportCamera;

/// Frames an offscreen capture camera renders before its image is read back, so pipelines and shadows settle.
const SETTLE_FRAMES: u32 = 12;

/// Largest edge a capture may ask for, in pixels.
const MAX_EDGE: u32 = 8192;

/// Frames an offscreen capture waits at most for the renderer to finish
/// compiling pipelines before it is read back anyway, with a warning.
pub const READY_CAP_FRAMES: u32 = 600;

/// Frames a capture still renders once no pipeline is compiling, so the new
/// pipelines draw before the readback.
const READY_SETTLE_FRAMES: u32 = 3;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<PipelinesPending>()
        .add_systems(Update, (settle_captures, keep_gizmos_out_of_recordings));
    if let Some(render_app) = app.get_sub_app_mut(bevy::render::RenderApp) {
        render_app.add_systems(bevy::render::ExtractSchedule, report_pending_pipelines);
    }
}

/// How many render pipelines the renderer was still compiling when it last
/// reported. `None` without a renderer, where nothing waits on it.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PipelinesPending(pub Option<usize>);

fn report_pending_pipelines(
    mut main_world: ResMut<bevy::render::MainWorld>,
    cache: Res<bevy::render::render_resource::PipelineCache>,
) {
    let waiting = Some(cache.waiting_pipelines().count());
    if let Some(mut pending) = main_world.get_resource_mut::<PipelinesPending>()
        && pending.0 != waiting
    {
        pending.0 = waiting;
    }
}

/// A camera recording the scene to a file. While one exists the editor's
/// gizmos are switched off, so their lines and markers stay out of the image.
#[derive(Component, Default)]
pub struct KeepsGizmosOut;

fn keep_gizmos_out_of_recordings(
    recording: Query<(), With<KeepsGizmosOut>>,
    store: Option<ResMut<GizmoConfigStore>>,
    mut held: Local<Vec<std::any::TypeId>>,
) {
    let Some(mut store) = store else {
        return;
    };
    if recording.is_empty() {
        if held.is_empty() {
            return;
        }
        for (group, config, _) in store.iter_mut() {
            if held.contains(group) {
                config.enabled = true;
            }
        }
        held.clear();
        return;
    }
    for (group, config, _) in store.iter_mut() {
        if config.enabled {
            config.enabled = false;
            held.push(*group);
        }
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ViewportCaptureOp>();
}

/// Where an offscreen capture goes once it is read back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureSink {
    /// A PNG at this path, recorded in the capture log.
    File(PathBuf),
    /// One tile of a views request, held until the request collects it.
    ViewTile { request: u64, index: usize },
}

/// An offscreen camera waiting to be read back.
#[derive(Component)]
pub struct PendingCapture {
    pub sink: CaptureSink,
    pub frames_left: u32,
    /// Frames spent waiting on the renderer's pipelines.
    pub waited: u32,
}

impl PendingCapture {
    /// The file the capture is written to, when it goes to one.
    pub fn path(&self) -> Option<&Path> {
        match &self.sink {
            CaptureSink::File(path) => Some(path),
            CaptureSink::ViewTile { .. } => None,
        }
    }
}

/// What an offscreen camera sees and how big its image is.
pub struct OffscreenCamera {
    pub transform: Transform,
    pub projection: Projection,
    pub layers: RenderLayers,
    pub width: u32,
    pub height: u32,
    /// The camera whose environment light and tone mapping it borrows.
    pub look_of: Option<Entity>,
}

/// Where a capture path lands: under the open project, whether given relative to it or absolute inside it.
fn capture_path(root: &Path, asked: &str) -> Result<PathBuf, String> {
    let asked = Path::new(asked);
    let relative = if asked.is_absolute() {
        asked
            .strip_prefix(root)
            .map_err(|_| format!("`{}` is outside the project", asked.display()))?
    } else {
        asked
    };
    crate::project::path_within(root, relative)
}

/// Spawn an offscreen camera seeing what `source` sees at `width` by `height`, to be written to `path`.
pub fn spawn_capture_camera(
    world: &mut World,
    source: Entity,
    width: u32,
    height: u32,
    path: PathBuf,
) -> Option<Entity> {
    let node = world.get_entity(source).ok()?;
    let transform = node
        .get::<GlobalTransform>()
        .map(GlobalTransform::compute_transform)
        .or_else(|| node.get::<Transform>().copied())?;
    let projection = node.get::<Projection>().cloned().unwrap_or_default();
    let camera = OffscreenCamera {
        transform,
        projection,
        layers: RenderLayers::layer(0),
        width,
        height,
        look_of: Some(source),
    };
    Some(spawn_offscreen_camera(
        world,
        camera,
        CaptureSink::File(path),
    ))
}

/// Spawn an offscreen camera rendering `camera` into an image of its own,
/// read back to `sink` once it has settled. Editor gizmos stay off while it
/// exists.
pub fn spawn_offscreen_camera(
    world: &mut World,
    camera: OffscreenCamera,
    sink: CaptureSink,
) -> Entity {
    let light = camera
        .look_of
        .and_then(|source| world.get::<EnvironmentMapLight>(source).cloned());
    let tonemapping = camera
        .look_of
        .and_then(|source| world.get::<Tonemapping>(source).copied());

    let mut image = Image::new_target_texture(
        camera.width,
        camera.height,
        TextureFormat::Rgba8UnormSrgb,
        None,
    );
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = world.resource_mut::<Assets<Image>>().add(image);

    let mut spawned = world.spawn((
        crate::EditorEntity,
        Camera3d::default(),
        Camera {
            order: -2,
            ..default()
        },
        RenderTarget::Image(target.into()),
        camera.transform,
        camera.projection,
        camera.layers,
        KeepsGizmosOut,
        PendingCapture {
            sink,
            frames_left: SETTLE_FRAMES,
            waited: 0,
        },
    ));
    if let Some(light) = light {
        spawned.insert(light);
    }
    if let Some(tonemapping) = tonemapping {
        spawned.insert(tonemapping);
    }
    spawned.id()
}

/// Read each offscreen camera back once it has rendered its settle frames
/// and the renderer has no pipeline left compiling. A capture that waits
/// [`READY_CAP_FRAMES`] is read back anyway and reported as possibly blank.
fn settle_captures(
    mut commands: Commands,
    pipelines: Res<PipelinesPending>,
    mut pending: Query<(Entity, &mut PendingCapture, &RenderTarget)>,
) {
    let compiling = pipelines.0.is_some_and(|waiting| waiting > 0);
    for (camera, mut capture, target) in &mut pending {
        if compiling && capture.waited < READY_CAP_FRAMES {
            capture.waited += 1;
            capture.frames_left = capture.frames_left.max(READY_SETTLE_FRAMES);
            continue;
        }
        if capture.frames_left > 0 {
            capture.frames_left -= 1;
            continue;
        }
        let late = capture.waited >= READY_CAP_FRAMES;
        let Some(image) = target.as_image().cloned() else {
            commands.entity(camera).despawn();
            continue;
        };
        let sink = capture.sink.clone();
        commands.entity(camera).remove::<PendingCapture>();
        commands.spawn(Screenshot::image(image)).observe(
            move |captured: On<ScreenshotCaptured>,
                  mut commands: Commands,
                  log: Option<ResMut<crate::screenshot::CaptureLog>>,
                  views: Option<ResMut<crate::views::ViewCaptures>>| {
                // A camera despawned before its readback belongs to a request
                // that has already ended.
                let Ok(mut spent) = commands.get_entity(camera) else {
                    return;
                };
                spent.despawn();
                match &sink {
                    CaptureSink::File(path) => {
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        if late {
                            warn!(
                                "capture: {} was read back while the renderer was still \
                                 compiling; it may be blank",
                                path.display()
                            );
                        }
                        if crate::screenshot::write_png(&captured.image, path)
                            && let Some(mut log) = log
                        {
                            log.record(
                                path.clone(),
                                (captured.image.width(), captured.image.height()),
                            );
                        }
                    }
                    CaptureSink::ViewTile { request, index } => {
                        if let Some(mut views) = views {
                            views.land(*request, *index, &captured.image, late);
                        }
                    }
                }
            },
        );
    }
}

/// Render a camera's view to a PNG at a chosen size, without the grid, gizmos or selection outlines.
#[operator(
    id = "viewport.capture",
    writes_disk = true,
    label = "Capture Camera",
    description = "Render what a camera sees to a PNG at a chosen size, without the editor's grid, \
                   gizmos or selection outlines.",
    allows_undo = false,
    params(
        camera(
            Entity,
            doc = "The camera to see through. Defaults to the viewport's camera."
        ),
        width(i64, doc = "Image width in pixels."),
        height(i64, doc = "Image height in pixels."),
        path(
            String,
            doc = "Where to write the PNG: relative to the project, or absolute inside it."
        ),
    )
)]
pub(crate) fn viewport_capture(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let id = "viewport.capture";
    let edge = |name: &str| {
        params
            .as_int(name)
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| (1..=MAX_EDGE).contains(value))
    };
    let (Some(width), Some(height)) = (edge("width"), edge("height")) else {
        warn_caller(
            world,
            format!("{id}: width and height are whole pixels from 1 to {MAX_EDGE}"),
        );
        return OperatorResult::Cancelled;
    };
    let Some(root) = world
        .get_resource::<ProjectRoot>()
        .map(|project| project.root.clone())
    else {
        warn_caller(world, format!("{id}: no project is open"));
        return OperatorResult::Cancelled;
    };
    let Some(asked) = params.as_str("path").filter(|path| !path.trim().is_empty()) else {
        warn_caller(world, format!("{id}: name the PNG to write with path="));
        return OperatorResult::Cancelled;
    };
    let path = match capture_path(&root, asked.trim()) {
        Ok(path) => path,
        Err(refusal) => {
            warn_caller(world, format!("{id}: {refusal}"));
            return OperatorResult::Cancelled;
        }
    };
    let source = params.as_entity("camera").or_else(|| {
        world
            .resource::<crate::viewport::ActiveViewport>()
            .camera
            .or_else(|| {
                let mut cameras = world.query_filtered::<Entity, With<MainViewportCamera>>();
                cameras.iter(world).next()
            })
    });
    let Some(source) = source.filter(|camera| world.get::<Camera3d>(*camera).is_some()) else {
        warn_caller(world, format!("{id}: there is no 3D camera to see through"));
        return OperatorResult::Cancelled;
    };
    world
        .resource_mut::<crate::screenshot::CaptureLog>()
        .forget(&path);
    match spawn_capture_camera(world, source, width, height, path) {
        Some(_) => OperatorResult::Finished,
        None => OperatorResult::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gizmos_are_off_while_a_recording_camera_exists_and_back_after() {
        let mut world = World::new();
        let mut store = GizmoConfigStore::default();
        store.insert(GizmoConfig::default(), DefaultGizmoConfigGroup);
        world.insert_resource(store);
        let enabled = |world: &World| {
            world
                .resource::<GizmoConfigStore>()
                .config::<DefaultGizmoConfigGroup>()
                .0
                .enabled
        };

        let recording = world.spawn(KeepsGizmosOut).id();
        world
            .run_system_cached(keep_gizmos_out_of_recordings)
            .expect("runs");
        assert!(!enabled(&world));

        world.despawn(recording);
        world
            .run_system_cached(keep_gizmos_out_of_recordings)
            .expect("runs");
        assert!(enabled(&world));
    }

    #[test]
    fn a_capture_path_stays_inside_the_project() {
        let root = tempfile::tempdir().expect("tempdir");
        let root = root.path();
        assert_eq!(
            capture_path(root, "target/shot.png").ok(),
            Some(dunce::canonicalize(root).unwrap().join("target/shot.png"))
        );
        assert!(capture_path(root, &root.join("target/abs.png").to_string_lossy()).is_ok());
        assert!(capture_path(root, "../outside.png").is_err());
        assert!(capture_path(root, "/tmp/elsewhere.png").is_err());
    }
}
