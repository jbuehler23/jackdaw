//! Editing a node's text where it is drawn.
//!
//! What is pinned here:
//!  * a double click on a node carrying text opens an entry over its rect,
//!    seeded with what the node says;
//!  * a double click on a node with no text opens nothing;
//!  * Enter commits through the field path, as one history entry that
//!    undoes back to the text that was there;
//!  * Escape puts the entry away and writes nothing;
//!  * an entry dismissed without a change writes nothing either;
//!  * the entry follows the canvas, so a zoom moves it with the node.

use crate::util;

use bevy::{
    camera::{NormalizedRenderTarget, RenderTarget},
    input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
    },
    picking::{
        backend::HitData,
        events::{Pointer, Press},
        pointer::{Location, PointerButton, PointerId},
    },
    prelude::*,
    text::EditableText,
    ui::ComputedNode,
    window::{PrimaryWindow, WindowRef},
};
use jackdaw::commands::CommandHistory;
use jackdaw::ui_text_edit::{TextEditOverlay, TextEditSession};
use jackdaw::viewport_2d::{Viewport2dPanelHost, build_viewport_2d_panel};
use jackdaw_feathers::tokens::TOOLBAR_HEIGHT;
use jackdaw_scene_types::UiSceneRoot;

const REFERENCE: UVec2 = UVec2::new(2400, 1200);

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn panel(app: &mut App) -> Entity {
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
    parent
}

/// A label and a plain box, so a double click can be aimed at each.
fn scene(app: &mut App) -> (Entity, Entity) {
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
                ..default()
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), root);
    let label = app
        .world_mut()
        .spawn((
            Name::new("Label"),
            Node {
                position_type: PositionType::Absolute,
                left: px(200.0),
                top: px(100.0),
                width: px(400.0),
                height: px(200.0),
                ..default()
            },
            Text::new("Before"),
            ChildOf(root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), label);
    let plain = app
        .world_mut()
        .spawn((
            Name::new("Plain"),
            Node {
                position_type: PositionType::Absolute,
                left: px(800.0),
                top: px(100.0),
                width: px(200.0),
                height: px(200.0),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), plain);
    settle(app);
    (label, plain)
}

fn screen_position_of(app: &mut App, panel: Entity, authored: Vec2) -> Vec2 {
    let (area, view, target_size) = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .map(|host| (host.area, host.view, host.target_size))
        .expect("host on panel parent");
    let computed = *app
        .world()
        .get::<ComputedNode>(area)
        .expect("the stage area is laid out");
    let centre = app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(area)
        .expect("the stage area is laid out")
        .translation;
    let focus = target_size.as_vec2() / 2.0 + Vec2::new(view.pan.x, -view.pan.y);
    let area_centre_logical = centre * computed.inverse_scale_factor();
    let logical = area_centre_logical + (authored - focus) * view.zoom;
    logical * app.world().resource::<UiScale>().0
}

/// Press at the point on screen showing `authored`, with `count` clicks.
fn press_at(app: &mut App, panel: Entity, authored: Vec2, count: u8) {
    let (stage, camera) = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .map(|host| (host.stage, host.camera))
        .expect("host on panel parent");
    let position = screen_position_of(app, panel, authored);
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    let render_target: NormalizedRenderTarget = RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target: render_target,
            position,
        },
        Press {
            button: PointerButton::Primary,
            hit: HitData::new(camera, 0.0, None, None),
            count,
        },
        stage,
    ));
    settle(app);
}

fn open_entry(app: &mut App, panel: Entity, authored: Vec2) {
    press_at(app, panel, authored, 1);
    press_at(app, panel, authored, 2);
}

/// The entry's editable buffer, if one is open.
fn entry(app: &mut App) -> Option<Entity> {
    let overlay = app
        .world_mut()
        .query_filtered::<Entity, With<TextEditOverlay>>()
        .iter(app.world())
        .next()?;
    let world = app.world();
    fn find(world: &World, entity: Entity) -> Option<Entity> {
        if world.get::<EditableText>(entity).is_some() {
            return Some(entity);
        }
        world
            .get::<Children>(entity)
            .into_iter()
            .flatten()
            .copied()
            .find_map(|child| find(world, child))
    }
    find(world, overlay)
}

fn type_into_entry(app: &mut App, text: &str) {
    let entry = entry(app).expect("an entry is open");
    app.world_mut().trigger(bevy::ui_widgets::ValueChange {
        source: entry,
        value: text.to_string(),
        is_final: true,
    });
    settle(app);
}

fn press_escape(app: &mut App) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    app.world_mut().write_message(KeyboardInput {
        key_code: KeyCode::Escape,
        logical_key: Key::Escape,
        state: ButtonState::Pressed,
        text: None,
        repeat: false,
        window,
    });
    app.update();
    app.update();
}

fn text_of(app: &App, entity: Entity) -> String {
    app.world()
        .get::<Text>(entity)
        .map(|text| text.0.clone())
        .unwrap_or_default()
}

fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

#[test]
fn a_double_click_on_text_opens_an_entry_over_it() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (label, _plain) = scene(&mut app);

    open_entry(&mut app, panel, Vec2::new(400.0, 200.0));

    assert_eq!(
        app.world().resource::<TextEditSession>().editing(),
        Some(label),
        "the entry is open over the node that was clicked",
    );
    let overlay = app
        .world_mut()
        .query_filtered::<Entity, With<TextEditOverlay>>()
        .iter(app.world())
        .next()
        .expect("the entry is drawn");
    let node = app.world().get::<Node>(overlay).expect("a node");
    // Half a stage pixel per authored pixel at this zoom.
    assert_eq!(
        (node.left, node.top, node.width, node.height),
        (px(100.0), px(50.0), px(200.0), px(100.0)),
        "the entry covers the node's own rect",
    );
    let entry = entry(&mut app).expect("the entry holds a buffer");
    assert_eq!(
        app.world()
            .get::<EditableText>(entry)
            .map(|editable| editable.value().to_string()),
        Some("Before".to_string()),
        "the entry is seeded with what the node says",
    );
}

#[test]
fn a_double_click_on_a_node_with_no_text_opens_nothing() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (_label, _plain) = scene(&mut app);

    open_entry(&mut app, panel, Vec2::new(900.0, 200.0));

    assert!(
        app.world()
            .resource::<TextEditSession>()
            .editing()
            .is_none(),
        "there is no text there to edit",
    );
}

#[test]
fn enter_commits_the_text_as_one_entry_that_undoes() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (label, _plain) = scene(&mut app);
    open_entry(&mut app, panel, Vec2::new(400.0, 200.0));
    let depth = undo_depth(&app);

    type_into_entry(&mut app, "After");

    assert_eq!(text_of(&app, label), "After", "the node took the new text");
    assert_eq!(undo_depth(&app) - depth, 1, "one edit is one history entry",);
    assert!(
        app.world()
            .resource::<TextEditSession>()
            .editing()
            .is_none(),
        "a commit closes the entry",
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    settle(&mut app);
    assert_eq!(
        text_of(&app, label),
        "Before",
        "undo puts back what the node said",
    );
}

#[test]
fn escape_puts_the_entry_away_and_writes_nothing() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (label, _plain) = scene(&mut app);
    open_entry(&mut app, panel, Vec2::new(400.0, 200.0));
    let depth = undo_depth(&app);

    press_escape(&mut app);
    settle(&mut app);

    assert!(
        app.world()
            .resource::<TextEditSession>()
            .editing()
            .is_none(),
        "Escape closes the entry",
    );
    assert_eq!(text_of(&app, label), "Before", "and leaves the node alone");
    assert_eq!(undo_depth(&app), depth, "a cancel records nothing");
}

#[test]
fn text_that_came_back_unchanged_records_nothing() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (label, _plain) = scene(&mut app);
    open_entry(&mut app, panel, Vec2::new(400.0, 200.0));
    let depth = undo_depth(&app);

    type_into_entry(&mut app, "Before");

    assert_eq!(text_of(&app, label), "Before");
    assert_eq!(
        undo_depth(&app),
        depth,
        "an entry opened and dismissed is not an edit",
    );
}

#[test]
fn the_entry_follows_the_canvas_it_is_drawn_on() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (_label, _plain) = scene(&mut app);
    open_entry(&mut app, panel, Vec2::new(400.0, 200.0));

    app.world_mut()
        .get_mut::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .view
        .zoom = 1.0;
    settle(&mut app);

    let overlay = app
        .world_mut()
        .query_filtered::<Entity, With<TextEditOverlay>>()
        .iter(app.world())
        .next()
        .expect("the entry is still drawn");
    let node = app.world().get::<Node>(overlay).expect("a node");
    assert_eq!(
        (node.left, node.top, node.width, node.height),
        (px(200.0), px(100.0), px(400.0), px(200.0)),
        "a zoom moves the entry with the node it is over",
    );
}

/// A commit writes the edited node and nothing else, and hands the
/// selection back the way it found it.
///
/// A field commit writes to every selected node, so a commit made with two
/// labels selected typed the same words onto both; naming only the edited
/// one instead collapsed the selection to it, and the pair the user had
/// lined up to align was gone by the time they looked up.
#[test]
fn a_commit_writes_one_node_and_leaves_the_selection_as_it_was() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (label, _plain) = scene(&mut app);
    let root = app
        .world()
        .get::<ChildOf>(label)
        .expect("the label has a parent")
        .parent();
    let other = app
        .world_mut()
        .spawn((
            Name::new("Other"),
            Node {
                position_type: PositionType::Absolute,
                left: px(200.0),
                top: px(400.0),
                width: px(400.0),
                height: px(200.0),
                ..default()
            },
            Text::new("Untouched"),
            ChildOf(root),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), other);
    settle(&mut app);

    open_entry(&mut app, panel, Vec2::new(400.0, 200.0));
    // The pair the user lined up while the entry was open, which the
    // commit is not asked to touch.
    jackdaw::selection::select_many(app.world_mut(), &[other, label]);
    settle(&mut app);
    let selected = app
        .world()
        .resource::<jackdaw::selection::Selection>()
        .entities
        .clone();

    type_into_entry(&mut app, "After");

    assert_eq!(text_of(&app, label), "After", "the edited node took it");
    assert_eq!(
        text_of(&app, other),
        "Untouched",
        "and the rest of the selection did not",
    );
    assert_eq!(
        app.world()
            .resource::<jackdaw::selection::Selection>()
            .entities,
        selected,
        "the selection the entry opened over is the selection it leaves behind",
    );
}
