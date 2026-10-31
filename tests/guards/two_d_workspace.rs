//! The 2D workspace is registered with the viewport, the Outliner, the
//! Inspector, the Tile Palette and the Project window docked.

use crate::util;

use jackdaw_panels::WorkspaceRegistry;

#[test]
fn the_2d_workspace_docks_the_tile_palette() {
    let mut app = util::editor_test_app();
    for _ in 0..3 {
        app.update();
    }
    let workspace = app
        .world()
        .resource::<WorkspaceRegistry>()
        .get("2d")
        .expect("the 2D workspace is registered");
    assert_eq!(workspace.name, "2D");
    let windows: Vec<String> = workspace
        .tree
        .leaves()
        .flat_map(|(_, leaf)| leaf.tabs().map(|(window, _)| window.to_string()))
        .collect();
    for id in [
        jackdaw::viewport::VIEWPORT_WINDOW_ID,
        "jackdaw.hierarchy",
        "jackdaw.inspector.components",
        jackdaw::tile_palette::TILE_PALETTE_WINDOW_ID,
        jackdaw::project_window::PROJECT_WINDOW_ID,
    ] {
        assert_eq!(
            windows.iter().filter(|w| *w == id).count(),
            1,
            "{id} is docked once: {windows:?}"
        );
    }
    let registry = app.world().resource::<jackdaw_panels::WindowRegistry>();
    for id in &windows {
        assert!(registry.get(id).is_some(), "{id} is a registered window");
    }
}
