//! The synthetic input operators, measured against what a hand-injected
//! gesture does.
//!
//! `input.pointer`, `input.key` and `input.text` exist so a scripted run
//! can do what only a mouse and a keyboard could: drag a node, click a
//! menu row, open the in-place text editor, rename a node. Nothing here
//! asserts that a message was written; each test asserts the *downstream*
//! effect -- the node moved, the selection changed, the entry opened --
//! because the whole point of feeding the window's own event streams is
//! that everything past them behaves as it does for a user.

use crate::util;

use crate::util::OperatorResultExt as _;
use bevy::{
    prelude::*,
    ui::ComputedNode,
    window::{PrimaryWindow, WindowResolution},
};
use jackdaw::{
    selection::Selection,
    test_input::SyntheticInput,
    ui_stage::UiSelectionOverlay,
    viewport_2d::{RULER_SIZE, Viewport2dPanelHost, build_viewport_2d_panel},
};
use jackdaw_scene_types::UiSceneRoot;

/// The reference resolution the canvas below is authored at, matching
/// the stage tests next door: twice the 1200x600 stage area, so every
/// conversion factor is an exact 2.
const REFERENCE: UVec2 = UVec2::new(2400, 1200);

/// An editor with one 2D panel showing a canvas, framed so the whole of
/// it fits, and a window big enough to hold the panel.
fn canvas_app() -> (App, Entity) {
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
    let parent = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                width: px(1200.0 + RULER_SIZE),
                height: px(600.0 + RULER_SIZE + jackdaw_feathers::tokens::TOOLBAR_HEIGHT),
                ..default()
            },
        ))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);
    {
        let mut host = app
            .world_mut()
            .get_mut::<Viewport2dPanelHost>(parent)
            .expect("host on panel parent");
        host.view.zoom = 0.5;
        host.fit_pending = false;
    }
    settle(&mut app);
    (app, parent)
}

/// A root filling the canvas with one absolutely placed child at
/// authored (400, 200), 400x200.
fn authored_panel(app: &mut App) -> Entity {
    let root = app
        .world_mut()
        .spawn((
            UiSceneRoot {
                reference_size: REFERENCE,
            },
            Name::new("UiRoot"),
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), root);
    let node = app
        .world_mut()
        .spawn((
            Name::new("Panel"),
            Node {
                position_type: PositionType::Absolute,
                left: px(400),
                top: px(200),
                width: px(400),
                height: px(200),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), node);
    node
}

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

/// Run one clause the way `JACKDAW_RUN_OP` runs it, then let the gesture
/// it queued play out to the last beat.
fn run(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    play(app);
}

/// Advance until the synthetic queue is empty, plus a settle.
///
/// Bounded: a gesture that never drained would otherwise hang the run
/// rather than fail it.
fn play(app: &mut App) {
    for _ in 0..600 {
        app.update();
        if app.world().resource::<SyntheticInput>().is_idle() {
            break;
        }
    }
    assert!(
        app.world().resource::<SyntheticInput>().is_idle(),
        "the gesture drained"
    );
    settle(app);
}

/// Where the cursor is, in window logical pixels.
fn cursor(app: &mut App) -> Option<Vec2> {
    let mut windows = app
        .world_mut()
        .query_filtered::<&Window, With<PrimaryWindow>>();
    windows
        .single(app.world())
        .ok()
        .and_then(Window::cursor_position)
}

fn node_of(app: &App, entity: Entity) -> Node {
    app.world()
        .get::<Node>(entity)
        .expect("the authored entity is a node")
        .clone()
}

fn history_len(app: &App) -> usize {
    app.world()
        .resource::<jackdaw::commands::CommandHistory>()
        .undo_stack
        .len()
}

/// A move in canvas space lands the cursor where the panel is showing
/// that authored point, and moving back through the forward mapping
/// returns the point.
///
/// The whole `space=canvas` contract: a script states a position in the
/// coordinates the inspector states them in, and the operator finds
/// where on screen that is, wherever the panel is docked and however far
/// it has been panned.
#[test]
fn a_canvas_position_lands_where_the_panel_is_showing_it() {
    let (mut app, panel) = canvas_app();
    let node = authored_panel(&mut app);
    settle(&mut app);

    run(
        &mut app,
        "input.pointer space=canvas x=600 y=300 action=move",
    );
    let landed = cursor(&mut app).expect("the cursor is over the window");

    // The forward mapping, read off the stage the same way the editor's
    // own hit test reads it.
    let (stage, target_size) = {
        let host = app
            .world()
            .get::<Viewport2dPanelHost>(panel)
            .expect("host on panel parent");
        (host.stage, host.target_size)
    };
    let computed = *app
        .world()
        .get::<ComputedNode>(stage)
        .expect("the stage is laid out");
    let transform = *app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(stage)
        .expect("the stage is placed");
    let scale = jackdaw::viewport_2d::target_pixels_per_stage_pixel(computed.size(), target_size);
    let ui_scale = app.world().resource::<UiScale>().0;
    let offset = jackdaw::viewport_2d::stage_offset_unbounded(
        landed / ui_scale,
        transform.translation,
        computed.inverse_scale_factor(),
        scale,
    );
    let back = jackdaw::ui_stage::stage_to_authored(offset, target_size);
    assert!(
        (back - Vec2::new(600.0, 300.0)).length() < 1.0,
        "a canvas move round-trips: aimed at (600, 300), landed on {back:?}",
    );
    // The node is beside the point, but a canvas that could not be aimed
    // at would fail the round trip above for every position.
    let _ = node;
}

/// A drag on the canvas moves the node and leaves exactly one history
/// entry, the same as the hand-injected drag next door.
#[test]
fn a_drag_moves_the_node_it_started_on_and_records_one_entry() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    settle(&mut app);

    // Select it first, so the outline the drag is delivered to exists,
    // exactly as a user's own click would leave it.
    app.world_mut().resource_mut::<Selection>().entities = vec![node];
    settle(&mut app);

    let before = node_of(&app, node);
    let entries = history_len(&app);

    // Put the cursor on the node, then drag from there: `drag_to`
    // presses where the pointer already is, as a hand on a mouse does.
    run(
        &mut app,
        "input.pointer space=canvas x=600 y=300 action=move",
    );
    run(
        &mut app,
        "input.pointer space=canvas x=700 y=380 action=drag_to steps=6",
    );

    let after = node_of(&app, node);
    assert_ne!(
        (before.left, before.top),
        (after.left, after.top),
        "the drag moved the node",
    );
    assert_eq!(
        history_len(&app) - entries,
        1,
        "one gesture is one history entry",
    );
}

/// A click on a node selects it, and a Shift-click on a second one
/// extends the selection rather than replacing it.
#[test]
fn shift_extends_what_a_click_selected() {
    let (mut app, _panel) = canvas_app();
    let first = authored_panel(&mut app);
    let root = app
        .world()
        .get::<ChildOf>(first)
        .expect("the node is in a scene")
        .parent();
    let second = app
        .world_mut()
        .spawn((
            Name::new("Second"),
            Node {
                position_type: PositionType::Absolute,
                left: px(1200),
                top: px(600),
                width: px(300),
                height: px(200),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    settle(&mut app);

    run(
        &mut app,
        "input.pointer space=canvas x=600 y=300 action=click",
    );
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![first],
        "a click selects the node under it",
    );

    run(
        &mut app,
        "input.pointer space=canvas x=1350 y=700 action=click mods=shift",
    );
    let selected = app.world().resource::<Selection>().entities.clone();
    assert!(
        selected.contains(&first) && selected.contains(&second),
        "shift extends rather than replaces: {selected:?}",
    );
}

/// A double click on a node carrying text opens the in-place editor over
/// it.
///
/// The two presses have to reach the same entity for `Pointer<Press>` to
/// carry a count of two, and the first press on an *unselected* node
/// spawns the selection outline over it, so the second lands on the
/// outline and counts as a first press there. The gesture is therefore
/// the one a user makes on a node they have already selected. That
/// asymmetry is the editor's, not this operator's: a hand-injected pair
/// of presses does the same thing.
#[test]
fn a_double_click_opens_the_in_place_editor() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    app.world_mut()
        .entity_mut(node)
        .insert(Text::new("Button"))
        .insert(Name::new("Label"));
    settle(&mut app);

    run(
        &mut app,
        "input.pointer space=canvas x=600 y=300 action=click",
    );
    run(
        &mut app,
        "input.pointer space=canvas x=600 y=300 action=dblclick",
    );
    assert_eq!(
        app.world()
            .resource::<jackdaw::ui_text_edit::TextEditSession>()
            .editing(),
        Some(node),
        "a double click opens the entry over the node it landed on",
    );
}

/// Typing goes through the keyboard into whatever holds the focus, so
/// the entry a double click opened takes a new label and the commit
/// writes it to the node.
#[test]
fn typing_reaches_the_entry_the_canvas_opened() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    // An empty label, so what the node ends up saying is what was typed
    // and nothing about where the caret sat when the entry opened.
    app.world_mut()
        .entity_mut(node)
        .insert(Text::new(""))
        .insert(Name::new("Label"));
    settle(&mut app);

    run(
        &mut app,
        "input.pointer space=canvas x=600 y=300 action=click",
    );
    run(
        &mut app,
        "input.pointer space=canvas x=600 y=300 action=dblclick",
    );
    run(&mut app, "input.text text=Play");
    run(&mut app, "input.key key=Enter");

    assert_eq!(
        app.world()
            .get::<Text>(node)
            .map(|text| text.0.clone())
            .unwrap_or_default(),
        "Play",
        "what was typed is what the node says",
    );
}

/// `input.key` presses a key the editor's own chords read, so a bare
/// Delete deletes the selection exactly as pressing it does.
#[test]
fn a_key_reaches_the_editors_own_chords() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![node];
    settle(&mut app);

    run(&mut app, "input.key key=Delete");
    assert!(
        app.world().get_entity(node).is_err(),
        "Delete deletes the selection",
    );
}

/// A move is a hover: the node under the cursor gets the pre-select
/// outline before anything is clicked.
#[test]
fn a_move_hovers_the_node_it_stops_over() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    settle(&mut app);

    run(
        &mut app,
        "input.pointer space=canvas x=600 y=300 action=move",
    );
    // A second beat, because entering the stage takes one pointer input
    // to warm up (see `forward_pointer_into_stage`).
    run(
        &mut app,
        "input.pointer space=canvas x=605 y=305 action=move",
    );

    let outlines = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::ui_stage::UiHoverOutline>>()
        .iter(app.world())
        .count();
    assert_eq!(outlines, 1, "the node under the cursor is outlined");
    let _ = node;
}

/// Nothing here is meant to be pressable: an input operator with a chord
/// would move the mouse out from under the user.
#[test]
fn the_input_operators_are_listed_but_never_bound() {
    let mut app = util::editor_test_app();
    let pending = jackdaw::keybind_settings::pending_from_world(app.world_mut());
    for id in ["input.pointer", "input.key", "input.text"] {
        let row = pending
            .rows
            .iter()
            .find(|row| row.operator == id)
            .unwrap_or_else(|| panic!("the dialog lists {id}"));
        assert!(!row.bindable, "{id} has no action to attach a chord to");
        assert!(!row.is_editable(), "{id} offers no rebind");
        assert!(row.fixed.is_empty(), "{id} holds no chord of its own");
    }
}

/// An overlay is spawned per selection, so a selection made by a click
/// draws the same chrome one made by an operator does.
#[test]
fn a_click_draws_the_same_selection_chrome_an_operator_does() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    settle(&mut app);

    run(
        &mut app,
        "input.pointer space=canvas x=600 y=300 action=click",
    );
    let overlays: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<UiSelectionOverlay>>()
        .iter(app.world())
        .collect();
    assert_eq!(overlays.len(), 1, "exactly one outline per selection");
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![node],
        "and it is drawn over what the click selected",
    );
}
