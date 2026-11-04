use bevy::prelude::*;

pub use jackdaw_editor_core::testing::fixed_frame_clock;

/// The repository root, whose `assets/` holds the animated glTF files these
/// tests index.
pub fn workspace_root() -> std::path::PathBuf {
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

fn isolate_config_dir() {
    jackdaw_editor_core::testing::isolate_config_dir(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("jackdaw-config-{}", std::process::id())),
    );
}

/// The editor with the animation editor, past its startup frame.
pub fn editor_test_app() -> App {
    run_from_workspace_root();
    isolate_config_dir();
    jackdaw_editor_core::project::set_open_project_assets_dir(None);
    let assets = workspace_root().join("assets");
    jackdaw_editor_core::testing::editor_test_app_with(Some(&assets), |app| {
        app.add_plugins(jackdaw_editor_animation::AnimationEditorPlugin);
    })
}
