//! Windowless play-in-editor: the game renders into the streamed image
//! instead of an OS window.
//!
//! The editor sets `JACKDAW_PIE_WINDOWLESS=1` on embedded launches. The
//! game's `main` wraps its plugin group with [`maybe_windowless`], which
//! drops winit and the primary window and drives the loop with a schedule
//! runner. A data-only `Window` entity (no `RawHandleWrapper`, so the
//! renderer never creates a surface for it) stands in for the real window:
//! cursor position, UI layout, and focus reads all keep working. Every
//! camera that targets a window is retargeted at the capture image, so the
//! scene renders exactly once per frame and menus stream without a camera
//! rig.

use bevy::app::PluginGroupBuilder;
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::{TextureFormat, TextureUsages};
use bevy::window::{CursorOptions, ExitCondition, PrimaryWindow, WindowPlugin};

/// Initial capture size until the editor's first `StartFrameStream`.
pub(crate) const DEFAULT_SIZE: UVec2 = UVec2::new(1280, 720);

/// True when the editor asked this process to run without an OS window.
///
/// Requires the PIE link variable as well: dropping the OS window only makes
/// sense paired with the editor link that installs the virtual window and the
/// capture stream, so a windowless flag on its own leaves the normal window in
/// place rather than yielding a windowless, input-dead process.
pub fn windowless_requested() -> bool {
    std::env::var_os("JACKDAW_PIE_WINDOWLESS").is_some()
        && std::env::var_os("JACKDAW_PIE").is_some()
}

/// Added by [`maybe_windowless`] when it reconfigures the app for a
/// windowless launch. Its presence is the proof the app opted in: the PIE
/// link checks it before installing the virtual window, camera retarget,
/// and picking backend, so a headless process that shares the launch
/// environment (the dedicated server) is never misconfigured by the env
/// vars alone.
pub struct WindowlessActive;

impl Plugin for WindowlessActive {
    fn build(&self, _app: &mut App) {}
}

/// True when this app was reconfigured by [`maybe_windowless`].
pub(crate) fn windowless_active(app: &App) -> bool {
    app.is_plugin_added::<WindowlessActive>()
}

/// Wrap the game's plugin group for play-in-editor. When the editor asked
/// for a windowless launch this disables winit, drops the primary window,
/// and adds a schedule-runner loop at about 60 updates per second; otherwise
/// the group is returned unchanged.
pub fn maybe_windowless(plugins: impl PluginGroup) -> PluginGroupBuilder {
    let builder = plugins.build();
    if !windowless_requested() {
        return builder;
    }
    builder
        .disable::<bevy::winit::WinitPlugin>()
        .set(WindowPlugin {
            primary_window: None,
            exit_condition: ExitCondition::DontExit,
            close_when_requested: false,
            ..Default::default()
        })
        .add(bevy::app::ScheduleRunnerPlugin::run_loop(
            std::time::Duration::from_secs_f64(1.0 / 60.0),
        ))
        .add(WindowlessActive)
}

/// Marker for the data-only window standing in for a real one.
#[derive(Component)]
pub(crate) struct PieVirtualWindow;

/// The capture image every game camera renders into while windowless. The
/// handle is stable for the life of the process; resizes replace the asset
/// in place so camera targets and the readback stay valid.
#[derive(Resource)]
pub(crate) struct WindowlessTarget {
    pub image: Handle<Image>,
    pub size: UVec2,
}

/// Build the render-target image for the given size.
pub(crate) fn make_target_image(size: UVec2) -> Image {
    let mut image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    image
}

/// Install the windowless world pieces: the capture image and the virtual
/// primary window. Runs once at startup, before anything queries windows.
pub(crate) fn install_windowless_world(world: &mut World) {
    let image = world
        .resource_mut::<Assets<Image>>()
        .add(make_target_image(DEFAULT_SIZE));
    world.insert_resource(WindowlessTarget {
        image,
        size: DEFAULT_SIZE,
    });
    let size = world.resource::<WindowlessTarget>().size;
    let mut window = Window::default();
    window.resolution.set_physical_resolution(size.x, size.y);
    world.spawn((
        window,
        PrimaryWindow,
        CursorOptions::default(),
        PieVirtualWindow,
    ));
}

/// Point every window-targeting camera at the capture image. Because no
/// camera renders to a window anymore, bevy_ui has no default UI camera, so
/// exactly one retargeted camera is marked as the default; the marker is a
/// singleton, so the rest are left unmarked. A game that already set its own
/// default UI camera keeps it. Runs every frame: cameras spawned later (zone
/// changes, menu cameras) are caught the frame they appear, one skipped
/// render at worst, and a despawned default is replaced the next frame.
pub(crate) fn retarget_cameras(
    target: Res<WindowlessTarget>,
    default_ui: Query<(), With<bevy::ui::IsDefaultUiCamera>>,
    mut cameras: Query<(Entity, &mut RenderTarget), With<Camera>>,
    mut commands: Commands,
) {
    let mut have_default_ui = !default_ui.is_empty();
    for (entity, mut render_target) in &mut cameras {
        if matches!(*render_target, RenderTarget::Window(_)) {
            *render_target = RenderTarget::Image(target.image.clone().into());
            if !have_default_ui {
                commands
                    .entity(entity)
                    .try_insert(bevy::ui::IsDefaultUiCamera);
                have_default_ui = true;
            }
        }
    }
}

/// Wire the windowless systems into the app. Called from `attach_pie` when
/// the editor requested a windowless launch.
pub(crate) fn setup_windowless(app: &mut App) {
    app.add_systems(PreStartup, install_windowless_world);
    app.add_systems(PostUpdate, retarget_cameras);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::camera::{ImageRenderTarget, RenderTarget};
    use bevy::window::WindowRef;

    fn world_with_target() -> (World, Handle<Image>) {
        let mut world = World::new();
        world.insert_resource(Assets::<Image>::default());
        let handle = world
            .resource_mut::<Assets<Image>>()
            .add(make_target_image(DEFAULT_SIZE));
        world.insert_resource(WindowlessTarget {
            image: handle.clone(),
            size: DEFAULT_SIZE,
        });
        (world, handle)
    }

    #[test]
    fn retarget_rewrites_window_cameras_and_marks_ui_default() {
        let (mut world, handle) = world_with_target();
        let cam = world
            .spawn((Camera::default(), RenderTarget::Window(WindowRef::Primary)))
            .id();
        world.run_system_cached(retarget_cameras).unwrap();
        match world.get::<RenderTarget>(cam).unwrap() {
            RenderTarget::Image(target) => assert_eq!(target.handle, handle),
            other => panic!("expected image target, got {other:?}"),
        }
        assert!(world.entity(cam).contains::<bevy::ui::IsDefaultUiCamera>());
    }

    #[test]
    fn retarget_marks_only_one_ui_default_with_multiple_window_cameras() {
        let (mut world, _handle) = world_with_target();
        world.spawn((Camera::default(), RenderTarget::Window(WindowRef::Primary)));
        world.spawn((Camera::default(), RenderTarget::Window(WindowRef::Primary)));
        world.run_system_cached(retarget_cameras).unwrap();
        let count = world
            .query_filtered::<(), With<bevy::ui::IsDefaultUiCamera>>()
            .iter(&world)
            .count();
        assert_eq!(
            count, 1,
            "exactly one camera marked as the default UI camera"
        );
    }

    #[test]
    fn retarget_leaves_image_cameras_alone_and_catches_late_spawns() {
        let (mut world, handle) = world_with_target();
        let other = world
            .resource_mut::<Assets<Image>>()
            .add(make_target_image(DEFAULT_SIZE));
        let image_cam = world
            .spawn((
                Camera::default(),
                RenderTarget::Image(ImageRenderTarget::from(other.clone())),
            ))
            .id();
        world.run_system_cached(retarget_cameras).unwrap();
        match world.get::<RenderTarget>(image_cam).unwrap() {
            RenderTarget::Image(target) => assert_eq!(target.handle, other),
            other => panic!("expected the original image target, got {other:?}"),
        }
        let late = world
            .spawn((Camera::default(), RenderTarget::Window(WindowRef::Primary)))
            .id();
        world.run_system_cached(retarget_cameras).unwrap();
        match world.get::<RenderTarget>(late).unwrap() {
            RenderTarget::Image(target) => assert_eq!(target.handle, handle),
            other => panic!("expected image target, got {other:?}"),
        }
    }

    #[test]
    fn windowless_active_requires_the_marker() {
        let app = bevy::app::App::new();
        assert!(!windowless_active(&app));
        let mut app = bevy::app::App::new();
        app.add_plugins(WindowlessActive);
        assert!(windowless_active(&app));
    }

    #[test]
    fn install_spawns_a_virtual_primary_window_at_default_size() {
        let mut app = bevy::app::App::new();
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Image>();
        install_windowless_world(app.world_mut());
        let mut windows = app
            .world_mut()
            .query_filtered::<&Window, (With<PieVirtualWindow>, With<bevy::window::PrimaryWindow>)>(
            );
        let window = windows.single(app.world()).expect("virtual window spawned");
        assert_eq!(window.physical_width(), DEFAULT_SIZE.x);
        assert_eq!(window.physical_height(), DEFAULT_SIZE.y);
        let target = app.world().resource::<WindowlessTarget>();
        assert!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&target.image)
                .is_some()
        );
    }
}
