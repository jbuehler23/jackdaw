//! Viewport and window screenshot capture.
//!
//! The 3D viewport already renders off-screen into a dedicated `Image`
//! (`crate::viewport::build_viewport_panel`), so a viewport capture is a
//! [`Screenshot::image`] against an existing render target: no second render
//! pass and no window grab. Capturing works when the editor window is
//! unfocused, partly covered, or never had a cursor over it.
//!
//! A window capture ([`Screenshot::primary_window`]) is the whole editor
//! surface: palette, options bar, docked panels, and the viewport together.
//! `viewport.screenshot` shows no `bevy_ui` chrome.
//!
//! Entry points, both funneled through `spawn_capture`:
//!
//! - the `viewport.screenshot` and `window.screenshot` operators, for
//!   interactive use and scripted runs (`JACKDAW_RUN_OP`);
//! - the `JACKDAW_SHOT=<path>` environment variable, which waits for the
//!   editor to settle, captures the viewport once, and exits.

use std::path::{Path, PathBuf};

use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

use crate::prelude::*;
use crate::viewport::MainViewportCamera;

/// Names a PNG the editor writes once the viewport has settled, then
/// exits. Matches the other `JACKDAW_*` boot switches
/// (`JACKDAW_OPEN_PROJECT`, `JACKDAW_AUTO_OPEN`, `JACKDAW_PIE_WINDOWLESS`).
pub const ENV_SHOT: &str = "JACKDAW_SHOT";

/// Frames to let pass after entering the editor before an unattended
/// capture fires. The first frames after the state transition have no
/// meshes in them, and `ViewportNode` resizes the render target to the
/// panel a frame or two later still, so this counts frames rather than
/// keying off `OnEnter(AppState::Editor)`.
const SETTLE_FRAMES: u32 = 90;

/// Frames an unattended run keeps retrying a capture it could not start
/// before it gives up and exits non-zero. Without it, a boot that never
/// opens a viewport panel would hang a CI job forever.
const CAPTURE_TIMEOUT_FRAMES: u32 = 900;

pub(crate) fn plugin(app: &mut App) {
    if let Some(path) = std::env::var_os(ENV_SHOT) {
        app.insert_resource(ShotProbe {
            path: PathBuf::from(path),
            frames: 0,
            queued: false,
        });
    }
    app.add_systems(
        Update,
        drive_shot_probe.run_if(in_state(crate::AppState::Editor)),
    );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ViewportScreenshotOp>();
    ctx.register_menu_entry::<ViewportScreenshotOp>(TopLevelMenu::Tools);
    ctx.register_operator::<WindowScreenshotOp>();
    ctx.register_menu_entry::<WindowScreenshotOp>(TopLevelMenu::Tools);
}

/// The pending one-shot capture requested by [`ENV_SHOT`]. Absent when
/// the variable is unset, which is every interactive run.
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

/// Queue a capture of the main viewport's render target, written to
/// `path` as a PNG once the GPU readback lands.
///
/// The render target is read off [`MainViewportCamera`] and never off
/// [`crate::viewport::ActiveViewport`]: `ActiveViewport` tracks the
/// viewport *under the cursor*, and in an unattended run no cursor has
/// entered a viewport, so both its fields are `None`.
///
/// With `exit_when_done`, the app exits as soon as the file is written --
/// success when it landed, failure when it did not. That is the one-shot
/// `JACKDAW_SHOT` path; interactive captures pass `false`.
pub fn queue_capture(
    world: &mut World,
    path: PathBuf,
    exit_when_done: bool,
) -> Result<(), CaptureError> {
    let mut cameras = world.query_filtered::<&RenderTarget, With<MainViewportCamera>>();
    let target = cameras.iter(world).next().ok_or(CaptureError::NoViewport)?;
    let handle = target
        .as_image()
        .cloned()
        .ok_or(CaptureError::NotAnImageTarget)?;

    spawn_capture(world, Screenshot::image(handle), path, exit_when_done);
    Ok(())
}

/// Queue a capture of the whole primary window (palette, options bar, docked
/// panels, and viewport together), written to `path` as a PNG once the GPU
/// readback lands.
///
/// Unlike [`queue_capture`] this cannot fail up front: `Screenshot` targets the
/// primary window by [`bevy::window::WindowRef::Primary`], which the renderer
/// resolves itself, so there is no camera or render target to look up. A window
/// that never renders never fires the observer, and that case is undetectable
/// here.
pub fn queue_window_capture(world: &mut World, path: PathBuf, exit_when_done: bool) {
    spawn_capture(world, Screenshot::primary_window(), path, exit_when_done);
}

/// Spawn the [`Screenshot`] entity shared by every capture path: write
/// the PNG and, for unattended runs, exit once the GPU readback lands.
fn spawn_capture(world: &mut World, screenshot: Screenshot, path: PathBuf, exit_when_done: bool) {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        let _ = std::fs::create_dir_all(parent);
    }

    world.spawn(screenshot).observe(
        move |capture: On<ScreenshotCaptured>, mut exit: MessageWriter<AppExit>| {
            // Writing the PNG here rather than through bevy's
            // `save_to_disk` observer keeps write-then-exit in one
            // observer. Two observers on the same entity have no
            // ordering guarantee between them, so an exit hook added
            // alongside `save_to_disk` could run before the bytes
            // reached the disk.
            let wrote = write_png(&capture.image, &path);
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
///
/// Shared with [`crate::model_thumbnail`], which writes its off-screen
/// model renders through the same encoder rather than a second one.
pub(crate) fn write_png(image: &Image, path: &Path) -> bool {
    let dynamic = match image.clone().try_into_dynamic() {
        Ok(dynamic) => dynamic,
        Err(err) => {
            error!("screenshot: cannot convert the captured frame: {err}");
            return false;
        }
    };
    // Drop alpha: with HDR on it carries brightness rather than opacity,
    // and saving it straight through comes out looking wrong.
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

/// Count settle frames for an unattended run, then fire exactly one
/// capture. A capture that cannot start yet (the viewport panel is still
/// building) is retried until [`CAPTURE_TIMEOUT_FRAMES`] runs out.
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
            // Latch: `queued` doubles as "nothing left to do here", so
            // without setting it a capture that keeps failing past the
            // timeout re-entered this arm and re-emitted AppExit every
            // frame from then on.
            world.resource_mut::<ShotProbe>().queued = true;
            world.write_message(AppExit::error());
        }
        Err(_) => {}
    }
}

/// Capture the viewport to a PNG.
#[operator(
    id = "viewport.screenshot",
    label = "Screenshot Viewport",
    description = "Save what the 3D viewport is showing to a PNG file.",
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

/// Capture the whole editor window (palette, options bar, docked panels, and
/// viewport) to a PNG. Unlike `viewport.screenshot`, this includes the
/// `bevy_ui` chrome rather than only the 3D render target.
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
/// the open project, or under the working directory when no project is open.
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

    /// A capture against a world with no viewport camera reports it
    /// rather than panicking or silently doing nothing -- the operator's
    /// whole job is producing a file, so a miss has to be visible.
    #[test]
    fn capturing_without_a_viewport_reports_no_viewport() {
        let mut world = World::new();
        let err = queue_capture(&mut world, PathBuf::from("unused.png"), false)
            .expect_err("no viewport camera exists");
        assert_eq!(err, CaptureError::NoViewport);
    }

    /// A viewport camera that renders to a window (rather than to its
    /// own image) is not capturable through this path.
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

    /// The default path lands inside the open project, so an
    /// interactive capture does not scatter files across the cwd.
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
        // Colons are legal on unix and illegal on Windows; a timestamped
        // default has to be writable on both.
        assert!(!path.to_string_lossy().contains(':'), "{path:?}");
    }

    /// M12 pinning test: a capture that never gets a viewport keeps
    /// failing every frame past the timeout. Before the latch,
    /// `drive_shot_probe` re-entered the timeout arm on every subsequent
    /// call and wrote a fresh `AppExit` each time.
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
