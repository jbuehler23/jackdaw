//! An editor with the Physics module loaded, as a person's editor runs it.

use bevy::prelude::*;
use bevy::render::{
    RenderPlugin,
    settings::{RenderCreation, WgpuSettings},
};
use bevy::window::{PrimaryWindow, WindowResolution};
use bevy::winit::WinitPlugin;
use jackdaw::prelude::*;
use jackdaw::test_input::SyntheticInput;
use jackdaw_api::prelude::OperatorResult;
use jackdaw_api_internal::lifecycle::{ExtensionAppExt as _, enable_extension};
use jackdaw_physics::{EXTENSION_ID, PhysicsExtension};

/// Point the editor's per-user config at a scratch directory of this test
/// process before anything reads it.
fn isolate_config_dir() {
    static ISOLATED: std::sync::Once = std::sync::Once::new();
    ISOLATED.call_once(|| {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("jackdaw-physics-config-{}", std::process::id()));
        // SAFETY: runs once, before the first plugin is added, so no other
        // thread reads the environment while it is set.
        unsafe { std::env::set_var(jackdaw_env::paths::CONFIG_DIR_VAR, dir) };
    });
}

/// A headless editor with the Physics module enabled, past its first frame.
pub fn editor_app() -> App {
    jackdaw_project_build::bootstrap::skip_setup_check();
    isolate_config_dir();
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
                    backends: None,
                    ..default()
                })),
                ..default()
            })
            .disable::<bevy::audio::AudioPlugin>()
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(JackdawEditorPlugins::default());
    app.add_plugins(bevy::extract::sync_world::SyncWorldPlugin::<
        bevy::render::RenderApp,
    >::default());
    app.register_extension::<PhysicsExtension>();
    app.finish();
    app.update();
    enable_extension(app.world_mut(), EXTENSION_ID);
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Editor);
    app.update();
    app
}

/// An editor with the Physics module in a window of `size`, laid out so
/// pointer input lands on its toolbar and viewport.
pub fn editor_with_viewport(size: (u32, u32)) -> App {
    let mut app = editor_app();
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows
            .single_mut(app.world_mut())
            .expect("headless apps still have a primary window");
        window.resolution = WindowResolution::new(size.0, size.1);
    }
    settle(&mut app);
    app
}

/// Run frames until queued work has landed.
pub fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

/// Dispatch one boot-op clause, such as `input.pointer x=1 y=2 action=click`,
/// and play out the input it queues.
pub fn run(app: &mut App, clause: &str) {
    let result = jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .unwrap_or_else(|error| panic!("{clause}: {error}"));
    assert_ne!(result, OperatorResult::Cancelled, "{clause} was refused");
    for _ in 0..200 {
        app.update();
        if app.world().resource::<SyntheticInput>().is_idle() {
            break;
        }
    }
    settle(app);
}

/// Where a UI `entity` is drawn, in the window logical pixels
/// `input.pointer` takes.
pub fn centre_of(app: &App, entity: Entity) -> Vec2 {
    let transform = app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(entity)
        .expect("the node is placed");
    let computed = app
        .world()
        .get::<ComputedNode>(entity)
        .expect("the node is laid out");
    transform.translation * computed.inverse_scale_factor() * app.world().resource::<UiScale>().0
}
