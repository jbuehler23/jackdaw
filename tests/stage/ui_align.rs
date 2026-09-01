//! Aligning a selection to its bounding box, and distributing it along an
//! axis.
//!
//! What is pinned here:
//!  * each of the six alignments moves only its own axis, and lands every
//!    member on the line the box states;
//!  * the alignment writes through each node's own offset box, so a member
//!    under a bordered parent lands on the canvas line rather than ten
//!    pixels off it;
//!  * one press is one history entry that undoes to the exact `Node`;
//!  * a member its parent lays out refuses the whole press and says so;
//!  * distributing evens the gaps and leaves the two outermost members
//!    where they are, and refuses fewer than three.

use crate::util;

use bevy::prelude::*;
use jackdaw::boot_ops::run_op_clause_as_user;
use jackdaw::commands::CommandHistory;
use jackdaw::selection::Selection;
use jackdaw::status_bar::StatusNotice;
use jackdaw::viewport_2d::{Viewport2dPanelHost, build_viewport_2d_panel};
use jackdaw_api::prelude::*;
use jackdaw_feathers::tokens::TOOLBAR_HEIGHT;
use jackdaw_scene_types::UiSceneRoot;

const REFERENCE: UVec2 = UVec2::new(2400, 1200);

#[track_caller]
fn run_finished(app: &mut App, clause: &str) {
    let result = run_op_clause_as_user(app.world_mut(), clause)
        .unwrap_or_else(|err| panic!("{clause}: dispatch errored: {err}"));
    settle(app);
    assert_eq!(
        result,
        OperatorResult::Finished,
        "{clause} reported {result:?}"
    );
}

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

/// A 2D panel framed so the whole authored canvas fits it, which is what
/// gives the authored scene a target to be laid out against.
fn panel(app: &mut App) {
    let parent = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                width: px(1200.0 + jackdaw::viewport_2d::RULER_SIZE),
                height: px(600.0 + jackdaw::viewport_2d::RULER_SIZE + TOOLBAR_HEIGHT),
                ..default()
            },
        ))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);
    let mut host = app
        .world_mut()
        .get_mut::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent");
    host.view.zoom = 0.5;
    host.fit_pending = false;
}

fn root_with_border(app: &mut App) -> Entity {
    let root = app
        .world_mut()
        .spawn((
            Name::new("UiRoot"),
            UiSceneRoot {
                reference_size: REFERENCE,
            },
            Node {
                width: percent(100),
                height: percent(100),
                border: UiRect::all(px(10.0)),
                ..default()
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), root);
    root
}

fn child(
    app: &mut App,
    parent: Entity,
    name: &str,
    left: f32,
    top: f32,
    width: f32,
    height: f32,
) -> Entity {
    let entity = app
        .world_mut()
        .spawn((
            Name::new(name.to_string()),
            Node {
                position_type: PositionType::Absolute,
                left: px(left),
                top: px(top),
                width: px(width),
                height: px(height),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    entity
}

/// Three boxes of different sizes at different places, all under the same
/// bordered root.
fn three_boxes(app: &mut App) -> Vec<Entity> {
    panel(app);
    let root = root_with_border(app);
    let boxes = vec![
        child(app, root, "First", 100.0, 100.0, 200.0, 100.0),
        child(app, root, "Second", 400.0, 300.0, 100.0, 200.0),
        child(app, root, "Third", 700.0, 200.0, 300.0, 50.0),
    ];
    settle(app);
    boxes
}

fn select(app: &mut App, entities: &[Entity]) {
    app.world_mut().resource_mut::<Selection>().entities = entities.to_vec();
    settle(app);
}

fn offsets(app: &App, entity: Entity) -> (Val, Val) {
    let node = app.world().get::<Node>(entity).expect("a node");
    (node.left, node.top)
}

fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    settle(app);
}

#[test]
fn each_alignment_lands_every_member_on_its_own_line() {
    // Bounding box of the three: left 100, right 1000, top 100, bottom 500.
    // (id, expected `left` per box, expected `top` per box)
    let cases: [(&str, [Option<f32>; 3], [Option<f32>; 3]); 6] = [
        (
            "ui.align_left",
            [Some(100.0), Some(100.0), Some(100.0)],
            [None, None, None],
        ),
        (
            "ui.align_right",
            [Some(800.0), Some(900.0), Some(700.0)],
            [None, None, None],
        ),
        (
            "ui.align_center_x",
            [Some(450.0), Some(500.0), Some(400.0)],
            [None, None, None],
        ),
        (
            "ui.align_top",
            [None, None, None],
            [Some(100.0), Some(100.0), Some(100.0)],
        ),
        (
            "ui.align_bottom",
            [None, None, None],
            [Some(400.0), Some(300.0), Some(450.0)],
        ),
        (
            "ui.align_center_y",
            [None, None, None],
            [Some(250.0), Some(200.0), Some(275.0)],
        ),
    ];

    for (clause, lefts, tops) in cases {
        let mut app = util::editor_test_app();
        let boxes = three_boxes(&mut app);
        let before: Vec<(Val, Val)> = boxes.iter().map(|&b| offsets(&app, b)).collect();
        select(&mut app, &boxes);

        run_finished(&mut app, clause);

        for (index, &entity) in boxes.iter().enumerate() {
            let (left, top) = offsets(&app, entity);
            let want_left = lefts[index].map_or(before[index].0, px);
            let want_top = tops[index].map_or(before[index].1, px);
            assert_eq!(
                (left, top),
                (want_left, want_top),
                "{clause} put box {index} in the wrong place",
            );
        }
    }
}

#[test]
fn one_alignment_is_one_history_entry_that_undoes() {
    let mut app = util::editor_test_app();
    let boxes = three_boxes(&mut app);
    let before: Vec<(Val, Val)> = boxes.iter().map(|&b| offsets(&app, b)).collect();
    select(&mut app, &boxes);
    let depth = undo_depth(&app);

    run_finished(&mut app, "ui.align_left");

    assert_eq!(
        undo_depth(&app) - depth,
        1,
        "however many nodes moved, the press is one entry",
    );
    undo(&mut app);
    let after: Vec<(Val, Val)> = boxes.iter().map(|&b| offsets(&app, b)).collect();
    assert_eq!(after, before, "undo puts every member back where it was");
}

#[test]
fn a_member_its_parent_lays_out_refuses_the_whole_press() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let root = root_with_border(&mut app);
    let placed = child(&mut app, root, "Placed", 100.0, 100.0, 200.0, 100.0);
    let flowed = app
        .world_mut()
        .spawn((
            Name::new("Flowed"),
            Node {
                width: px(120.0),
                height: px(40.0),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), flowed);
    settle(&mut app);
    let before = offsets(&app, placed);
    select(&mut app, &[placed, flowed]);
    let depth = undo_depth(&app);

    run_finished(&mut app, "ui.align_left");

    assert_eq!(
        offsets(&app, placed),
        before,
        "a refusal moves nothing, not even the members that could have moved",
    );
    assert_eq!(undo_depth(&app), depth, "a refusal records nothing");
    let notice = app.world().resource::<StatusNotice>();
    assert!(
        notice.text().contains("Flowed"),
        "the notice names the member that cannot move: {:?}",
        notice.text(),
    );
}

#[test]
fn distributing_evens_the_gaps_and_leaves_the_ends_alone() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let root = root_with_border(&mut app);
    // Spans 0..600 with 300 pixels of box in it, so the two gaps are 150
    // each: the middle box belongs at 250.
    let boxes = vec![
        child(&mut app, root, "First", 0.0, 0.0, 100.0, 50.0),
        child(&mut app, root, "Second", 40.0, 0.0, 100.0, 50.0),
        child(&mut app, root, "Third", 500.0, 0.0, 100.0, 50.0),
    ];
    settle(&mut app);
    select(&mut app, &boxes);

    run_finished(&mut app, "ui.distribute_horizontal");

    assert_eq!(offsets(&app, boxes[0]).0, px(0.0), "the first end holds");
    assert_eq!(offsets(&app, boxes[1]).0, px(250.0), "the gaps are even");
    assert_eq!(offsets(&app, boxes[2]).0, px(500.0), "the last end holds");
}

#[test]
fn distributing_the_other_way_moves_the_other_axis() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let root = root_with_border(&mut app);
    let boxes = vec![
        child(&mut app, root, "First", 0.0, 0.0, 50.0, 100.0),
        child(&mut app, root, "Second", 0.0, 40.0, 50.0, 100.0),
        child(&mut app, root, "Third", 0.0, 500.0, 50.0, 100.0),
    ];
    settle(&mut app);
    select(&mut app, &boxes);

    run_finished(&mut app, "ui.distribute_vertical");

    assert_eq!(offsets(&app, boxes[1]).1, px(250.0));
    assert_eq!(
        offsets(&app, boxes[1]).0,
        px(0.0),
        "the axis that was not distributed is untouched",
    );
}

#[test]
fn distributing_two_nodes_refuses_rather_than_doing_nothing_quietly() {
    let mut app = util::editor_test_app();
    let boxes = three_boxes(&mut app);
    select(&mut app, &boxes[..2]);
    let depth = undo_depth(&app);

    run_finished(&mut app, "ui.distribute_horizontal");

    assert_eq!(undo_depth(&app), depth, "two nodes have no gap to even out");
    assert!(
        app.world()
            .resource::<StatusNotice>()
            .text()
            .contains("three"),
        "the notice says how many it needs",
    );
}

#[test]
fn one_selected_node_is_not_an_alignment() {
    let mut app = util::editor_test_app();
    let boxes = three_boxes(&mut app);
    let before = offsets(&app, boxes[0]);
    select(&mut app, &boxes[..1]);

    let result =
        run_op_clause_as_user(app.world_mut(), "ui.align_left").expect("dispatch does not error");
    settle(&mut app);

    assert_eq!(
        result,
        OperatorResult::Cancelled,
        "a lone node has nothing to line up with",
    );
    assert_eq!(offsets(&app, boxes[0]), before);
}
