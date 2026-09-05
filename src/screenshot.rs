//! Viewport and window screenshot capture.
//!
//! A viewport capture reads back the panel's existing render target; a window
//! capture takes the whole editor surface, `bevy_ui` chrome included.

use std::path::{Path, PathBuf};

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

use crate::prelude::*;
use crate::viewport::MainViewportCamera;
use crate::viewport_host::{ViewportHost, ViewportMode};

/// Names a PNG the editor writes once the viewport has settled, then exits.
pub const ENV_SHOT: &str = "JACKDAW_SHOT";

/// Frames to let pass after entering the editor before an unattended capture
/// fires; the first frames hold no meshes and the render target is still being
/// resized.
const SETTLE_FRAMES: u32 = 90;

/// Frames an unattended run keeps retrying a capture it could not start before
/// giving up and exiting non-zero.
const CAPTURE_TIMEOUT_FRAMES: u32 = 900;

pub(crate) fn plugin(app: &mut App) {
    if let Some(path) = std::env::var_os(ENV_SHOT) {
        app.insert_resource(ShotProbe {
            path: PathBuf::from(path),
            frames: 0,
            queued: false,
        });
    }
    app.init_resource::<CaptureLog>();
    app.add_systems(
        Update,
        drive_shot_probe.run_if(in_state(crate::AppState::Editor)),
    );
}

/// Captures that have reached the disk, by the path they were written to.
///
/// A capture is queued frames before its GPU readback lands, so a caller that
/// needs the file polls here. Entries past [`CaptureLog::CAPACITY`] are
/// evicted oldest first.
#[derive(Resource, Default)]
pub struct CaptureLog {
    landed: bevy::platform::collections::HashMap<PathBuf, (u32, u32)>,
    /// Insertion order, for eviction. Holds the same paths as `landed`.
    order: std::collections::VecDeque<PathBuf>,
}

impl CaptureLog {
    /// Captures remembered before the oldest is dropped.
    pub const CAPACITY: usize = 64;

    /// Record a capture that has reached the disk.
    pub fn record(&mut self, path: PathBuf, size: (u32, u32)) {
        if self.landed.insert(path.clone(), size).is_none() {
            self.order.push_back(path);
        }
        while self.order.len() > Self::CAPACITY {
            if let Some(oldest) = self.order.pop_front() {
                self.landed.remove(&oldest);
            }
        }
    }

    /// The size of the capture written to `path`, if one has landed.
    pub fn size_of(&self, path: &Path) -> Option<(u32, u32)> {
        self.landed.get(path).copied()
    }

    /// Forget `path`, so a later capture there is not answered with this one.
    pub fn forget(&mut self, path: &Path) {
        if self.landed.remove(path).is_some() {
            self.order.retain(|held| held != path);
        }
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ViewportScreenshotOp>();
    ctx.register_menu_entry::<ViewportScreenshotOp>(TopLevelMenu::Tools);
    ctx.register_operator::<WindowScreenshotOp>();
    ctx.register_menu_entry::<WindowScreenshotOp>(TopLevelMenu::Tools);
}

/// The 2D presentation's own capture, registered only where a 2D viewport
/// exists.
pub(crate) fn add_2d_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<Viewport2dScreenshotOp>();
    ctx.register_menu_entry::<Viewport2dScreenshotOp>(TopLevelMenu::Tools);
}

/// The pending one-shot capture requested by [`ENV_SHOT`].
#[derive(Resource)]
struct ShotProbe {
    path: PathBuf,
    frames: u32,
    queued: bool,
}

/// Why a capture could not be started.
#[derive(Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// No viewport panel is open, so there is no camera to capture.
    NoViewport,
    /// A viewport camera exists but does not render to an image.
    NotAnImageTarget,
}

impl core::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NoViewport => write!(f, "no viewport panel is open to capture"),
            Self::NotAnImageTarget => {
                write!(f, "the viewport camera does not render to an image")
            }
        }
    }
}

impl core::error::Error for CaptureError {}

/// Queue a capture of what the first viewport panel is showing, written to
/// `path` as a PNG once the GPU readback lands.
///
/// The surface follows the panel's mode: the world in
/// [`ViewportMode::ThreeD`], the canvas in [`ViewportMode::TwoD`]. With
/// `exit_when_done`, the app exits once the file is written, reporting
/// failure when it did not land.
pub fn queue_capture(
    world: &mut World,
    path: PathBuf,
    exit_when_done: bool,
) -> Result<(), CaptureError> {
    match viewport_camera(world) {
        Some(camera) => queue_capture_of(world, camera, path, exit_when_done),
        None => queue_camera_capture::<MainViewportCamera>(world, path, exit_when_done),
    }
}

/// The camera the first viewport panel is showing through.
fn viewport_camera(world: &mut World) -> Option<Entity> {
    let mut panels = world.query::<(Entity, &ViewportHost)>();
    let (panel, mode) = panels.iter(world).next().map(|(e, host)| (e, host.mode))?;
    match mode {
        ViewportMode::ThreeD => world
            .get::<crate::viewport::ViewportPanelHost>(panel)
            .map(|host| host.camera),
        ViewportMode::TwoD => world
            .get::<crate::viewport_2d::Viewport2dPanelHost>(panel)
            .map(|host| host.camera),
    }
}

/// Queue a capture of a 2D viewport panel's render target: the authored UI
/// scene at its reference resolution, without the panel chrome around it.
pub fn queue_2d_capture(
    world: &mut World,
    path: PathBuf,
    exit_when_done: bool,
) -> Result<(), CaptureError> {
    let camera = primary_2d_camera(world).ok_or(CaptureError::NoViewport)?;
    queue_capture_of(world, camera, path, exit_when_done)
}

/// The 2D camera of the panel answering for the canvas.
fn primary_2d_camera(world: &mut World) -> Option<Entity> {
    let mut panels = world.query::<(Entity, &ViewportHost)>();
    let panel = crate::viewport_host::primary_2d_host(panels.iter(world))?;
    world
        .get::<crate::viewport_2d::Viewport2dPanelHost>(panel)
        .map(|host| host.camera)
}

/// Queue a capture of the render target belonging to the first camera marked
/// `C`.
fn queue_camera_capture<C: Component>(
    world: &mut World,
    path: PathBuf,
    exit_when_done: bool,
) -> Result<(), CaptureError> {
    let mut cameras = world.query_filtered::<Entity, With<C>>();
    let camera = cameras.iter(world).next().ok_or(CaptureError::NoViewport)?;
    queue_capture_of(world, camera, path, exit_when_done)
}

/// Queue a capture of `camera`'s render target, which has to be an image; a
/// camera drawing straight to the window has no texture to read back.
fn queue_capture_of(
    world: &mut World,
    camera: Entity,
    path: PathBuf,
    exit_when_done: bool,
) -> Result<(), CaptureError> {
    let handle = world
        .get::<RenderTarget>(camera)
        .ok_or(CaptureError::NoViewport)?
        .as_image()
        .cloned()
        .ok_or(CaptureError::NotAnImageTarget)?;

    spawn_capture(world, Screenshot::image(handle), path, exit_when_done);
    Ok(())
}

/// Queue a capture of the whole primary window, written to `path` as a PNG
/// once the GPU readback lands.
///
/// Unlike [`queue_capture`] this cannot fail up front; with no primary window
/// the observer never fires.
pub fn queue_window_capture(world: &mut World, path: PathBuf, exit_when_done: bool) {
    spawn_capture(world, Screenshot::primary_window(), path, exit_when_done);
}

/// Spawn the [`Screenshot`] entity shared by every capture path.
fn spawn_capture(world: &mut World, screenshot: Screenshot, path: PathBuf, exit_when_done: bool) {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = std::fs::create_dir_all(parent);
    }

    world.spawn(screenshot).observe(
        move |capture: On<ScreenshotCaptured>,
              mut exit: MessageWriter<AppExit>,
              log: Option<ResMut<CaptureLog>>| {
            // Writing here rather than through bevy's `save_to_disk` keeps
            // write-then-exit in one observer; two observers on an entity have
            // no ordering guarantee between them.
            let wrote = write_png(&capture.image, &path);
            if wrote && let Some(mut log) = log {
                log.record(
                    path.clone(),
                    (capture.image.width(), capture.image.height()),
                );
            }
            if exit_when_done {
                exit.write(if wrote {
                    AppExit::Success
                } else {
                    AppExit::error()
                });
            }
        },
    );
}

/// Encode a captured frame as a PNG on disk. Returns whether it landed.
pub(crate) fn write_png(image: &Image, path: &Path) -> bool {
    let dynamic = match image.clone().try_into_dynamic() {
        Ok(dynamic) => dynamic,
        Err(err) => {
            error!("screenshot: cannot convert the captured frame: {err}");
            return false;
        }
    };
    // Drop alpha: with HDR on it carries brightness rather than opacity.
    let rgb = dynamic.to_rgb8();
    match rgb.save_with_format(path, ::image::ImageFormat::Png) {
        Ok(()) => {
            info!("screenshot: wrote {}", path.display());
            true
        }
        Err(err) => {
            error!("screenshot: cannot write {}: {err}", path.display());
            false
        }
    }
}

/// Count settle frames for an unattended run, then fire exactly one capture,
/// retrying until `CAPTURE_TIMEOUT_FRAMES` runs out.
fn drive_shot_probe(world: &mut World) {
    let Some(probe) = world.get_resource::<ShotProbe>() else {
        return;
    };
    if probe.queued {
        return;
    }
    let frames = probe.frames + 1;
    world.resource_mut::<ShotProbe>().frames = frames;
    if frames < SETTLE_FRAMES {
        return;
    }

    let path = world.resource::<ShotProbe>().path.clone();
    match queue_capture(world, path, true) {
        Ok(()) => world.resource_mut::<ShotProbe>().queued = true,
        Err(err) if frames > SETTLE_FRAMES + CAPTURE_TIMEOUT_FRAMES => {
            error!("{ENV_SHOT}: {err}");
            // `queued` doubles as "nothing left to do", so latching it keeps
            // a failed capture from re-emitting AppExit every frame.
            world.resource_mut::<ShotProbe>().queued = true;
            world.write_message(AppExit::error());
        }
        Err(_) => {}
    }
}

/// Capture the viewport to a PNG: the world it is showing, or the canvas.
#[operator(
    id = "viewport.screenshot",
    label = "Screenshot Viewport",
    description = "Save what the viewport is showing to a PNG file.",
    allows_undo = false,
    params(path(
        String,
        doc = "Where to write the PNG. Defaults to a timestamped file in the project."
    ))
)]
pub(crate) fn viewport_screenshot(
    params: In<OperatorParameters>,
    project: Option<Res<crate::project::ProjectRoot>>,
    mut commands: Commands,
) -> OperatorResult {
    let path = match params.as_str("path").filter(|p| !p.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => default_shot_path(project.as_deref(), "viewport"),
    };
    commands.queue(move |world: &mut World| {
        if let Err(err) = queue_capture(world, path, false) {
            error!("viewport.screenshot: {err}");
        }
    });
    OperatorResult::Finished
}

/// Capture what the 2D viewport is showing to a PNG: the authored scene
/// alone, without the stage chrome drawn over it.
#[operator(
    id = "viewport2d.screenshot",
    label = "Screenshot 2D Viewport",
    description = "Save the UI scene the 2D viewport is showing to a PNG file, \
                   without the selection chrome; `window.screenshot` includes it.",
    allows_undo = false,
    params(path(
        String,
        doc = "Where to write the PNG. Defaults to a timestamped file in the project."
    ))
)]
pub(crate) fn viewport_2d_screenshot(
    params: In<OperatorParameters>,
    project: Option<Res<crate::project::ProjectRoot>>,
    mut commands: Commands,
) -> OperatorResult {
    let path = match params.as_str("path").filter(|p| !p.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => default_shot_path(project.as_deref(), "viewport-2d"),
    };
    commands.queue(move |world: &mut World| {
        if let Err(err) = queue_2d_capture(world, path, false) {
            error!("viewport2d.screenshot: {err}");
        }
    });
    OperatorResult::Finished
}

/// Capture the whole editor window to a PNG, `bevy_ui` chrome included.
#[operator(
    id = "window.screenshot",
    label = "Screenshot Window",
    description = "Save the whole editor window, including all UI panels, to a PNG file.",
    allows_undo = false,
    params(path(
        String,
        doc = "Where to write the PNG. Defaults to a timestamped file in the project."
    ))
)]
pub(crate) fn window_screenshot(
    params: In<OperatorParameters>,
    project: Option<Res<crate::project::ProjectRoot>>,
    mut commands: Commands,
) -> OperatorResult {
    let path = match params.as_str("path").filter(|p| !p.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => default_shot_path(project.as_deref(), "window"),
    };
    commands.queue(move |world: &mut World| {
        queue_window_capture(world, path, false);
    });
    OperatorResult::Finished
}

/// Where an unnamed capture goes: `screenshots/<prefix>-<timestamp>.png` under
/// the open project, or under the working directory.
fn default_shot_path(project: Option<&crate::project::ProjectRoot>, prefix: &str) -> PathBuf {
    let stamp = crate::timestamps::utc_rfc3339_now().replace(':', "-");
    let dir = project
        .map(|p| p.root.join("screenshots"))
        .unwrap_or_else(|| PathBuf::from("screenshots"));
    dir.join(format!("{prefix}-{stamp}.png"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capturing_without_a_viewport_reports_no_viewport() {
        let mut world = World::new();
        let err = queue_capture(&mut world, PathBuf::from("unused.png"), false)
            .expect_err("no viewport camera exists");
        assert_eq!(err, CaptureError::NoViewport);
    }

    #[test]
    fn capturing_a_window_target_reports_not_an_image_target() {
        let mut world = World::new();
        world.spawn((
            MainViewportCamera,
            RenderTarget::Window(bevy::window::WindowRef::Primary),
        ));
        let err = queue_capture(&mut world, PathBuf::from("unused.png"), false)
            .expect_err("the target is a window");
        assert_eq!(err, CaptureError::NotAnImageTarget);
    }

    #[test]
    fn default_path_is_a_png_under_the_project() {
        let root = PathBuf::from("/tmp/some-project");
        let project = crate::project::ProjectRoot {
            root: root.clone(),
            config: crate::project::ProjectConfig::default(),
        };
        let path = default_shot_path(Some(&project), "viewport");
        assert!(path.starts_with(root.join("screenshots")), "{path:?}");
        assert_eq!(path.extension().and_then(|e| e.to_str()), Some("png"));
        // Colons are illegal in Windows paths.
        assert!(!path.to_string_lossy().contains(':'), "{path:?}");
    }

    #[test]
    fn a_capture_that_never_starts_emits_app_exit_only_once() {
        let mut world = World::new();
        world.init_resource::<bevy::ecs::message::Messages<AppExit>>();
        world.insert_resource(ShotProbe {
            path: PathBuf::from("unused.png"),
            frames: SETTLE_FRAMES + CAPTURE_TIMEOUT_FRAMES,
            queued: false,
        });

        for _ in 0..5 {
            drive_shot_probe(&mut world);
        }

        assert_eq!(
            world
                .resource::<bevy::ecs::message::Messages<AppExit>>()
                .len(),
            1,
            "AppExit must be written exactly once, not once per frame",
        );
        assert!(
            world.resource::<ShotProbe>().queued,
            "must latch so drive_shot_probe stops retrying",
        );
    }
}
