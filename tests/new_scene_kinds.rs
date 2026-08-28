//! Making a UI scene: what `scene.new ui=true` puts in the document, what
//! it names it, and where it leaves the workspace.
//!
//! Three contracts, all of them things a human authoring a screen through
//! the GUI trips over on the first try:
//!
//! 1. A UI scene seeds UI defaults and nothing else. A directional light
//!    is 3D furniture; seeded into a UI document it is saved to disk as
//!    part of a screen that has nothing to light.
//! 2. The seeded root is named without a space, so an operator clause --
//!    which has no quoting -- can address it as `name=UiRoot`.
//! 3. Creating a UI scene brings the 2D viewport forward, the way opening
//!    one already does. The canvas is the whole point of the scene.

use bevy::prelude::*;
use jackdaw::scenes::operators::scene_new_configured;
use jackdaw::selection::Selection;
use jackdaw::ui_palette::UI_SCENE_ROOT_NAME;
use jackdaw::viewport_2d::VIEWPORT_2D_WINDOW_ID;
use jackdaw_panels::{
    area::DockAreaStyle,
    tree::{DockLeaf, DockTree, NodeId},
};
use jackdaw_scene_types::UiSceneRoot;

mod util;

/// Every name in the live scene document, in no particular order.
fn scene_names(app: &mut App) -> Vec<String> {
    let mut query = app
        .world_mut()
        .query_filtered::<&Name, With<jackdaw_scene_types::SceneNodeId>>();
    query
        .iter(app.world())
        .map(|name| name.as_str().to_string())
        .collect()
}

/// A dock whose one leaf holds `windows`, with the first in front, so a
/// focus change reads as a change rather than as the starting state.
fn dock_leaf(app: &mut App, windows: &[&str]) -> NodeId {
    app.init_resource::<DockTree>();
    let mut tree = app.world_mut().resource_mut::<DockTree>();
    tree.set_root_leaf(
        DockLeaf::new("center", DockAreaStyle::default())
            .with_windows(windows.iter().copied().map(String::from).collect()),
    )
}

/// The window whose tab is in front of `leaf`.
fn active_window(app: &App, leaf: NodeId) -> Option<String> {
    let tree = app.world().resource::<DockTree>();
    let leaf = tree.get(leaf)?.as_leaf()?;
    let active = leaf.active?;
    leaf.tabs()
        .find_map(|(window, tab)| (tab == active).then(|| window.to_string()))
}

/// Gap 2. The light is what an empty 3D scene needs to be visible at all;
/// a UI document has nothing to light, and whatever is in the document at
/// the end of this is what a save writes.
#[test]
fn a_new_ui_scene_seeds_the_ui_root_and_nothing_else() {
    let mut app = util::editor_test_app();

    scene_new_configured(app.world_mut(), true, None);
    app.update();

    let names = scene_names(&mut app);
    assert_eq!(
        names,
        vec![UI_SCENE_ROOT_NAME.to_string()],
        "a UI scene starts with its root alone: {names:?}",
    );

    let mut lights = app.world_mut().query::<&DirectionalLight>();
    assert_eq!(
        lights.iter(app.world()).count(),
        0,
        "no light is seeded into a document that has nothing to light",
    );
}

/// The other half of the same branch: an ordinary scene still gets the
/// light it has always got, and no UI root.
#[test]
fn a_new_3d_scene_still_seeds_its_light_and_no_ui_root() {
    let mut app = util::editor_test_app();

    scene_new_configured(app.world_mut(), false, None);
    app.update();

    let mut lights = app.world_mut().query::<&DirectionalLight>();
    assert_eq!(
        lights.iter(app.world()).count(),
        1,
        "an empty 3D scene is black without it",
    );
    let mut roots = app.world_mut().query::<&UiSceneRoot>();
    assert_eq!(
        roots.iter(app.world()).count(),
        0,
        "and a 3D scene is not a UI scene",
    );
}

/// Gap 3. An operator clause is text with no quoting, so a value cannot
/// carry a space; a root called `UI Root` is unreachable from every
/// scripted path the editor has.
#[test]
fn the_seeded_root_is_named_so_a_clause_can_address_it() {
    let mut app = util::editor_test_app();

    scene_new_configured(app.world_mut(), true, None);
    app.update();

    assert_eq!(UI_SCENE_ROOT_NAME, "UiRoot");
    assert!(
        !UI_SCENE_ROOT_NAME.contains(' '),
        "a clause value cannot contain a space, so neither can this name",
    );

    let mut query = app.world_mut().query::<(Entity, &Name, &UiSceneRoot)>();
    let (root, name, _) = query
        .iter(app.world())
        .next()
        .map(|(entity, name, root)| (entity, name.as_str().to_string(), root))
        .expect("the UI scene has a root");
    assert_eq!(name, UI_SCENE_ROOT_NAME);
    assert_eq!(
        app.world().resource::<Selection>().primary(),
        Some(root),
        "and it is selected, so the first Add lands inside it",
    );
}

/// Gap 11. Making a UI scene is exactly the moment the canvas is wanted.
/// The load and tab-swap paths already front the panel; creation did not.
#[test]
fn creating_a_ui_scene_brings_the_2d_viewport_forward() {
    let mut app = util::editor_test_app();
    let leaf = dock_leaf(&mut app, &["jackdaw.outliner", VIEWPORT_2D_WINDOW_ID]);
    app.update();
    assert_eq!(
        active_window(&app, leaf).as_deref(),
        Some("jackdaw.outliner"),
        "the fixture starts with the 2D viewport behind another tab",
    );

    scene_new_configured(app.world_mut(), true, None);
    app.update();

    assert_eq!(
        active_window(&app, leaf).as_deref(),
        Some(VIEWPORT_2D_WINDOW_ID),
        "a new UI scene fronts the panel it is authored in",
    );
}

/// And only for a UI scene: a new 3D scene leaves the workspace as the
/// user arranged it.
#[test]
fn creating_a_3d_scene_leaves_the_fronted_tab_alone() {
    let mut app = util::editor_test_app();
    let leaf = dock_leaf(&mut app, &["jackdaw.outliner", VIEWPORT_2D_WINDOW_ID]);
    app.update();

    scene_new_configured(app.world_mut(), false, None);
    app.update();

    assert_eq!(
        active_window(&app, leaf).as_deref(),
        Some("jackdaw.outliner"),
        "nothing about a 3D scene asks for the 2D canvas",
    );
}
