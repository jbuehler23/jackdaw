//! Layout presets: `ui.layout_preset name=<id>` and the row of buttons at
//! the top of the `Node` card.
//!
//! What is pinned here:
//!  * every preset writes the whole placement, so nothing is left over from
//!    the preset applied before it;
//!  * one press is one history entry that undoes to the exact `Node`;
//!  * the card's row offers all eleven and each button dispatches its own.

use crate::util;

use bevy::{prelude::*, ui_widgets::Activate};
use jackdaw::boot_ops::run_op_clause_as_user;
use jackdaw::commands::CommandHistory;
use jackdaw::selection::Selection;
use jackdaw::ui_layout_presets::{LAYOUT_PRESET_OP, LayoutPresetRow, presets, spawn_preset_row};
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::ButtonOperatorCall;

#[track_caller]
fn run(app: &mut App, clause: &str) -> OperatorResult {
    let result = run_op_clause_as_user(app.world_mut(), clause)
        .unwrap_or_else(|err| panic!("{clause}: dispatch errored: {err}"));
    app.update();
    app.update();
    result
}

/// Run one clause the way a chord runs it.
///
/// `creates_history_entry`, which a scripted call leaves off, is what makes
/// the dispatcher open a snapshot span: an operator that records its own entry
/// and one that leaves the entry to the snapshot are only told apart under a
/// press, and this suite counts entries.
#[track_caller]
fn run_finished(app: &mut App, clause: &str) {
    let result = run(app, clause);
    assert_eq!(
        result,
        OperatorResult::Finished,
        "{clause} reported {result:?}"
    );
}

/// One selected node carrying an offset and a size a preset has to write over.
fn selected_node(app: &mut App) -> Entity {
    let entity = app
        .world_mut()
        .spawn((
            Name::new("Panel"),
            Node {
                position_type: PositionType::Relative,
                left: px(17.0),
                top: px(23.0),
                right: px(31.0),
                bottom: px(37.0),
                width: px(120.0),
                height: px(60.0),
                margin: UiRect::all(px(9.0)),
                ..default()
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    jackdaw::selection::select_only(app.world_mut(), entity);
    app.update();
    entity
}

fn node_of(app: &App, entity: Entity) -> Node {
    app.world().get::<Node>(entity).cloned().expect("a node")
}

fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

#[test]
fn every_preset_writes_the_whole_placement() {
    let auto = Val::Auto;
    let zero = px(0.0);
    // (id, position_type, left, right, top, bottom, margin)
    let expected: [(&str, PositionType, Val, Val, Val, Val, UiRect); 11] = [
        (
            "top_left",
            PositionType::Absolute,
            zero,
            auto,
            zero,
            auto,
            UiRect::all(zero),
        ),
        (
            "top_center",
            PositionType::Absolute,
            zero,
            zero,
            zero,
            auto,
            UiRect {
                left: auto,
                right: auto,
                top: zero,
                bottom: zero,
            },
        ),
        (
            "top_right",
            PositionType::Absolute,
            auto,
            zero,
            zero,
            auto,
            UiRect::all(zero),
        ),
        (
            "center_left",
            PositionType::Absolute,
            zero,
            auto,
            zero,
            zero,
            UiRect {
                left: zero,
                right: zero,
                top: auto,
                bottom: auto,
            },
        ),
        (
            "middle_center",
            PositionType::Absolute,
            zero,
            zero,
            zero,
            zero,
            UiRect::all(auto),
        ),
        (
            "center_right",
            PositionType::Absolute,
            auto,
            zero,
            zero,
            zero,
            UiRect {
                left: zero,
                right: zero,
                top: auto,
                bottom: auto,
            },
        ),
        (
            "bottom_left",
            PositionType::Absolute,
            zero,
            auto,
            auto,
            zero,
            UiRect::all(zero),
        ),
        (
            "bottom_center",
            PositionType::Absolute,
            zero,
            zero,
            auto,
            zero,
            UiRect {
                left: auto,
                right: auto,
                top: zero,
                bottom: zero,
            },
        ),
        (
            "bottom_right",
            PositionType::Absolute,
            auto,
            zero,
            auto,
            zero,
            UiRect::all(zero),
        ),
        (
            "full_rect",
            PositionType::Absolute,
            zero,
            zero,
            zero,
            zero,
            UiRect::all(zero),
        ),
        (
            "center",
            PositionType::Relative,
            auto,
            auto,
            auto,
            auto,
            UiRect::all(auto),
        ),
    ];

    let mut app = util::editor_test_app();
    let entity = selected_node(&mut app);

    for (id, position_type, left, right, top, bottom, margin) in expected {
        run_finished(&mut app, &format!("ui.layout_preset name={id}"));
        let node = node_of(&app, entity);
        assert_eq!(node.position_type, position_type, "{id}: position_type");
        assert_eq!(
            (node.left, node.right, node.top, node.bottom),
            (left, right, top, bottom),
            "{id}: offsets"
        );
        assert_eq!(node.margin, margin, "{id}: margin");
        if id == "full_rect" {
            assert_eq!(
                (node.width, node.height),
                (auto, auto),
                "full rect stretches, so it states no size of its own"
            );
        }
    }
}

#[test]
fn a_preset_is_one_undo_entry() {
    let mut app = util::editor_test_app();
    let entity = selected_node(&mut app);
    let before = node_of(&app, entity);
    let depth = undo_depth(&app);

    run_finished(&mut app, "ui.layout_preset name=bottom_right");
    assert_eq!(
        undo_depth(&app) - depth,
        1,
        "one preset press is one history entry"
    );
    assert_ne!(node_of(&app, entity), before);

    run_finished(&mut app, "history.undo");
    assert_eq!(
        node_of(&app, entity),
        before,
        "undo put the exact node back"
    );
}

#[test]
fn an_unknown_preset_is_refused() {
    let mut app = util::editor_test_app();
    let entity = selected_node(&mut app);
    let before = node_of(&app, entity);

    assert_eq!(
        run(&mut app, "ui.layout_preset name=upside_down"),
        OperatorResult::Cancelled,
    );
    assert_eq!(node_of(&app, entity), before);
}

#[test]
fn the_card_row_offers_every_preset_and_dispatches_it() {
    let mut app = util::editor_test_app();
    let entity = selected_node(&mut app);

    let host = app.world_mut().spawn(Node::default()).id();
    let font = app
        .world()
        .get_resource::<jackdaw_feathers::icons::IconFont>()
        .map(|font| font.0.clone())
        .unwrap_or_default();
    app.world_mut().commands().queue(move |world: &mut World| {
        let mut state: bevy::ecs::system::SystemState<Commands> =
            bevy::ecs::system::SystemState::new(world);
        if let Ok(mut commands) = state.get_mut(world) {
            spawn_preset_row(&mut commands, host, &font);
        }
        state.apply(world);
    });
    app.update();
    app.update();

    assert_eq!(
        app.world_mut()
            .query::<&LayoutPresetRow>()
            .iter(app.world())
            .count(),
        1,
        "the row is there"
    );

    let mut offered: Vec<(Entity, String)> = app
        .world_mut()
        .query::<(Entity, &ButtonOperatorCall)>()
        .iter(app.world())
        .filter(|(_, call)| call.id == LAYOUT_PRESET_OP)
        .filter_map(|(entity, call)| {
            call.params
                .iter()
                .find(|(key, _)| key == "name")
                // A `PropertyValue` prints a string in quotes; the preset id
                // is what is inside them.
                .map(|(_, value)| (entity, value.to_string().trim_matches('"').to_string()))
        })
        .collect();
    offered.sort_by(|left, right| left.1.cmp(&right.1));

    let mut wanted: Vec<String> = presets().map(|preset| preset.id.to_string()).collect();
    wanted.sort();
    assert_eq!(
        offered.iter().map(|(_, id)| id.clone()).collect::<Vec<_>>(),
        wanted,
        "the row offers every preset once"
    );

    // And the buttons are wired: activating one puts the node where it says.
    let (button, _) = offered
        .iter()
        .find(|(_, id)| id == "bottom_right")
        .expect("the bottom right button");
    app.world_mut().trigger(Activate { entity: *button });
    app.update();
    app.update();

    let node = node_of(&app, entity);
    assert_eq!(
        (node.left, node.right, node.top, node.bottom),
        (Val::Auto, px(0.0), Val::Auto, px(0.0)),
        "the button dispatched its own preset"
    );
    assert!(
        app.world()
            .resource::<Selection>()
            .entities
            .contains(&entity),
        "the press acted on the selection"
    );
}
