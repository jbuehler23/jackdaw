use bevy::prelude::*;

/// Run from the workspace root, as the editor's own tests do, so `assets/`
/// names the repository's assets. Done once, before the first app is built.
fn run_from_workspace_root() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        std::env::set_current_dir(root).expect("the workspace root exists");
    });
}

fn isolate_config_dir() {
    jackdaw_editor_core::testing::isolate_config_dir(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("jackdaw-config-{}", std::process::id())),
    );
}

fn add_terrain_editor(app: &mut App) {
    app.add_plugins(jackdaw_editor_terrain::TerrainEditorPlugin);
}

/// The editor with the terrain editor, past its startup frame.
pub fn editor_test_app() -> App {
    run_from_workspace_root();
    isolate_config_dir();
    // A test that opened a project leaves its assets folder as the open one
    // after its app is gone; a new app starts with no project open.
    jackdaw_editor_core::project::set_open_project_assets_dir(None);
    jackdaw_editor_core::testing::editor_test_app_with(None, add_terrain_editor)
}
