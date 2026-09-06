//! The rubber band a drag from bare canvas pulls out.

use crate::util;

use bevy::{
    camera::{NormalizedRenderTarget, RenderTarget},
    input::{
        ButtonState,
        keyboard::{Key, KeyboardInput},
    },
    picking::{
        backend::HitData,
        events::{Drag, DragEnd, DragStart, Pointer, Press},
        pointer::{Location, PointerButton, PointerId},
    },
    prelude::*,
    ui::ComputedNode,
    window::{PrimaryWindow, WindowRef},
};
use jackdaw::selection::Selection;
use jackdaw::ui_stage::{MarqueeOverlay, MarqueeSelect};
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

/// Two boxes with a gap between them, so a band can take one, the other,
/// or both.
fn two_boxes(app: &mut App) -> (Entity, Entity, Entity) {
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
    let left = child(app, root, "Left", 100.0, 100.0, 200.0, 200.0);
    let right = child(app, root, "Right", 700.0, 100.0, 200.0, 200.0);
    settle(app);
    (root, left, right)
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
    app.world_mut()
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
        .id()
}

fn stage_of(app: &App, panel: Entity) -> Entity {
    app.world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .stage
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

fn pointer_at<E: std::fmt::Debug + Clone + Reflect>(
    app: &mut App,
    target: Entity,
    position: Vec2,
    event: E,
) {
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
        event,
        target,
    ));
}

/// The whole band gesture: press, drag, release, in authored pixels.
fn band(app: &mut App, panel: Entity, from: Vec2, to: Vec2) {
    band_to(app, panel, from, to);
    let stage = stage_of(app, panel);
    let start = screen_position_of(app, panel, from);
    let distance = screen_position_of(app, panel, to) - start;
    pointer_at(
        app,
        stage,
        start + distance,
        DragEnd {
            button: PointerButton::Primary,
            distance,
        },
    );
    settle(app);
}

/// The band up to but not including the release, so a test can look at
/// what is drawn while it is still being pulled out.
fn band_to(app: &mut App, panel: Entity, from: Vec2, to: Vec2) {
    let stage = stage_of(app, panel);
    let camera = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .camera;
    let start = screen_position_of(app, panel, from);
    pointer_at(
        app,
        stage,
        start,
        Press {
            button: PointerButton::Primary,
            hit: HitData::new(camera, 0.0, None, None),
            count: 1,
        },
    );
    settle(app);
    pointer_at(
        app,
        stage,
        start,
        DragStart {
            button: PointerButton::Primary,
            hit: HitData::new(camera, 0.0, None, None),
        },
    );
    settle(app);
    let distance = screen_position_of(app, panel, to) - start;
    pointer_at(
        app,
        stage,
        start + distance,
        Drag {
            button: PointerButton::Primary,
            distance,
            delta: distance,
        },
    );
    settle(app);
}

fn selection(app: &App) -> Vec<Entity> {
    app.world().resource::<Selection>().entities.clone()
}

fn hold(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(key);
}

fn release(app: &mut App, key: KeyCode) {
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(key);
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
}

#[test]
fn a_band_takes_every_node_it_is_pulled_across() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (_root, left, right) = two_boxes(&mut app);

    band(
        &mut app,
        panel,
        Vec2::new(50.0, 50.0),
        Vec2::new(400.0, 400.0),
    );
    assert_eq!(
        selection(&app),
        vec![left],
        "the band took what it crossed and left what it did not",
    );

    band(
        &mut app,
        panel,
        Vec2::new(50.0, 50.0),
        Vec2::new(1000.0, 400.0),
    );
    let mut both = selection(&app);
    both.sort();
    let mut wanted = vec![left, right];
    wanted.sort();
    assert_eq!(both, wanted, "a wider band takes both");

    band(
        &mut app,
        panel,
        Vec2::new(350.0, 50.0),
        Vec2::new(250.0, 250.0),
    );
    assert_eq!(
        selection(&app),
        vec![left],
        "a band that clips a corner takes the node; it does not have to contain it",
    );
}

/// A press on the background is a press on a node, so a screen built on a
/// full-rect panel has nowhere left to start a band. Locked, the panel
/// contributes no hit at all and the press falls through to the canvas.
#[test]
fn a_locked_background_lets_a_band_be_pulled_out_over_it() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (root, left, right) = two_boxes(&mut app);
    // Last, so it is painted over the two boxes: the background a screen is
    // built on, and the thing every press lands on.
    let background = child(&mut app, root, "Background", 0.0, 0.0, 2400.0, 1200.0);
    settle(&mut app);

    band(
        &mut app,
        panel,
        Vec2::new(50.0, 50.0),
        Vec2::new(950.0, 400.0),
    );
    assert_eq!(
        selection(&app),
        vec![background],
        "over an unlocked background the press is that panel's own, so no band comes out",
    );

    jackdaw::hierarchy::set_locked(app.world_mut(), background, true);
    settle(&mut app);
    jackdaw::selection::clear_selection_in_world(app.world_mut());
    settle(&mut app);

    band(
        &mut app,
        panel,
        Vec2::new(50.0, 50.0),
        Vec2::new(950.0, 400.0),
    );
    let mut caught = selection(&app);
    caught.sort();
    let mut wanted = vec![left, right];
    wanted.sort();
    assert_eq!(
        caught, wanted,
        "locked, it lets the press through and the band takes the two nodes over it",
    );
    assert!(
        !selection(&app).contains(&background),
        "and the locked panel is not itself taken",
    );
}

#[test]
fn a_band_that_crosses_nothing_empties_the_selection() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (_root, left, _right) = two_boxes(&mut app);
    jackdaw::selection::select_only(app.world_mut(), left);
    settle(&mut app);

    band(
        &mut app,
        panel,
        Vec2::new(400.0, 500.0),
        Vec2::new(600.0, 550.0),
    );

    assert!(
        selection(&app).is_empty(),
        "a band over bare canvas is a selection of nothing",
    );
}

#[test]
fn shift_adds_the_bands_catch_and_ctrl_toggles_it() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (_root, left, right) = two_boxes(&mut app);

    jackdaw::selection::select_only(app.world_mut(), right);
    settle(&mut app);
    hold(&mut app, KeyCode::ShiftLeft);
    band(
        &mut app,
        panel,
        Vec2::new(50.0, 50.0),
        Vec2::new(400.0, 400.0),
    );
    release(&mut app, KeyCode::ShiftLeft);
    let mut added = selection(&app);
    added.sort();
    let mut wanted = vec![left, right];
    wanted.sort();
    assert_eq!(added, wanted, "Shift keeps what was selected and adds");

    hold(&mut app, KeyCode::ControlLeft);
    band(
        &mut app,
        panel,
        Vec2::new(50.0, 50.0),
        Vec2::new(400.0, 400.0),
    );
    release(&mut app, KeyCode::ControlLeft);
    assert_eq!(
        selection(&app),
        vec![right],
        "Ctrl takes back out what the band crossed and was already in",
    );
}

#[test]
fn a_drag_that_starts_on_a_node_moves_it_instead_of_banding() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (_root, left, _right) = two_boxes(&mut app);

    band_to(
        &mut app,
        panel,
        Vec2::new(200.0, 200.0),
        Vec2::new(400.0, 400.0),
    );

    assert!(
        app.world().resource::<MarqueeSelect>().corners().is_none(),
        "a press on a node is that node's gesture, not a band",
    );
    assert_eq!(
        selection(&app),
        vec![left],
        "the press selected the node it landed on",
    );
}

#[test]
fn the_band_is_drawn_over_the_stage_where_the_drag_put_it() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    two_boxes(&mut app);

    band_to(
        &mut app,
        panel,
        Vec2::new(400.0, 500.0),
        Vec2::new(600.0, 560.0),
    );

    let bands: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<MarqueeOverlay>>()
        .iter(app.world())
        .collect();
    assert_eq!(bands.len(), 1, "one band per canvas being swept");
    let node = app
        .world()
        .get::<Node>(bands[0])
        .expect("the band is a node");
    // Half a stage pixel per authored pixel at this zoom.
    assert_eq!(
        (node.left, node.top, node.width, node.height),
        (px(200.0), px(250.0), px(100.0), px(30.0)),
        "the band is drawn on the canvas pixels the drag swept",
    );
}

/// Once the backdrop has been clicked its own outline covers the whole canvas,
/// so the next drag is delivered to that outline rather than to the stage.
#[test]
fn a_drag_on_the_backdrops_own_outline_is_still_a_band() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (root, left, _right) = two_boxes(&mut app);
    jackdaw::selection::select_only(app.world_mut(), root);
    settle(&mut app);

    let outline = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::ui_stage::UiSelectionOverlay>>()
        .iter(app.world())
        .next()
        .expect("the backdrop carries an outline");
    let start = screen_position_of(&mut app, panel, Vec2::new(50.0, 50.0));
    let camera = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .camera;
    pointer_at(
        &mut app,
        outline,
        start,
        DragStart {
            button: PointerButton::Primary,
            hit: HitData::new(camera, 0.0, None, None),
        },
    );
    settle(&mut app);
    let distance = screen_position_of(&mut app, panel, Vec2::new(400.0, 400.0)) - start;
    pointer_at(
        &mut app,
        outline,
        start + distance,
        Drag {
            button: PointerButton::Primary,
            distance,
            delta: distance,
        },
    );
    settle(&mut app);
    pointer_at(
        &mut app,
        outline,
        start + distance,
        DragEnd {
            button: PointerButton::Primary,
            distance,
        },
    );
    settle(&mut app);

    assert_eq!(
        selection(&app),
        vec![left],
        "the drag banded across the canvas rather than moving the backdrop",
    );
    assert_eq!(
        app.world().get::<Node>(root).expect("a node").left,
        Val::Auto,
        "the backdrop did not move",
    );
}

#[test]
fn escape_drops_the_band_and_leaves_the_selection_alone() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (root, left, right) = two_boxes(&mut app);
    jackdaw::selection::select_only(app.world_mut(), right);
    settle(&mut app);

    band_to(
        &mut app,
        panel,
        Vec2::new(50.0, 50.0),
        Vec2::new(400.0, 400.0),
    );
    press_escape(&mut app);
    settle(&mut app);

    assert!(
        app.world().resource::<MarqueeSelect>().corners().is_none(),
        "Escape drops the band",
    );
    assert!(
        app.world_mut()
            .query_filtered::<Entity, With<MarqueeOverlay>>()
            .iter(app.world())
            .next()
            .is_none(),
        "and takes what was drawn with it",
    );
    assert_eq!(
        selection(&app),
        vec![root],
        "the selection is what the press left, not what the band was over",
    );
    let _ = (left, right);
}
