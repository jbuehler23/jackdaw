//! The label a canvas gesture draws beside the node it is dragging.

use crate::util;

use bevy::{
    camera::{NormalizedRenderTarget, RenderTarget},
    picking::{
        backend::HitData,
        events::{Drag, DragEnd, DragStart, Pointer},
        pointer::{Location, PointerButton, PointerId},
    },
    prelude::*,
    ui::ComputedNode,
    window::{PrimaryWindow, WindowRef},
};
use jackdaw::selection::Selection;
use jackdaw::ui_stage::{
    DragReadout, DragReadoutMeasure, DragReadoutSpacing, UiResizeHandle, UiSelectionOverlay,
};
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

/// A node to drag, and a sibling 100 authored pixels to its right for the
/// gap readout to find.
fn scene(app: &mut App) -> (Entity, Entity) {
    let root = app
        .world_mut()
        .spawn((
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
    let dragged = child(app, root, 200.0, 100.0, 200.0, 100.0);
    let sibling = child(app, root, 500.0, 100.0, 200.0, 100.0);
    settle(app);
    (dragged, sibling)
}

fn child(app: &mut App, root: Entity, left: f32, top: f32, width: f32, height: f32) -> Entity {
    app.world_mut()
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(left),
                top: px(top),
                width: px(width),
                height: px(height),
                ..default()
            },
            ChildOf(root),
        ))
        .id()
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

fn overlay(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<UiSelectionOverlay>>()
        .iter(app.world())
        .next()
        .expect("the selected node carries an outline")
}

fn handle(app: &mut App, overlay: Entity, want: (i8, i8)) -> Entity {
    let world = app.world();
    world
        .get::<Children>(overlay)
        .into_iter()
        .flatten()
        .find(|&&child| {
            world
                .get::<UiResizeHandle>(child)
                .is_some_and(|handle| (handle.x, handle.y) == want)
        })
        .copied()
        .expect("the outline carries the handle")
}

/// Press and drag `target` from one authored point to another, leaving the
/// button down.
fn drag_to(app: &mut App, panel: Entity, target: Entity, from: Vec2, to: Vec2) -> (Vec2, Vec2) {
    let camera = app
        .world()
        .get::<Viewport2dPanelHost>(panel)
        .expect("host on panel parent")
        .camera;
    let start = screen_position_of(app, panel, from);
    pointer_at(
        app,
        target,
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
        target,
        start + distance,
        Drag {
            button: PointerButton::Primary,
            distance,
            delta: distance,
        },
    );
    settle(app);
    (start, distance)
}

fn release(app: &mut App, target: Entity, start: Vec2, distance: Vec2) {
    pointer_at(
        app,
        target,
        start + distance,
        DragEnd {
            button: PointerButton::Primary,
            distance,
        },
    );
    settle(app);
}

/// The two lines the readout is drawing: the measure, then the spacing.
fn readout_lines(app: &mut App) -> Option<(String, String)> {
    let label = app
        .world_mut()
        .query_filtered::<Entity, With<DragReadout>>()
        .iter(app.world())
        .next()?;
    let world = app.world();
    let mut measure = String::new();
    let mut spacing = String::new();
    for child in world.get::<Children>(label).into_iter().flatten().copied() {
        let Some(text) = world.get::<Text>(child) else {
            continue;
        };
        if world.get::<DragReadoutMeasure>(child).is_some() {
            measure = text.0.clone();
        } else if world.get::<DragReadoutSpacing>(child).is_some() {
            spacing = text.0.clone();
        }
    }
    Some((measure, spacing))
}

fn select(app: &mut App, entity: Entity) {
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    settle(app);
}

/// The canvas magnet off, so the drag writes the cursor's own figures.
fn without_the_magnet(app: &mut App) {
    let mut kinds = app
        .world_mut()
        .resource_mut::<jackdaw::canvas_snap::CanvasSnap>();
    kinds.enabled = false;
}

#[test]
fn a_move_states_where_the_node_is() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (dragged, _sibling) = scene(&mut app);
    without_the_magnet(&mut app);
    select(&mut app, dragged);
    let outline = overlay(&mut app);

    let (start, distance) = drag_to(
        &mut app,
        panel,
        outline,
        Vec2::new(300.0, 150.0),
        Vec2::new(340.0, 170.0),
    );

    let (measure, _) = readout_lines(&mut app).expect("the gesture draws a readout");
    assert_eq!(
        measure, "240, 120",
        "the move states the node's authored left and top",
    );

    release(&mut app, outline, start, distance);
    assert!(
        readout_lines(&mut app).is_none(),
        "the readout goes with the gesture that drew it",
    );
}

#[test]
fn a_resize_states_the_size_instead() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (dragged, _sibling) = scene(&mut app);
    without_the_magnet(&mut app);
    select(&mut app, dragged);
    let outline = overlay(&mut app);
    let corner = handle(&mut app, outline, (1, 1));

    drag_to(
        &mut app,
        panel,
        corner,
        Vec2::new(400.0, 200.0),
        Vec2::new(450.0, 230.0),
    );

    let (measure, _) = readout_lines(&mut app).expect("the gesture draws a readout");
    assert_eq!(
        measure, "250 x 130",
        "a resize states what it is writing, which is the size",
    );
}

#[test]
fn the_spacing_line_states_the_gap_to_the_nearest_sibling_edge() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (dragged, _sibling) = scene(&mut app);
    without_the_magnet(&mut app);
    select(&mut app, dragged);
    let outline = overlay(&mut app);

    // Dragged 40 right: its far edge is at 440, and the sibling's near
    // edge is at 500, so the gap is 60.
    drag_to(
        &mut app,
        panel,
        outline,
        Vec2::new(300.0, 150.0),
        Vec2::new(340.0, 150.0),
    );

    let (_, spacing) = readout_lines(&mut app).expect("the gesture draws a readout");
    assert!(
        spacing.contains("x 60"),
        "the spacing says how far there is to the sibling's edge; got {spacing:?}",
    );
}

/// Taken as an absolute the two were the same figure: `x 30` read the same
/// whether there were thirty pixels of daylight left or thirty pixels of the
/// neighbour already covered.
#[test]
fn the_spacing_line_reads_negative_once_the_nodes_overlap() {
    let mut app = util::editor_test_app();
    let panel = panel(&mut app);
    let (dragged, _sibling) = scene(&mut app);
    without_the_magnet(&mut app);
    select(&mut app, dragged);
    let outline = overlay(&mut app);

    // Dragged 400 right: it spans 600..800 and the sibling spans 500..700,
    // so the sibling's near edge is 100 inside it.
    drag_to(
        &mut app,
        panel,
        outline,
        Vec2::new(300.0, 150.0),
        Vec2::new(700.0, 150.0),
    );

    let (_, spacing) = readout_lines(&mut app).expect("the gesture draws a readout");
    assert!(
        spacing.contains("x -100"),
        "the spacing says how far the nodes have run into each other; got {spacing:?}",
    );
}
