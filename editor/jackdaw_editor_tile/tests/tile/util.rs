use bevy::prelude::*;

pub use jackdaw_editor_core::testing::{
    OVERWORLD_SHEET, OperatorResultExt, fixed_frame_clock, update_until,
    write_spaced_overworld_sheet,
};

fn isolate_config_dir() {
    jackdaw_editor_core::testing::isolate_config_dir(
        std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("jackdaw-config-{}", std::process::id())),
    );
}

fn add_tile_editor(app: &mut App) {
    app.add_plugins(jackdaw_editor_tile::TileEditorPlugin);
}

/// The editor with the tile editor, past its startup frame.
pub fn editor_test_app() -> App {
    isolate_config_dir();
    jackdaw_editor_core::testing::editor_test_app_with(None, add_tile_editor)
}

/// Like [`editor_test_app`], with the asset server reading `assets`.
pub fn editor_test_app_reading(assets: &std::path::Path) -> App {
    isolate_config_dir();
    jackdaw_editor_core::testing::editor_test_app_with(Some(assets), add_tile_editor)
}
