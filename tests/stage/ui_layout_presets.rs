//! Layout presets: `ui.layout_preset name=<id>` and the row of buttons at
//! the top of the `Node` card.
//!
//! What is pinned here:
//!  * every preset writes the whole placement, so nothing is left over from
//!    the preset applied before it;
//!  * an anchor captures the size layout measured, so a node that states no
//!    size of its own is placed rather than stretched;
//!  * Full Rect is the one preset that drops the size, and the anchor after
//!    it captures what the stretch measured;
//!  * a preset that takes a node out of its parent's flow says so;
//!  * one press is one history entry that undoes to the exact `Node`;
//!  * the card's row offers all eleven and each button dispatches its own.

use crate::util;

use bevy::{prelude::*, ui::ComputedNode, ui_widgets::Activate};
use jackdaw::boot_ops::run_op_clause_as_user;
use jackdaw::commands::CommandHistory;
use jackdaw::selection::Selection;
use jackdaw::status_bar::StatusNotice;
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

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

/// A 2D panel, which is what gives an authored scene a target to be laid
/// out against.
fn panel(app: &mut App) {
    use jackdaw::viewport_2d::{Viewport2dPanelHost, build_viewport_2d_panel};
    let parent = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                width: px(800.0 + jackdaw::viewport_2d::RULER_SIZE),
                height: px(600.0
                    + jackdaw::viewport_2d::RULER_SIZE
                    + jackdaw_feathers::tokens::TOOLBAR_HEIGHT),
                ..default()
            },
        ))
        .id();
    build_viewport_2d_panel(app.world_mut(), parent);
    let mut host = app
        .world_mut()
        .get_mut::<Viewport2dPanelHost>(parent)
        .expect("host on panel parent");
    host.view.zoom = 1.0;
    host.fit_pending = false;
}

/// The reference resolution [`jackdaw::ui_palette::seed_ui_scene_root`]
/// gives the root it seeds, and so the size of the canvas box every
/// absolute preset resolves against.
const SEEDED_REFERENCE: Vec2 = Vec2::new(1280.0, 720.0);

/// The scene `scene.new kind=ui` makes: the root through the one function
/// that seeds it, holding a flow child that states no size of its own.
///
/// The geometry tests are built on this rather than on a root of a stated
/// size, because the placement a preset writes only means anything against
/// the box the real root is: a root that shrank to fit its content would
/// let every one of these assertions pass while nothing moved on the
/// canvas.
fn seeded_scene(app: &mut App) -> (Entity, Entity) {
    let root = jackdaw::ui_palette::seed_ui_scene_root(app.world_mut());
    let panel_entity = app
        .world_mut()
        .spawn((Name::new("Panel"), Node::default(), ChildOf(root)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), panel_entity);
    app.world_mut().spawn((
        Name::new("Filler"),
        Node {
            width: px(100.0),
            height: px(40.0),
            ..default()
        },
        ChildOf(panel_entity),
    ));

    settle(app);
    jackdaw::selection::select_only(app.world_mut(), panel_entity);
    settle(app);
    (root, panel_entity)
}

fn computed_size(app: &App, entity: Entity) -> Vec2 {
    app.world()
        .get::<ComputedNode>(entity)
        .expect("a laid-out node")
        .size()
}

fn centre_of(app: &App, entity: Entity) -> Vec2 {
    app.world()
        .get::<bevy::ui::UiGlobalTransform>(entity)
        .expect("a laid-out node")
        .translation
}

/// Where `entity`'s top-left corner sits inside `root`'s.
fn top_left_in_root(app: &App, entity: Entity, root: Entity) -> Vec2 {
    let corner = |entity| centre_of(app, entity) - computed_size(app, entity) / 2.0;
    corner(entity) - corner(root)
}

/// An anchor keeps the node the size it is. A node that states no size has
/// only the size layout gave it, so the preset writes that down before it
/// states the offsets: without it the two offsets of Middle Center would
/// stretch the node over the parent instead of putting it in the middle.
#[test]
fn middle_center_centres_an_auto_sized_panel_instead_of_stretching_it() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let (root, panel_entity) = seeded_scene(&mut app);
    assert_eq!(
        computed_size(&app, root),
        SEEDED_REFERENCE,
        "the seeded root is the canvas box the preset resolves against",
    );
    assert_eq!(
        computed_size(&app, panel_entity),
        Vec2::new(100.0, 40.0),
        "the fixture's panel is the size of its content",
    );

    run_finished(&mut app, "ui.layout_preset name=middle_center");
    settle(&mut app);

    let node = node_of(&app, panel_entity);
    assert_eq!(
        (node.width, node.height),
        (px(100.0), px(40.0)),
        "the preset captured the size layout measured",
    );
    assert_eq!(
        computed_size(&app, panel_entity),
        Vec2::new(100.0, 40.0),
        "and the node is still that size once it is placed",
    );
    assert!(
        (centre_of(&app, panel_entity) - centre_of(&app, root)).length() < 0.5,
        "a centred node sits at the middle of its parent, not over all of it",
    );
    // The two centres coincide however small the root is, so the distance
    // the node actually travelled is what says the containing block was the
    // canvas: half the canvas, less half the node.
    assert!(
        (top_left_in_root(&app, panel_entity, root)
            - (SEEDED_REFERENCE - Vec2::new(100.0, 40.0)) / 2.0)
            .length()
            < 0.5,
        "and that middle is the middle of the canvas, not of a shrunken root",
    );
}

/// Full Rect means "be the size of the parent", so it is the one preset
/// that drops a size rather than keeping it. The anchor pressed after it
/// captures what that stretch measured, so the node stays the size it is
/// on screen and moves to the corner.
///
/// On the scene `scene.new kind=ui` seeds, "the parent" is the canvas, so
/// this also pins what Full Rect is for: filling the reference resolution.
#[test]
fn full_rect_drops_the_size_and_the_next_anchor_captures_the_stretch() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let (root, panel_entity) = seeded_scene(&mut app);

    run_finished(&mut app, "ui.layout_preset name=full_rect");
    settle(&mut app);
    let node = node_of(&app, panel_entity);
    assert_eq!(
        (node.width, node.height),
        (Val::Auto, Val::Auto),
        "full rect states no size of its own",
    );
    assert_eq!(
        computed_size(&app, panel_entity),
        SEEDED_REFERENCE,
        "so it stretches over the whole canvas",
    );
    assert_eq!(
        top_left_in_root(&app, panel_entity, root),
        Vec2::ZERO,
        "starting at the canvas corner",
    );

    run_finished(&mut app, "ui.layout_preset name=top_left");
    settle(&mut app);
    let node = node_of(&app, panel_entity);
    assert_eq!(
        (node.width, node.height),
        (px(SEEDED_REFERENCE.x), px(SEEDED_REFERENCE.y)),
        "the anchor captured the size the stretch had reached",
    );
}

/// An anchor on the far side of the canvas is the case that shows a
/// shrunken containing block for what it is: against the real canvas the
/// node keeps its size and travels the width of the reference, and against
/// a root the size of its own content it would have nowhere to go.
#[test]
fn bottom_right_puts_the_node_in_the_far_corner_of_the_canvas() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let (root, panel_entity) = seeded_scene(&mut app);

    run_finished(&mut app, "ui.layout_preset name=bottom_right");
    settle(&mut app);

    let size = Vec2::new(100.0, 40.0);
    assert_eq!(
        computed_size(&app, panel_entity),
        size,
        "an anchor keeps the node the size it is",
    );
    assert_eq!(
        top_left_in_root(&app, panel_entity, root),
        SEEDED_REFERENCE - size,
        "and puts it against the canvas's bottom-right corner",
    );
}

/// A preset is a placement, so it may take a node out of its parent's
/// flow. The nudge refuses that; a preset does it and says so, rather than
/// leaving the change for the user to find in the Position field.
#[test]
fn a_preset_that_places_a_flowed_child_absolutely_says_so() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let (_root, panel_entity) = seeded_scene(&mut app);
    assert_eq!(
        node_of(&app, panel_entity).position_type,
        PositionType::Relative,
        "the fixture's panel starts in its parent's flow",
    );

    run_finished(&mut app, "ui.layout_preset name=middle_center");
    settle(&mut app);

    let notice = app.world().resource::<StatusNotice>();
    assert!(notice.is_active(), "the promotion is announced");
    assert_eq!(
        notice.text(),
        "Panel is now placed absolutely",
        "the notice names the node it moved",
    );
}

/// Center In Flow leaves the node where its parent puts it, so there is
/// nothing to announce.
#[test]
fn center_in_flow_promotes_nothing_and_says_nothing() {
    let mut app = util::editor_test_app();
    panel(&mut app);
    let (_root, panel_entity) = seeded_scene(&mut app);

    run_finished(&mut app, "ui.layout_preset name=center");
    settle(&mut app);

    assert_eq!(
        node_of(&app, panel_entity).position_type,
        PositionType::Relative,
    );
    assert!(
        !app.world().resource::<StatusNotice>().is_active(),
        "nothing was taken out of the flow, so nothing is announced",
    );
}
