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

/// A selection can hold both a parent and something under it. Moving the
/// parent moves its subtree, so carrying the child as well would move it
/// twice: once inside the parent, and once more on its own, which lands it
/// beside the parent instead of in it. The subtree travels once.
#[test]
fn a_drop_carrying_a_parent_and_its_child_moves_the_subtree_once() {
    let mut app = util::editor_test_app();
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
    let parent = world
        .spawn((Name::new("Parent"), Node::default(), ChildOf(column)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, parent);
    let child = world
        .spawn((Name::new("Child"), Node::default(), ChildOf(parent)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, child);
    let last = world
        .spawn((Name::new("Last"), Node::default(), ChildOf(column)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, last);
    app.update();
    assert_eq!(document_order(app.world(), column), vec!["Parent", "Last"]);

    app.world_mut().resource_mut::<Selection>().entities = vec![parent, child];
    app.update();

    app.world_mut().trigger(TreeRowInserted {
        entity: parent,
        dragged_source: parent,
        target: last,
        index: 1,
    });
    app.update();
    app.update();

    assert_eq!(
        document_order(app.world(), column),
        vec!["Last", "Parent"],
        "the parent moved and the child went with it, rather than beside it",
    );
    assert_eq!(
        document_order(app.world(), parent),
        vec!["Child"],
        "the child is still under the parent it was dragged with",
    );
    assert_eq!(
        app.world().get::<ChildOf>(child).map(ChildOf::parent),
        Some(parent),
        "and the ECS agrees",
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

/// The gap below a nest of last children is one strip standing for
/// several places at once, and which zone entity the pick hands over says
/// nothing about which of them was meant: a row's own after-gap is a later
/// sibling than the container holding its children, so it is drawn over
/// every gap nested inside it and wins every pick there. The level is the
/// pointer's x against each candidate row's own indent.
#[test]
fn the_gap_below_a_nest_of_last_children_means_the_level_the_pointer_is_at() {
    let mut app = drop_depth_app(1.0);
    let nest = nest_of_three(&mut app);

    let (zone, gap) = picked_gap(&mut app, &nest);

    // Far right: the deepest level, so the drop stays where it is.
    drop_at(&mut app, zone, nest.leaf_row, Vec2::new(300.0, gap.y));
    assert_eq!(
        app.world().get::<ChildOf>(nest.leaf).map(ChildOf::parent),
        Some(nest.middle),
        "at its own indent the drop is a no-op among its siblings",
    );

    // One level in: after the middle row, so a sibling of it.
    drop_at(
        &mut app,
        zone,
        nest.leaf_row,
        Vec2::new(gap.middle_indent, gap.y),
    );
    assert_eq!(
        app.world().get::<ChildOf>(nest.leaf).map(ChildOf::parent),
        Some(nest.outer),
        "at the middle row's indent the drop means after that row",
    );

    // Far left: the outermost level, so out of the nest entirely.
    let (zone, gap) = picked_gap(&mut app, &nest);
    drop_at(&mut app, zone, nest.leaf_row, Vec2::new(0.0, gap.y));
    assert_eq!(
        app.world().get::<ChildOf>(nest.leaf).map(ChildOf::parent),
        None,
        "at the outer indent the drop means after the outermost row",
    );
}

/// The pointer's position is logical pixels and a laid-out node's is
/// physical, so the two only agree at a scale factor of 1. Unconverted,
/// every indent reads as further right than it is drawn, and a pointer
/// resting on one level is answered with a shallower one.
#[test]
fn the_level_the_pointer_is_at_survives_a_scale_factor() {
    let mut app = drop_depth_app(2.0);
    let nest = nest_of_three(&mut app);
    assert_eq!(
        app.world()
            .get::<bevy::ui::ComputedNode>(nest.rows[2])
            .expect("a laid-out row")
            .inverse_scale_factor(),
        0.5,
        "precondition: the rows are laid out at two physical pixels per logical one",
    );

    let (zone, gap) = picked_gap(&mut app, &nest);
    drop_at(
        &mut app,
        zone,
        nest.leaf_row,
        Vec2::new(gap.middle_indent, gap.y),
    );
    assert_eq!(
        app.world().get::<ChildOf>(nest.leaf).map(ChildOf::parent),
        Some(nest.outer),
        "the middle row's indent still means the middle row's level",
    );
}

/// The rows and entities of a three-level nest, deepest last.
struct Nest {
    outer: Entity,
    middle: Entity,
    leaf: Entity,
    /// The rows of `outer`, `middle` and `leaf`, in that order.
    rows: [Entity; 3],
    leaf_row: Entity,
}

/// Where the one gap under the nest is, and the indents that tell its
/// levels apart. All logical pixels, as the pointer reports them.
struct Gap {
    y: f32,
    middle_indent: f32,
}

/// An Outliner panel wide enough for three indents, at `scale`.
fn drop_depth_app(scale: f32) -> App {
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(HierarchyShowAll(true));
    // What the editor is laid out at on a hidpi screen: a laid-out node's
    // figures are multiplied by this, while the pointer keeps reporting
    // logical pixels.
    app.world_mut().insert_resource(bevy::ui::UiScale(scale));
    app.world_mut().spawn((
        HierarchyTreeContainer,
        Node {
            width: px(320.0),
            height: px(400.0),
            ..default()
        },
        Visibility::Inherited,
    ));
    app.update();
    app
}

/// Outer > Middle > Leaf, every level open, with the deepest row the last
/// thing under each of the two above it.
fn nest_of_three(app: &mut App) -> Nest {
    let world = app.world_mut();
    let outer = world.spawn((Name::new("Outer"), Node::default())).id();
    jackdaw::scene_io::register_entity_in_ast(world, outer);
    let middle = world
        .spawn((Name::new("Middle"), Node::default(), ChildOf(outer)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, middle);
    let leaf = world
        .spawn((Name::new("Leaf"), Node::default(), ChildOf(middle)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, leaf);
    // A sibling of the outermost row, so "after Outer" is a place that can
    // be told apart from "inside it".
    let outsider = world.spawn((Name::new("Outsider"), Node::default())).id();
    jackdaw::scene_io::register_entity_in_ast(world, outsider);
    app.update();

    let panel = app
        .world_mut()
        .query_filtered::<Entity, With<HierarchyTreeContainer>>()
        .single(app.world())
        .expect("the fixture built one panel");
    let mut rows = [Entity::PLACEHOLDER; 3];
    for (slot, source) in [outer, middle, leaf].into_iter().enumerate() {
        for _ in 0..4 {
            app.update();
        }
        let row = app
            .world()
            .resource::<TreeIndex>()
            .get(panel, source)
            .unwrap_or_else(|| panic!("the tree shows a row for {source}"));
        app.world_mut()
            .entity_mut(row)
            .insert(jackdaw_widgets::tree_view::TreeNodeExpanded(true));
        rows[slot] = row;
    }
    for _ in 0..4 {
        app.update();
    }
    Nest {
        outer,
        middle,
        leaf,
        rows,
        leaf_row: rows[2],
    }
}

/// The after-gap belonging to `row`.
fn after_zone_of(app: &App, row: Entity) -> Entity {
    use jackdaw_widgets::tree_view::TreeRowInsertZone;
    app.world()
        .get::<Children>(row)
        .expect("a row has children")
        .iter()
        .find(|&child| {
            app.world()
                .get::<TreeRowInsertZone>(child)
                .is_some_and(|zone| zone.after)
        })
        .expect("a row has an after-gap")
}

/// The zone a drag lands on at the gap below the nest, and the geometry of
/// that gap.
///
/// The zone is the outermost row's, because that is what the pick hands
/// over: a row's after-gap is a later sibling than the container holding
/// its children, so it is drawn over every gap nested inside it. That
/// ordering is asserted here rather than assumed, so a change to it fails
/// this test instead of quietly aiming it somewhere a drag never lands.
fn picked_gap(app: &mut App, nest: &Nest) -> (Entity, Gap) {
    use jackdaw_widgets::tree_view::{TreeRowChildren, TreeRowInsertZone};

    for row in nest.rows {
        let children: Vec<Entity> = app
            .world()
            .get::<Children>(row)
            .expect("a row has children")
            .iter()
            .collect();
        let container = children
            .iter()
            .position(|&child| app.world().get::<TreeRowChildren>(child).is_some())
            .expect("a row holds its children in a container");
        let gap = children
            .iter()
            .position(|&child| {
                app.world()
                    .get::<TreeRowInsertZone>(child)
                    .is_some_and(|zone| zone.after)
            })
            .expect("a row has an after-gap");
        assert!(
            gap > container,
            "a row's after-gap is drawn over what is nested in it",
        );
    }

    let logical = |entity: Entity| -> Rect {
        let computed = app
            .world()
            .get::<bevy::ui::ComputedNode>(entity)
            .expect("a laid-out node");
        let transform = app
            .world()
            .get::<bevy::ui::UiGlobalTransform>(entity)
            .expect("a laid-out node");
        let scale = computed.inverse_scale_factor();
        Rect::from_center_size(transform.translation * scale, computed.size() * scale)
    };
    let strip = logical(after_zone_of(app, nest.leaf_row));
    let gap = Gap {
        y: strip.center().y,
        middle_indent: logical(nest.rows[1]).min.x,
    };
    (after_zone_of(app, nest.rows[0]), gap)
}

/// The primary window as a pointer location's target.
fn window_target(app: &mut App) -> bevy::camera::NormalizedRenderTarget {
    use bevy::camera::RenderTarget;
    use bevy::window::{PrimaryWindow, WindowRef};
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes")
}

/// Drop `dragged` on `zone` with the pointer at `position`.
fn drop_at(app: &mut App, zone: Entity, dragged: Entity, position: Vec2) {
    use bevy::picking::events::DragDrop;
    use bevy::picking::pointer::{Location, PointerId};

    let target = window_target(app);
    app.world_mut().trigger(bevy::picking::events::Pointer::new(
        PointerId::Mouse,
        Location { target, position },
        DragDrop {
            button: bevy::picking::pointer::PointerButton::Primary,
            dropped: dragged,
            hit: bevy::picking::backend::HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
        },
        zone,
    ));
    for _ in 0..3 {
        app.update();
    }
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

/// The drop line is drawn from figures in two units: `TreeDropLine.indent`
/// is logical pixels, while a laid-out node's transform and size are
/// physical. On a hidpi screen a figure taken from the wrong one puts the
/// line at twice or half the gap it marks, and every test that reads the
/// resource rather than the drawn node passes anyway.
#[test]
fn the_drop_line_is_drawn_at_the_gap_it_marks_on_a_hidpi_screen() {
    use jackdaw_feathers::tree_view::TreeDropIndicator;
    use jackdaw_widgets::tree_view::TreeDropLine;

    const SCALE: f32 = 2.0;
    const INDENT: f32 = 48.0;

    let mut app = drop_depth_app(SCALE);
    let nest = nest_of_three(&mut app);
    let zone = after_zone_of(&app, nest.leaf_row);
    {
        let mut line = app.world_mut().resource_mut::<TreeDropLine>();
        line.zone = Some(zone);
        line.indent = INDENT;
    }
    for _ in 0..4 {
        app.update();
    }

    let indicator = app
        .world_mut()
        .query_filtered::<Entity, With<TreeDropIndicator>>()
        .single(app.world())
        .expect("one drop line is drawn");
    let root = app
        .world()
        .get::<ChildOf>(indicator)
        .map(ChildOf::parent)
        .expect("the line hangs off the tree root");

    // Physical, as the layout leaves them: the point of the test is that
    // the drawn line lands on the gap whatever the scale factor is.
    let physical = |entity: Entity| -> Rect {
        let computed = app
            .world()
            .get::<bevy::ui::ComputedNode>(entity)
            .expect("a laid-out node");
        let transform = app
            .world()
            .get::<bevy::ui::UiGlobalTransform>(entity)
            .expect("a laid-out node");
        Rect::from_center_size(transform.translation, computed.size())
    };
    let gap = physical(zone).max.y;
    let drawn = physical(indicator);
    assert!(
        (drawn.center().y - gap).abs() < 1.5,
        "the line sits on the gap: it is at {} and the gap is at {gap}",
        drawn.center().y,
    );
    assert!(
        (drawn.min.x - (physical(root).min.x + INDENT * SCALE)).abs() < 0.5,
        "and starts at the indent, read as the logical figure it is: {}",
        drawn.min.x,
    );
}

/// Ctrl+Up reorders and nothing else.
///
/// The chord is pressed through the window's own key stream because the
/// fault only exists there: `bevy_enhanced_input` matches a binding on the
/// modifiers it names and ignores the rest, so the bare-arrow nudge answered
/// Ctrl+Arrow as well and the entity moved a grid step sideways on its way
/// up the list. A call by hand reaches one operator and would never show it.
#[test]
fn ctrl_up_reorders_without_nudging() {
    use bevy::window::{PrimaryWindow, WindowResolution};
    use jackdaw::test_input::SyntheticInput;

    let mut app = util::editor_test_app();
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows
            .single_mut(app.world_mut())
            .expect("headless apps still have a primary window");
        window.resolution = WindowResolution::new(1600, 1000);
    }
    let (column, children) = column_of_three(&mut app);
    app.world_mut()
        .entity_mut(children[2])
        .insert(Transform::default());
    jackdaw::selection::select_only(app.world_mut(), children[2]);
    for _ in 0..8 {
        app.update();
    }
    let before = *app
        .world()
        .get::<Transform>(children[2])
        .expect("the third child has a transform");

    let dispatched =
        jackdaw::boot_ops::run_op_clause(app.world_mut(), "input.key key=ArrowUp mods=ctrl")
            .expect("the clause dispatches");
    assert_eq!(dispatched, OperatorResult::Finished);
    for _ in 0..600 {
        app.update();
        if app.world().resource::<SyntheticInput>().is_idle() {
            break;
        }
    }
    for _ in 0..8 {
        app.update();
    }

    assert_eq!(
        ecs_order(app.world(), column),
        vec!["First", "Third", "Second"],
        "Ctrl+Up moves the selection up its parent's list",
    );
    assert_eq!(
        app.world().get::<Transform>(children[2]).copied(),
        Some(before),
        "and moves it nowhere in space",
    );
}
