use bevy::prelude::*;

pub use jackdaw_editor_core::testing::update_until;

/// The repository root, whose `assets/` the editor's own tests read.
fn workspace_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Run from the workspace root, as the editor's own tests do. Done once,
/// before the first app is built.
fn run_from_workspace_root() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        std::env::set_current_dir(workspace_root()).expect("the workspace root exists");
    });
}

/// The editor with the launcher, past its startup frame.
pub fn editor_test_app() -> App {
    run_from_workspace_root();
    let assets = workspace_root().join("assets");
    jackdaw_editor_core::testing::editor_test_app_with(Some(&assets), |app| {
        app.add_plugins(jackdaw_editor_launcher::LauncherEditorPlugin);
    })
}
