use bevy::prelude::*;

pub use jackdaw_editor_core::testing::OperatorResultExt;

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

fn prepare() {
    run_from_workspace_root();
    isolate_config_dir();
    // A test that opened a project leaves its assets folder as the open one
    // after its app is gone; a new app starts with no project open.
    jackdaw_editor_core::project::set_open_project_assets_dir(None);
}

/// The editor with the modelling tools, before its startup frame.
pub fn headless_app() -> App {
    prepare();
    let mut app = jackdaw_editor_core::testing::ambient_app(bevy::asset::AssetPlugin::default());
    jackdaw_editor_core::testing::add_editor_plugins(&mut app);
    app.add_plugins(jackdaw_editor_modelling::ModellingEditorPlugin);
    app
}

/// The editor with the modelling tools, past its startup frame.
pub fn editor_test_app() -> App {
    prepare();
    jackdaw_editor_core::testing::editor_test_app_with(None, |app| {
        app.add_plugins(jackdaw_editor_modelling::ModellingEditorPlugin);
    })
}
