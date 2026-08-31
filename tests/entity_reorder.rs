//! Sibling reorder: the gap between two Outliner rows, and the
//! `entity.move_up` / `entity.move_down` chords.
//!
//! What is pinned here:
//!  * a reorder writes the document's child order, not only the ECS one, so
//!    it survives a save and a reload;
//!  * the ECS order moves with it, which is what makes a flowed child change
//!    place on the canvas;
//!  * every Outliner panel's rows follow;
//!  * one reorder is one undo entry, and undo puts the order back.

use bevy::prelude::*;
use jackdaw::boot_ops::run_op_clause_as_user;
use jackdaw::commands::CommandHistory;
use jackdaw::hierarchy::{HierarchyShowAll, HierarchyTreeContainer};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_bsn::SceneBsnAst;
use jackdaw_widgets::tree_view::{TreeIndex, TreeNode, TreeRowChildren, TreeRowInserted};

mod util;

/// Run one clause the way a chord runs it.
///
/// `creates_history_entry`, which a scripted call leaves off, is what makes
/// the dispatcher open a snapshot span: an operator that records its own entry
/// and one that leaves the entry to the snapshot are only told apart under a
/// press, and this suite counts entries.
#[track_caller]
fn run_finished(app: &mut App, clause: &str) {
    let result = run_op_clause_as_user(app.world_mut(), clause)
        .unwrap_or_else(|err| panic!("{clause}: dispatch errored: {err}"));
    app.update();
    assert_eq!(
        result,
        OperatorResult::Finished,
        "{clause} reported {result:?}"
    );
}

/// One column with three named children, registered in the document
/// parent-first the way a load leaves it.
fn column_of_three(app: &mut App) -> (Entity, Vec<Entity>) {
    let world = app.world_mut();
    let column = world
        .spawn((
            Name::new("Column"),
            Node {
                flex_direction: FlexDirection::Column,
                ..default()
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, column);
    let children: Vec<Entity> = ["First", "Second", "Third"]
        .into_iter()
        .map(|name| {
            let child = world
                .spawn((Name::new(name), Node::default(), ChildOf(column)))
                .id();
            jackdaw::scene_io::register_entity_in_ast(world, child);
            child
        })
        .collect();
    app.update();
    (column, children)
}

fn ecs_order(world: &World, parent: Entity) -> Vec<String> {
    world
        .get::<Children>(parent)
        .map(|children| {
            children
                .iter()
                .filter_map(|child| world.get::<Name>(child))
                .map(|name| name.as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
}

fn document_order(world: &World, parent: Entity) -> Vec<String> {
    let ast = world.resource::<SceneBsnAst>();
    let Some(node) = ast.ast_for(parent) else {
        return Vec::new();
    };
    ast.get_children_ast(node)
        .into_iter()
        .filter_map(|child| ast.ecs_for_ast(child))
        .filter_map(|entity| world.get::<Name>(entity))
        .map(|name| name.as_str().to_string())
        .collect()
}

fn select(app: &mut App, entity: Entity) {
    jackdaw::selection::select_only(app.world_mut(), entity);
    app.update();
}

fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

#[test]
fn move_up_reorders_the_document_and_the_ecs_as_one_undo_entry() {
    let mut app = util::editor_test_app();
    let (column, children) = column_of_three(&mut app);
    select(&mut app, children[2]);
    let before = undo_depth(&app);

    run_finished(&mut app, "entity.move_up");

    assert_eq!(
        ecs_order(app.world(), column),
        vec!["First", "Third", "Second"],
        "the live children order is what lays the column out"
    );
    assert_eq!(
        document_order(app.world(), column),
        vec!["First", "Third", "Second"],
        "the document holds the new order, so a save keeps it"
    );
    assert_eq!(
        undo_depth(&app) - before,
        1,
        "one reorder is one history entry"
    );

    run_finished(&mut app, "history.undo");
    assert_eq!(
        ecs_order(app.world(), column),
        vec!["First", "Second", "Third"]
    );
    assert_eq!(
        document_order(app.world(), column),
        vec!["First", "Second", "Third"]
    );
}

#[test]
fn move_down_moves_the_other_way_and_stops_at_the_end() {
    let mut app = util::editor_test_app();
    let (column, children) = column_of_three(&mut app);
    select(&mut app, children[0]);

    run_finished(&mut app, "entity.move_down");
    assert_eq!(
        document_order(app.world(), column),
        vec!["Second", "First", "Third"]
    );

    select(&mut app, children[2]);
    let before = undo_depth(&app);
    run_finished(&mut app, "entity.move_down");
    assert_eq!(
        document_order(app.world(), column),
        vec!["Second", "First", "Third"],
        "the last child has nowhere later to go"
    );
    assert_eq!(
        undo_depth(&app),
        before,
        "a move that changes nothing records nothing"
    );
}

#[test]
fn a_reorder_survives_a_save_and_a_reload() {
    let mut app = util::editor_test_app();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("reordered.bsn");
    run_finished(
        &mut app,
        &format!("scene.new ui=true path={}", path.display()),
    );

    let (column, children) = column_of_three(&mut app);
    select(&mut app, children[2]);
    run_finished(&mut app, "entity.move_up");
    run_finished(&mut app, "scene.save");
    run_finished(&mut app, &format!("scene.open path={}", path.display()));

    let reloaded = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Column")
        .map(|(entity, _)| entity)
        .expect("the reloaded scene holds the column");
    let _ = column;
    assert_eq!(
        ecs_order(app.world(), reloaded),
        vec!["First", "Third", "Second"],
        "the saved file carried the order"
    );
}

#[test]
fn a_drop_in_the_gap_between_two_rows_reorders_the_siblings() {
    let mut app = util::editor_test_app();
    let (column, children) = column_of_three(&mut app);

    app.world_mut()
        .trigger(jackdaw_widgets::tree_view::TreeRowInserted {
            entity: children[0],
            dragged_source: children[0],
            target: children[2],
            index: 1,
        });
    app.update();
    app.update();

    assert_eq!(
        document_order(app.world(), column),
        vec!["Second", "Third", "First"],
        "dropping below the last row lands the dragged node after it"
    );

    // And back the other way: above the first row.
    app.world_mut().trigger(TreeRowInserted {
        entity: children[0],
        dragged_source: children[0],
        target: children[1],
        index: 0,
    });
    app.update();
    app.update();
    assert_eq!(
        document_order(app.world(), column),
        vec!["First", "Second", "Third"]
    );
}

#[test]
fn a_reorder_moves_the_row_in_every_outliner_panel() {
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(HierarchyShowAll(true));
    let panels: Vec<Entity> = (0..2)
        .map(|_| {
            app.world_mut()
                .spawn((
                    HierarchyTreeContainer,
                    Node::default(),
                    Visibility::Inherited,
                ))
                .id()
        })
        .collect();
    app.update();

    let (column, children) = column_of_three(&mut app);
    // Expand the column so its children have rows to reorder.
    for panel in &panels {
        let row = app
            .world()
            .resource::<TreeIndex>()
            .get(*panel, column)
            .expect("the column has a row");
        app.world_mut()
            .entity_mut(row)
            .insert(jackdaw_widgets::tree_view::TreeNodeExpanded(true));
    }
    app.update();
    app.update();

    select(&mut app, children[2]);
    run_finished(&mut app, "entity.move_up");
    app.update();

    for panel in &panels {
        assert_eq!(
            row_order(app.world(), *panel, column),
            vec!["First", "Third", "Second"],
            "panel {panel} still shows the old order"
        );
    }
}

/// Names of the child rows under `source`'s row in `panel`, in the order the
/// panel draws them.
fn row_order(world: &World, panel: Entity, source: Entity) -> Vec<String> {
    let Some(row) = world.resource::<TreeIndex>().get(panel, source) else {
        return Vec::new();
    };
    let Some(children) = world.get::<Children>(row) else {
        return Vec::new();
    };
    let Some(container) = children
        .iter()
        .find(|child| world.get::<TreeRowChildren>(*child).is_some())
    else {
        return Vec::new();
    };
    world
        .get::<Children>(container)
        .map(|rows| {
            rows.iter()
                .filter_map(|row| world.get::<TreeNode>(row))
                .filter_map(|node| world.get::<Name>(node.0))
                .map(|name| name.as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_reorder_of_a_multi_selection_is_one_entry() {
    let mut app = util::editor_test_app();
    let (column, children) = column_of_three(&mut app);
    {
        let world = app.world_mut();
        world.resource_mut::<Selection>().entities = vec![children[1], children[2]];
    }
    app.update();
    let before = undo_depth(&app);

    run_finished(&mut app, "entity.move_up");

    assert_eq!(
        document_order(app.world(), column),
        vec!["Second", "Third", "First"],
        "both selected children moved one slot earlier"
    );
    assert_eq!(undo_depth(&app) - before, 1);
}
