//! An editor with the Navmesh module loaded, as a person's editor runs it.

use bevy::prelude::*;
use bevy::render::{
    RenderPlugin,
    settings::{RenderCreation, WgpuSettings},
};
use bevy::winit::WinitPlugin;
use jackdaw::prelude::*;
use jackdaw_api::op::OperatorResult;
use jackdaw_api_internal::lifecycle::{ExtensionAppExt as _, enable_extension};
use jackdaw_navmesh_editor::{EXTENSION_ID, NavmeshExtension};

/// Point the editor's per-user config at a scratch directory of this test
/// process before anything reads it.
fn isolate_config_dir() {
    static ISOLATED: std::sync::Once = std::sync::Once::new();
    ISOLATED.call_once(|| {
        let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("jackdaw-navmesh-config-{}", std::process::id()));
        // SAFETY: runs once, before the first plugin is added, so no other
        // thread reads the environment while it is set.
        unsafe { std::env::set_var(jackdaw_env::paths::CONFIG_DIR_VAR, dir) };
    });
}

/// A headless editor with the Navmesh module enabled, past its first frame.
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
    app.register_extension::<NavmeshExtension>();
    app.finish();
    app.update();
    enable_extension(app.world_mut(), EXTENSION_ID);
    app.world_mut()
        .resource_mut::<NextState<AppState>>()
        .set(AppState::Editor);
    app.update();
    app
}

/// Run frames until queued work has landed.
pub fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

/// Call operator `id` and let what it queued land.
pub fn call(app: &mut App, id: &str) {
    let result = app
        .world_mut()
        .operator(id.to_string())
        .call()
        .unwrap_or_else(|err| panic!("{id}: {err}"));
    assert_eq!(result, OperatorResult::Finished, "{id} reported {result:?}");
    settle(app);
}

/// A new terrain, selected.
pub fn selected_terrain(app: &mut App) -> Entity {
    call(app, "entity.add.terrain");
    let terrain = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw_scene_types::Terrain>>()
        .single(app.world())
        .expect("one terrain");
    app.world_mut()
        .resource_mut::<jackdaw_api::selection::Selection>()
        .entities = vec![terrain];
    settle(app);
    terrain
}
