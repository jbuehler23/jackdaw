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

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Update, (settle_captures, keep_gizmos_out_of_recordings));
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

/// An offscreen camera waiting to be read back to `path`.
#[derive(Component)]
pub struct PendingCapture {
    pub path: PathBuf,
    pub frames_left: u32,
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
    let light = node.get::<EnvironmentMapLight>().cloned();
    let tonemapping = node.get::<Tonemapping>().copied();

    let mut image = Image::new_target_texture(width, height, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    let target = world.resource_mut::<Assets<Image>>().add(image);

    let mut camera = world.spawn((
        crate::EditorEntity,
        Camera3d::default(),
        Camera {
            order: -2,
            ..default()
        },
        RenderTarget::Image(target.into()),
        transform,
        projection,
        RenderLayers::layer(0),
        KeepsGizmosOut,
        PendingCapture {
            path,
            frames_left: SETTLE_FRAMES,
        },
    ));
    if let Some(light) = light {
        camera.insert(light);
    }
    if let Some(tonemapping) = tonemapping {
        camera.insert(tonemapping);
    }
    Some(camera.id())
}

fn settle_captures(
    mut commands: Commands,
    mut pending: Query<(Entity, &mut PendingCapture, &RenderTarget)>,
) {
    for (camera, mut capture, target) in &mut pending {
        if capture.frames_left > 0 {
            capture.frames_left -= 1;
            continue;
        }
        let Some(image) = target.as_image().cloned() else {
            commands.entity(camera).despawn();
            continue;
        };
        let path = capture.path.clone();
        commands.entity(camera).remove::<PendingCapture>();
        commands.spawn(Screenshot::image(image)).observe(
            move |captured: On<ScreenshotCaptured>,
                  mut commands: Commands,
                  log: Option<ResMut<crate::screenshot::CaptureLog>>| {
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if crate::screenshot::write_png(&captured.image, &path)
                    && let Some(mut log) = log
                {
                    log.record(
                        path.clone(),
                        (captured.image.width(), captured.image.height()),
                    );
                }
                if let Ok(mut spent) = commands.get_entity(camera) {
                    spent.despawn();
                }
            },
        );
    }
}

/// Render a camera's view to a PNG at a chosen size, without the grid, gizmos or selection outlines.
#[operator(
    id = "viewport.capture",
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
