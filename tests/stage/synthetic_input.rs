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
/// it, from cold: nothing selected, nothing clicked first.
///
/// This is the gesture a user makes, and the one the editor used to
/// refuse. The first press selects the node, selecting it spawns the
/// outline over it, and the second press therefore lands on a different
/// entity and arrives carrying a count of one. The pair is counted
/// against the authored node under the cursor instead, so the outline
/// appearing between the two presses is not something the user has to
/// know about.
#[test]
fn a_cold_double_click_opens_the_in_place_editor() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    app.world_mut()
        .entity_mut(node)
        .insert(Text::new("Button"))
        .insert(Name::new("Label"));
    settle(&mut app);
    assert!(
        app.world().resource::<Selection>().entities.is_empty(),
        "nothing is selected before the gesture",
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
        "a double click on an unselected node opens the entry over it",
    );
}

/// One press is not two: a single click selects and opens nothing, so
/// the test above is measuring the pair rather than the press.
#[test]
fn a_single_click_opens_no_entry() {
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
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![node],
        "the click selected the node",
    );
    assert_eq!(
        app.world()
            .resource::<jackdaw::ui_text_edit::TextEditSession>()
            .editing(),
        None,
        "and opened no entry",
    );
}

/// The entry opens with the whole label selected, so the first thing
/// typed replaces it rather than joining it.
///
/// The selection queued when the entry is spawned does not survive the
/// focus arriving a frame later, which is why the label used to come out
/// as `ButtonPlay`.
#[test]
fn typing_into_a_freshly_opened_entry_replaces_the_label() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    app.world_mut()
        .entity_mut(node)
        .insert(Text::new("Button"))
        .insert(Name::new("Label"));
    settle(&mut app);

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
        "what was typed replaced what was there",
    );
}

/// A label small enough for the chrome to cover it still takes the
/// gesture once it is selected.
///
/// A 40x20 node is barely bigger than the eight handles hung around it,
/// so a press over it lands on a handle rather than on the stage. The
/// pair is counted against the node the chrome belongs to, so the entry
/// opens on a node selected in the outliner first, which is how a label
/// is renamed in practice.
#[test]
fn a_double_click_on_a_selected_small_label_opens_the_entry() {
    let (mut app, _panel) = canvas_app();
    let node = small_label(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![node];
    settle(&mut app);

    // Inside the label, and on the handle straddling its top edge.
    run(
        &mut app,
        "input.pointer space=canvas x=420 y=204 action=dblclick",
    );
    assert_eq!(
        app.world()
            .resource::<jackdaw::ui_text_edit::TextEditSession>()
            .editing(),
        Some(node),
        "a double click on a selected node opens the entry over it",
    );
}

/// The same gesture on the same small label with nothing selected: both
/// routes into the entry open it.
#[test]
fn a_double_click_on_an_unselected_small_label_opens_the_entry() {
    let (mut app, _panel) = canvas_app();
    let node = small_label(&mut app);
    settle(&mut app);

    // The first press selects and hangs the handles, so the second lands
    // on one of them just as it does above.
    run(
        &mut app,
        "input.pointer space=canvas x=420 y=204 action=dblclick",
    );
    assert_eq!(
        app.world()
            .resource::<jackdaw::ui_text_edit::TextEditSession>()
            .editing(),
        Some(node),
        "a double click on an unselected node opens the entry over it",
    );
}

/// A 40x20 label carrying text, at authored (400, 200): the size
/// `Add > UI > Label` gives one.
fn small_label(app: &mut App) -> Entity {
    let node = authored_panel(app);
    let mut entity = app.world_mut().entity_mut(node);
    entity.insert((Text::new("Label"), Name::new("Label")));
    let mut layout = entity.get_mut::<Node>().expect("the label is a node");
    layout.width = px(40);
    layout.height = px(20);
    node
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

/// A chord typed into the canvas entry commands the field, not the
/// scene.
///
/// The entry is Bevy's own text input rather than the editor's field
/// wrapper, and the keyboard guard used to ask after the wrapper: a
/// Ctrl+D pressed while renaming a label duplicated the node under it.
#[test]
fn a_chord_typed_into_the_canvas_entry_runs_no_operator() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    app.world_mut()
        .entity_mut(node)
        .insert((Text::new("Button"), Name::new("Label")));
    settle(&mut app);
    let root = app
        .world()
        .get::<ChildOf>(node)
        .expect("the node is in a scene")
        .parent();
    let before = children_of(&app, root);

    run(
        &mut app,
        "input.pointer space=canvas x=600 y=300 action=dblclick",
    );
    run(&mut app, "input.text text=PlayButton");
    run(&mut app, "input.key key=KeyD mods=ctrl");

    assert_eq!(
        children_of(&app, root),
        before,
        "Ctrl+D typed into the entry duplicated nothing",
    );
    run(&mut app, "input.key key=Enter");
    assert_eq!(
        app.world()
            .get::<Text>(node)
            .map(|text| text.0.clone())
            .unwrap_or_default(),
        "PlayButton",
        "and what was typed reached the field",
    );
}

/// The same for the Add Entity picker's search field, which is built
/// from the same bare text input.
#[test]
fn a_chord_typed_into_the_add_entity_search_runs_no_operator() {
    let (mut app, _panel) = canvas_app();
    app.world_mut()
        .remove_resource::<jackdaw::entity_ops::SystemClipboard>();
    let node = authored_panel(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![node];
    settle(&mut app);
    let root = app
        .world()
        .get::<ChildOf>(node)
        .expect("the node is in a scene")
        .parent();
    let before = children_of(&app, root);

    run(&mut app, "entity.add_picker");
    run(&mut app, "input.text text=Play");
    run(&mut app, "input.key key=KeyD mods=ctrl");
    run(&mut app, "input.key key=KeyC mods=ctrl");

    assert_eq!(
        children_of(&app, root),
        before,
        "Ctrl+D typed into the search duplicated nothing",
    );
    assert!(
        app.world()
            .resource::<jackdaw::entity_ops::EntityClipboard>()
            .text
            .is_empty(),
        "and Ctrl+C copied the search text rather than the selection",
    );
}

/// How many children `parent` has, which is what a duplicate or a paste
/// would change.
fn children_of(app: &App, parent: Entity) -> usize {
    app.world()
        .get::<Children>(parent)
        .map_or(0, bevy::prelude::RelationshipTarget::len)
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

/// A selection of two draws two outlines, and only the primary one
/// carries the resize handles.
///
/// A selection that drew one line said the other node was not selected,
/// while the next chord acted on both.
#[test]
fn every_selected_node_is_outlined_and_the_primary_carries_the_handles() {
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
    run(
        &mut app,
        "input.pointer space=canvas x=1350 y=700 action=click mods=shift",
    );

    let outlined: Vec<(Entity, bool)> = app
        .world_mut()
        .query::<&UiSelectionOverlay>()
        .iter(app.world())
        .map(|overlay| (overlay.node, overlay.primary))
        .collect();
    assert_eq!(outlined.len(), 2, "one outline per selected node");
    assert!(
        outlined.iter().any(|(node, _)| *node == first)
            && outlined.iter().any(|(node, _)| *node == second),
        "both selected nodes are outlined: {outlined:?}",
    );
    assert_eq!(
        outlined
            .iter()
            .filter(|(_, primary)| *primary)
            .map(|(node, _)| *node)
            .collect::<Vec<_>>(),
        vec![second],
        "the node the last click landed on is the primary",
    );

    let primary_overlay = app
        .world_mut()
        .query::<(Entity, &UiSelectionOverlay)>()
        .iter(app.world())
        .find(|(_, overlay)| overlay.primary)
        .map(|(entity, _)| entity)
        .expect("a primary outline is drawn");
    let handles: Vec<Entity> = app
        .world_mut()
        .query_filtered::<&ChildOf, With<jackdaw::ui_stage::UiResizeHandle>>()
        .iter(app.world())
        .map(bevy::prelude::ChildOf::parent)
        .collect();
    assert!(!handles.is_empty(), "the primary has handles");
    assert!(
        handles.iter().all(|parent| *parent == primary_overlay),
        "and no other outline does",
    );
}

/// Press a key through the window's own event stream, then run the
/// numeric-entry reader on the frame the press lands on.
///
/// The reader is scheduled inside `EditorInteractionSystems`, which only
/// runs in the editor state a headless app never enters, so the frame is
/// driven here and the reader asked for by name -- the same arrangement
/// `jackdaw::numeric_transform::run_numeric_transform_input` exists for.
/// The press itself is the real one: `ButtonInput` picked it up from the
/// window's keyboard stream.
fn press_and_read(app: &mut App, clause: &str, key: KeyCode) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    for _ in 0..60 {
        app.update();
        if app
            .world()
            .resource::<ButtonInput<KeyCode>>()
            .just_pressed(key)
        {
            jackdaw::numeric_transform::run_numeric_transform_input(app.world_mut());
            return;
        }
    }
    panic!("the synthetic press reached ButtonInput");
}

fn armed_axis(app: &App) -> Option<jackdaw::gizmos::GizmoAxis> {
    app.world()
        .resource::<jackdaw::numeric_transform::NumericTransformState>()
        .axis
}

/// A letter typed with nothing focused does not arm an axis while the
/// canvas is what the user is looking at.
///
/// X, Y and Z name the axes of a world that has three of them. Typing a
/// name into a panel that has not taken the keyboard used to spell one
/// out: `PlayButton` armed Y and put the numeric transform entry on the
/// status bar.
#[test]
fn a_letter_arms_no_axis_while_the_canvas_is_in_front() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    // Selected, so the numeric entry has a target; the pointer over the
    // canvas is what says which world the keyboard belongs to.
    app.world_mut().resource_mut::<Selection>().entities = vec![node];
    settle(&mut app);

    run(
        &mut app,
        "input.pointer space=canvas x=600 y=300 action=move",
    );
    press_and_read(&mut app, "input.key key=KeyY action=press", KeyCode::KeyY);
    assert_eq!(
        armed_axis(&app),
        None,
        "the canvas is in front, so Y is a letter",
    );
}

/// With the pointer off the canvas and no 2D panel fronted, the same key
/// still arms the axis: the gate is about which world is in front, not
/// about taking the chord away.
#[test]
fn the_same_letter_still_arms_the_axis_away_from_the_canvas() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    app.world_mut().resource_mut::<Selection>().entities = vec![node];
    settle(&mut app);

    // Off the panel entirely, into the window's bottom-right corner.
    run(&mut app, "input.pointer x=1560 y=960 action=move");
    press_and_read(&mut app, "input.key key=KeyY action=press", KeyCode::KeyY);
    assert_eq!(
        armed_axis(&app),
        Some(jackdaw::gizmos::GizmoAxis::Y),
        "away from the canvas the chord is the chord it always was",
    );
}

/// Ctrl+C copies and Ctrl+V pastes, pressed on the keyboard.
///
/// Both used to do nothing on a canvas, and the reason was Ctrl+C: the
/// draw brush's cut gesture is bound to a bare C, `bevy_enhanced_input`
/// matches a binding on the modifiers it names and ignores the ones it
/// does not, so Ctrl+C started a brush too. That modal is one every
/// entity operator refuses to run behind, and it stayed up.
#[test]
fn ctrl_c_copies_and_ctrl_v_pastes_from_the_keyboard() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    let root = app
        .world()
        .get::<ChildOf>(node)
        .expect("the node is in a scene")
        .parent();
    app.world_mut().resource_mut::<Selection>().entities = vec![node];
    settle(&mut app);

    let before = app
        .world()
        .get::<Children>(root)
        .map_or(0, bevy::prelude::Children::len);

    run(&mut app, "input.key key=KeyC mods=ctrl");
    assert!(
        !app.world()
            .resource::<jackdaw::entity_ops::EntityClipboard>()
            .text
            .is_empty(),
        "Ctrl+C filled the clipboard",
    );

    run(&mut app, "input.key key=KeyV mods=ctrl");
    assert_eq!(
        app.world()
            .get::<Children>(root)
            .map_or(0, bevy::prelude::Children::len),
        before + 1,
        "Ctrl+V landed a copy beside it",
    );
}

/// Ctrl+ArrowUp reorders the selection among its siblings.
///
/// In the walkthrough this switched the tool to Draw Brush instead. It
/// was not Ctrl+ArrowUp that did that: the brush modal had been standing
/// since the Ctrl+C two clauses earlier, and this was the press that
/// made it visible.
#[test]
fn ctrl_arrow_up_reorders_from_the_keyboard() {
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
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), second);
    app.world_mut().resource_mut::<Selection>().entities = vec![second];
    settle(&mut app);

    let order = |app: &App| -> Vec<Entity> {
        app.world()
            .get::<Children>(root)
            .map(|children| children.iter().collect())
            .unwrap_or_default()
    };
    assert_eq!(order(&app), vec![first, second], "the order to move");

    run(&mut app, "input.key key=ArrowUp mods=ctrl");
    assert_eq!(
        order(&app),
        vec![second, first],
        "Ctrl+ArrowUp moved the selection up among its siblings",
    );
}

/// A rest holds the cursor still for as many frames as it names.
///
/// The hover a script is waiting on is the hover it already has: a move to
/// the same point still reports a `CursorMoved`, and a menu reads that as
/// the pointer stirring rather than dwelling. A rest emits nothing at all,
/// so the frames pass with the pointer exactly where the last beat left it.
#[test]
fn a_rest_lets_frames_pass_without_moving_the_pointer() {
    let (mut app, _panel) = canvas_app();
    let node = authored_panel(&mut app);
    settle(&mut app);

    run(
        &mut app,
        "input.pointer space=canvas x=600 y=300 action=move",
    );
    run(
        &mut app,
        "input.pointer space=canvas x=605 y=305 action=move",
    );
    let resting = cursor(&mut app).expect("the move put the cursor somewhere");
    let hovered = |app: &mut App| {
        app.world_mut()
            .query_filtered::<Entity, With<jackdaw::ui_stage::UiHoverOutline>>()
            .iter(app.world())
            .count()
    };
    assert_eq!(hovered(&mut app), 1);

    let before = app.world().resource::<bevy::diagnostic::FrameCount>().0;
    run(&mut app, "input.pointer action=rest steps=6 frames=2");
    let after = app.world().resource::<bevy::diagnostic::FrameCount>().0;

    assert_eq!(cursor(&mut app), Some(resting), "a rest moves nothing",);
    assert!(
        after - before >= 12,
        "six beats two frames apart is at least twelve frames: {before} -> {after}",
    );
    assert_eq!(
        hovered(&mut app),
        1,
        "and the hover it was resting on is still the hover",
    );
    let _ = node;
}
