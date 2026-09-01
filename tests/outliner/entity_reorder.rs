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

use crate::util;

use bevy::prelude::*;
use jackdaw::boot_ops::run_op_clause_as_user;
use jackdaw::commands::CommandHistory;
use jackdaw::hierarchy::{HierarchyShowAll, HierarchyTreeContainer};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_bsn::SceneBsnAst;
use jackdaw_widgets::tree_view::{TreeIndex, TreeNode, TreeRowChildren, TreeRowInserted};

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

/// A selection packed against the end of its list stays packed. The first
/// entity has nowhere to go, so the one behind it has nowhere to go either:
/// letting it move into the blocked one's slot swaps the two, and the next
/// press swaps them back, so holding the chord shuffles the selection
/// instead of leaving it alone.
#[test]
fn a_selection_packed_against_the_top_keeps_its_own_order() {
    let mut app = util::editor_test_app();
    let (column, children) = column_of_three(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![children[1], children[2]];
    app.update();

    run_finished(&mut app, "entity.move_up");
    assert_eq!(
        document_order(app.world(), column),
        vec!["Second", "Third", "First"],
        "both moved while there was room",
    );

    run_finished(&mut app, "entity.move_up");
    assert_eq!(
        document_order(app.world(), column),
        vec!["Second", "Third", "First"],
        "at the top, neither moves; the blocked one blocks the one behind it",
    );
    assert_eq!(
        ecs_order(app.world(), column),
        vec!["Second", "Third", "First"],
    );
}

/// The same at the other end.
#[test]
fn a_selection_packed_against_the_bottom_keeps_its_own_order() {
    let mut app = util::editor_test_app();
    let (column, children) = column_of_three(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![children[0], children[1]];
    app.update();

    run_finished(&mut app, "entity.move_down");
    assert_eq!(
        document_order(app.world(), column),
        vec!["Third", "First", "Second"],
        "both moved while there was room",
    );

    run_finished(&mut app, "entity.move_down");
    assert_eq!(
        document_order(app.world(), column),
        vec!["Third", "First", "Second"],
        "at the bottom, neither moves",
    );
    assert_eq!(
        ecs_order(app.world(), column),
        vec!["Third", "First", "Second"],
    );
}

/// A drag that starts on a selected row carries the whole selection, the
/// way a drag in any list does, and lands it in the order the tree shows
/// rather than the order it was clicked in.
#[test]
fn a_drop_moves_the_whole_selection_in_the_order_the_tree_shows() {
    let mut app = util::editor_test_app();
    let (column, children) = column_of_three(&mut app);

    // Clicked bottom-up, so the click order is the reverse of the tree's.
    app.world_mut().resource_mut::<Selection>().entities = vec![children[1], children[0]];
    app.update();

    app.world_mut().trigger(TreeRowInserted {
        entity: children[0],
        dragged_source: children[0],
        target: children[2],
        index: 1,
    });
    app.update();
    app.update();

    assert_eq!(
        document_order(app.world(), column),
        vec!["Third", "First", "Second"],
        "both selected rows moved, in the order the tree drew them",
    );
    assert_eq!(
        ecs_order(app.world(), column),
        vec!["Third", "First", "Second"],
    );
}

/// A multi-row drop is one thing the user did, so it is one thing to undo.
#[test]
fn a_multi_selection_drop_is_one_history_entry() {
    let mut app = util::editor_test_app();
    let (column, children) = column_of_three(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![children[0], children[1]];
    app.update();
    let before = undo_depth(&app);

    app.world_mut().trigger(TreeRowInserted {
        entity: children[0],
        dragged_source: children[0],
        target: children[2],
        index: 1,
    });
    app.update();
    app.update();

    assert_eq!(undo_depth(&app) - before, 1);
    run_finished(&mut app, "history.undo");
    assert_eq!(
        document_order(app.world(), column),
        vec!["First", "Second", "Third"],
        "one undo put the whole drop back",
    );
}

/// A parent's after-gap is drawn in the same place as its last
/// descendant's: the descendant is the last thing under it, so both gaps
/// are the same line on screen. Taking the deepest one every time makes
/// "after the parent" a place with no pixel, so the pointer's x picks
/// between the levels against their indents.
#[test]
fn the_gap_below_a_last_child_means_the_level_the_pointer_is_at() {
    use bevy::camera::{NormalizedRenderTarget, RenderTarget};
    use bevy::picking::events::DragDrop;
    use bevy::picking::pointer::{Location, PointerId};
    use bevy::window::{PrimaryWindow, WindowRef};
    use jackdaw_widgets::tree_view::TreeRowInsertZone;

    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(HierarchyShowAll(true));
    let panel = app
        .world_mut()
        .spawn((
            HierarchyTreeContainer,
            Node {
                width: px(320.0),
                height: px(400.0),
                ..default()
            },
            Visibility::Inherited,
        ))
        .id();
    app.update();

    let (column, children) = column_of_three(&mut app);
    let row = app
        .world()
        .resource::<TreeIndex>()
        .get(panel, column)
        .expect("the column has a row");
    app.world_mut()
        .entity_mut(row)
        .insert(jackdaw_widgets::tree_view::TreeNodeExpanded(true));
    for _ in 0..4 {
        app.update();
    }

    let last = *children.last().expect("three children");
    let last_row = app
        .world()
        .resource::<TreeIndex>()
        .get(panel, last)
        .expect("the last child has a row");
    let zone = app
        .world()
        .get::<Children>(last_row)
        .expect("a row has children")
        .iter()
        .find(|&child| {
            app.world()
                .get::<TreeRowInsertZone>(child)
                .is_some_and(|zone| zone.after)
        })
        .expect("a row has an after-gap");

    // A sibling of the column, so a drop "after the column" has somewhere
    // to be told apart from "after its last child".
    let outsider = app
        .world_mut()
        .spawn((Name::new("Outsider"), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), outsider);
    app.update();

    let drop_at = |app: &mut App, x: f32| {
        let window = app
            .world_mut()
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(app.world())
            .expect("headless apps still have a primary window");
        let target: NormalizedRenderTarget = RenderTarget::Window(WindowRef::Primary)
            .normalize(Some(window))
            .expect("the primary window normalizes");
        app.world_mut().trigger(bevy::picking::events::Pointer::new(
            PointerId::Mouse,
            Location {
                target,
                position: Vec2::new(x, 0.0),
            },
            DragDrop {
                button: bevy::picking::pointer::PointerButton::Primary,
                dropped: last_row,
                hit: bevy::picking::backend::HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            },
            zone,
        ));
        app.update();
        app.update();
    };

    // Far right: the deepest level, so the drop stays among the column's
    // own children and nothing moves.
    drop_at(&mut app, 300.0);
    assert_eq!(
        ecs_order(app.world(), column),
        vec!["First", "Second", "Third"],
        "at the child's own indent the drop is a no-op among its siblings",
    );

    // Far left: the outer level, so the row leaves the column and lands
    // beside it.
    drop_at(&mut app, 0.0);
    assert_eq!(
        ecs_order(app.world(), column),
        vec!["First", "Second"],
        "at the outer indent the drop means after the column, not inside it",
    );
    assert_eq!(
        app.world().get::<ChildOf>(last).map(ChildOf::parent),
        None,
        "and the row is a sibling of the column now",
    );
}

/// A drag holds what it is carrying, so a closed parent cannot be opened
/// by clicking it. Resting the pointer on it during the drag opens it,
/// which is how a drop inside a closed subtree is reached at all.
#[test]
fn resting_a_drag_on_a_closed_row_opens_it() {
    use bevy::camera::{NormalizedRenderTarget, RenderTarget};
    use bevy::picking::events::DragEnter;
    use bevy::picking::pointer::{Location, PointerId};
    use bevy::window::{PrimaryWindow, WindowRef};
    use jackdaw_widgets::tree_view::{TreeNodeExpanded, TreeRowContent};

    let mut app = util::editor_test_app();
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
    let (column, _children) = column_of_three(&mut app);
    let row = app
        .world()
        .resource::<TreeIndex>()
        .get(panel, column)
        .expect("the column has a row");
    assert!(
        !app.world()
            .get::<TreeNodeExpanded>(row)
            .expect("a row tracks whether it is open")
            .0,
        "the fixture starts closed",
    );

    let content = app
        .world()
        .get::<Children>(row)
        .expect("a row has children")
        .iter()
        .find(|&child| app.world().get::<TreeRowContent>(child).is_some())
        .expect("a row has content");

    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    let target: NormalizedRenderTarget = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    app.world_mut().trigger(bevy::picking::events::Pointer::new(
        PointerId::Mouse,
        Location {
            target,
            position: Vec2::ZERO,
        },
        DragEnter {
            button: bevy::picking::pointer::PointerButton::Primary,
            dragged: Entity::PLACEHOLDER,
            hit: bevy::picking::backend::HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
        },
        content,
    ));
    app.update();
    assert!(
        !app.world()
            .get::<TreeNodeExpanded>(row)
            .expect("a row tracks whether it is open")
            .0,
        "a pointer crossing a row on its way elsewhere opens nothing",
    );
    assert_eq!(
        app.world()
            .resource::<jackdaw_widgets::tree_view::TreeSpringLoad>()
            .row,
        Some(row),
        "but the clock is running on it",
    );

    // Rest on it. The wait is a real interval, so the test hands the app
    // a clock it controls rather than waiting on the wall.
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_millis(100),
    ));
    for _ in 0..8 {
        app.update();
    }

    assert!(
        app.world()
            .get::<TreeNodeExpanded>(row)
            .expect("a row tracks whether it is open")
            .0,
        "resting on a closed row during a drag opens it",
    );
}
