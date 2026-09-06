//! What an Outliner row costs while nobody is looking at it. Rows are built when
//! a branch is opened and torn down when it closes, so a panel costs what it
//! draws rather than everything the session has ever opened.

use crate::util;

use bevy::prelude::*;
use jackdaw::hierarchy::{HierarchyShowAll, HierarchyTreeContainer};
use jackdaw_widgets::tree_view::{
    TreeFocused, TreeIndex, TreeNode, TreeNodeExpanded, TreeRowChildren, TreeRowContent,
    TreeRowSelected,
};

/// An Outliner panel over a scene with one branch two levels deep. Returns the
/// panel, the branch root, its children and the one grandchild.
fn panel_over_a_branch(app: &mut App) -> (Entity, Entity, Vec<Entity>, Entity) {
    app.world_mut().insert_resource(HierarchyShowAll(true));
    let panel = app
        .world_mut()
        .spawn((
            HierarchyTreeContainer,
            Node::default(),
            Visibility::Inherited,
        ))
        .id();
    app.update();

    let world = app.world_mut();
    let branch = world.spawn((Name::new("Branch"), Node::default())).id();
    jackdaw::scene_io::register_entity_in_ast(world, branch);
    let children: Vec<Entity> = ["First", "Second", "Third"]
        .into_iter()
        .map(|name| {
            let child = world
                .spawn((Name::new(name), Node::default(), ChildOf(branch)))
                .id();
            jackdaw::scene_io::register_entity_in_ast(world, child);
            child
        })
        .collect();
    let grandchild = world
        .spawn((Name::new("Leaf"), Node::default(), ChildOf(children[0])))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, grandchild);
    app.update();
    (panel, branch, children, grandchild)
}

/// Open `source`'s row in `panel` and let the rows land.
fn expand(app: &mut App, panel: Entity, source: Entity) {
    let row = row_for(app, panel, source);
    app.world_mut()
        .entity_mut(row)
        .insert(TreeNodeExpanded(true));
    app.update();
    app.update();
}

/// Close `source`'s row in `panel` and let the rows go.
fn collapse(app: &mut App, panel: Entity, source: Entity) {
    let row = row_for(app, panel, source);
    app.world_mut()
        .entity_mut(row)
        .insert(TreeNodeExpanded(false));
    app.update();
    app.update();
}

#[track_caller]
fn row_for(app: &App, panel: Entity, source: Entity) -> Entity {
    app.world()
        .resource::<TreeIndex>()
        .get(panel, source)
        .unwrap_or_else(|| panic!("{source} has no row in {panel}"))
}

/// The names of the rows drawn under `source`'s row, in panel order.
fn child_row_names(world: &World, panel: Entity, source: Entity) -> Vec<String> {
    let Some(row) = world.resource::<TreeIndex>().get(panel, source) else {
        return Vec::new();
    };
    let Some(container) = world.get::<Children>(row).and_then(|children| {
        children
            .iter()
            .find(|&child| world.get::<TreeRowChildren>(child).is_some())
    }) else {
        return Vec::new();
    };
    world
        .get::<Children>(container)
        .map(|children| {
            children
                .iter()
                .filter_map(|child| world.get::<TreeNode>(child))
                .filter_map(|node| world.get::<Name>(node.0))
                .map(|name| name.as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn closing_a_branch_frees_the_rows_under_it() {
    let mut app = util::editor_test_app();
    let (panel, branch, children, _leaf) = panel_over_a_branch(&mut app);

    expand(&mut app, panel, branch);
    let rows: Vec<Entity> = children
        .iter()
        .map(|&child| row_for(&app, panel, child))
        .collect();
    assert_eq!(
        child_row_names(app.world(), panel, branch),
        vec!["First", "Second", "Third"],
        "the fixture opens with a row per child"
    );

    collapse(&mut app, panel, branch);

    for row in rows {
        assert!(
            app.world().get_entity(row).is_err(),
            "{row} is still in the world after its branch closed"
        );
    }
    for child in &children {
        assert!(
            app.world()
                .resource::<TreeIndex>()
                .get(panel, *child)
                .is_none(),
            "the index still claims {child} has a row"
        );
    }
}

/// The freed subtree is the whole branch: an index entry left behind is a row
/// that can never be built again.
#[test]
fn closing_a_branch_frees_the_rows_nested_below_it_too() {
    let mut app = util::editor_test_app();
    let (panel, branch, children, leaf) = panel_over_a_branch(&mut app);

    expand(&mut app, panel, branch);
    expand(&mut app, panel, children[0]);
    let leaf_row = row_for(&app, panel, leaf);

    collapse(&mut app, panel, branch);

    assert!(
        app.world().get_entity(leaf_row).is_err(),
        "the grandchild's row outlived its branch"
    );
    assert!(
        app.world()
            .resource::<TreeIndex>()
            .get(panel, leaf)
            .is_none(),
        "the index still claims the grandchild has a row"
    );
}

#[test]
fn reopening_a_branch_builds_its_rows_again() {
    let mut app = util::editor_test_app();
    let (panel, branch, _children, _leaf) = panel_over_a_branch(&mut app);

    expand(&mut app, panel, branch);
    collapse(&mut app, panel, branch);
    expand(&mut app, panel, branch);

    assert_eq!(
        child_row_names(app.world(), panel, branch),
        vec!["First", "Second", "Third"],
        "a reopened branch draws its children in the order the document holds"
    );
}

/// A row is built when its branch is opened, which can be long after its entity
/// was selected, and a row that comes back unmarked is a selection the panel
/// disagrees with the rest of the editor about.
#[test]
fn a_selected_entity_gets_a_marked_row_when_its_branch_is_reopened() {
    let mut app = util::editor_test_app();
    let (panel, branch, children, _leaf) = panel_over_a_branch(&mut app);

    expand(&mut app, panel, branch);
    jackdaw::selection::select_only(app.world_mut(), children[1]);
    app.update();
    collapse(&mut app, panel, branch);
    expand(&mut app, panel, branch);

    let row = row_for(&app, panel, children[1]);
    let content = app
        .world()
        .get::<Children>(row)
        .and_then(|kids| {
            kids.iter()
                .find(|&kid| app.world().get::<TreeRowContent>(kid).is_some())
        })
        .expect("a row has content");
    assert!(
        app.world().get::<TreeRowSelected>(content).is_some(),
        "the reopened row does not know its entity is selected"
    );
}

/// The keyboard walks the rows that are drawn, so closing a branch it was inside
/// has to resume the walk on the row that was closed.
#[test]
fn the_keyboard_walk_resumes_on_the_row_that_was_closed() {
    let mut app = util::editor_test_app();
    let (panel, branch, children, _leaf) = panel_over_a_branch(&mut app);

    expand(&mut app, panel, branch);
    let child_row = row_for(&app, panel, children[2]);
    app.world_mut().resource_mut::<TreeFocused>().0 = Some(child_row);

    collapse(&mut app, panel, branch);

    let focused = app
        .world()
        .resource::<TreeFocused>()
        .0
        .expect("the walk still has a row");
    assert!(
        app.world().get_entity(focused).is_ok(),
        "the walk is standing on a despawned row"
    );
    assert_eq!(
        app.world().get::<TreeNode>(focused).map(|node| node.0),
        Some(branch),
        "the walk resumes on the row that was closed"
    );
}
