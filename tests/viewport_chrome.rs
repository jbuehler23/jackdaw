//! The floating chrome a 3D viewport panel carries: the terrain tool palette,
//! the terrain options bar, and the main toolbar.
//!
//! Each is spawned by `build_viewport_panel` rather than authored into the dock
//! tree, so nothing but that function decides how many of each a panel gets.
//! These tests pin the count at one per panel, and pin it across tab switches:
//! a swap tears the scene down and spawns another, and the chrome belongs to
//! the panel rather than to the scene, so it has to come through untouched.

use bevy::prelude::*;
use jackdaw::layout::Toolbar;
use jackdaw::terrain::{TerrainOptionsBar, TerrainPalette};
use jackdaw::viewport::build_viewport_panel;
use jackdaw_feathers::button::ButtonOperatorCall;

mod util;

/// Operator ids the terrain palette's buttons carry, one per entry.
const PALETTE_OP_IDS: [&str; 9] = [
    "terrain.tool.raise",
    "terrain.tool.lower",
    "terrain.tool.flatten",
    "terrain.tool.smooth",
    "terrain.tool.noise",
    "terrain.tool.paint",
    "terrain.tool.quantize",
    "terrain.tool.navmesh",
    "terrain.tool.regions",
];

fn count<C: Component>(app: &mut App) -> usize {
    app.world_mut().query::<&C>().iter(app.world()).count()
}

/// How many buttons in the world call the palette's raise operator. One per
/// palette, so this is the palette count read off its contents rather than off
/// its root marker.
fn raise_buttons(app: &mut App) -> usize {
    app.world_mut()
        .query::<&ButtonOperatorCall>()
        .iter(app.world())
        .filter(|call| call.id.as_ref() == "terrain.tool.raise")
        .count()
}

/// A panel with two scene tabs open, ready to swap between.
fn app_with_panel_and_two_tabs() -> App {
    use jackdaw::scenes::{SceneTab, Scenes};

    let mut app = util::editor_test_app();
    let parent = app.world_mut().spawn(Node::default()).id();
    build_viewport_panel(app.world_mut(), parent);

    {
        let mut scenes = app.world_mut().resource_mut::<Scenes>();
        scenes.tabs.clear();
        scenes.tabs.push(SceneTab::new_untitled(1));
        scenes.tabs.push(SceneTab::new_untitled(2));
        scenes.active = 0;
    }
    app.update();
    app
}

/// A panel carries exactly one terrain tool palette, holding exactly one
/// button per entry.
#[test]
fn a_viewport_panel_builds_one_terrain_palette() {
    let mut app = util::editor_test_app();
    let parent = app.world_mut().spawn(Node::default()).id();
    build_viewport_panel(app.world_mut(), parent);
    app.update();

    assert_eq!(
        count::<TerrainPalette>(&mut app),
        1,
        "one panel, one palette",
    );

    for op_id in PALETTE_OP_IDS {
        let buttons = app
            .world_mut()
            .query::<&ButtonOperatorCall>()
            .iter(app.world())
            .filter(|call| call.id.as_ref() == op_id)
            .count();
        assert_eq!(buttons, 1, "the palette holds one {op_id} button");
    }
}

/// The rest of the panel's floating chrome is single too. Same class of bug as
/// a doubled palette, so it is pinned in the same place.
#[test]
fn a_viewport_panel_builds_one_options_bar_and_one_toolbar() {
    let mut app = util::editor_test_app();
    let parent = app.world_mut().spawn(Node::default()).id();
    build_viewport_panel(app.world_mut(), parent);
    app.update();

    assert_eq!(count::<TerrainOptionsBar>(&mut app), 1, "one options bar");
    assert_eq!(count::<Toolbar>(&mut app), 1, "one main toolbar");
}

/// Swapping back and forth between two scenes leaves the palette alone. The
/// chrome belongs to the panel, and a swap replaces the scene under it.
#[test]
fn swapping_tabs_does_not_multiply_the_terrain_palette() {
    use jackdaw::scenes::swap::swap_active_tab;

    let mut app = app_with_panel_and_two_tabs();

    let palettes = count::<TerrainPalette>(&mut app);
    let buttons = raise_buttons(&mut app);
    assert_eq!(palettes, 1, "the panel starts with one palette");
    assert_eq!(buttons, 1, "and one button per entry");

    for target in [1, 0, 1, 0] {
        swap_active_tab(app.world_mut(), target);
        app.update();

        assert_eq!(
            count::<TerrainPalette>(&mut app),
            palettes,
            "swapping to tab {target} must not add a palette",
        );
        assert_eq!(
            raise_buttons(&mut app),
            buttons,
            "swapping to tab {target} must not add palette buttons",
        );
    }
}

/// The same for the bar and the toolbar.
#[test]
fn swapping_tabs_does_not_multiply_the_options_bar_or_toolbar() {
    use jackdaw::scenes::swap::swap_active_tab;

    let mut app = app_with_panel_and_two_tabs();

    let bars = count::<TerrainOptionsBar>(&mut app);
    let toolbars = count::<Toolbar>(&mut app);
    assert_eq!(bars, 1);
    assert_eq!(toolbars, 1);

    for target in [1, 0, 1, 0] {
        swap_active_tab(app.world_mut(), target);
        app.update();

        assert_eq!(
            count::<TerrainOptionsBar>(&mut app),
            bars,
            "swapping to tab {target} must not add an options bar",
        );
        assert_eq!(
            count::<Toolbar>(&mut app),
            toolbars,
            "swapping to tab {target} must not add a toolbar",
        );
    }
}
