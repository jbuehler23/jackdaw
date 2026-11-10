//! The shape editor of the collision mode and its operators: drawing,
//! moving and deleting the points of one tile's collision polygons, flips,
//! turns and flags, each change one undo entry.

use crate::operators::tileset_dock::{
    control, handmade, input, opened, press, screen_rect, settle,
};
use crate::operators::tileset_edit_ops::{
    call, collision_of, edit, entries, on_disk, overworld, refused, run,
};
use crate::operators::tileset_ops::warnings;

use bevy::input::ButtonState;
use bevy::input::mouse::MouseButtonInput;
use bevy::prelude::*;
use bevy::ui::ComputedNode;
use bevy::window::WindowEvent;
use jackdaw_commands::CommandHistory;
use jackdaw_editor_tile::tileset_dock::{DockAction, TilesetDockViews};
use jackdaw_editor_tile::tileset_modes::ModeId;
use jackdaw_editor_tile::tileset_shape::{
    ShapeCanvas, ShapeHandle, ShapePiece, ShapePopover, ShapeSnap, parse_points,
};
use jackdaw_feathers::tokens;
use jackdaw_tilemap::ShapeTurn;
use jackdaw_tileset::CollisionPreset;

fn ring(points: &[(f32, f32)]) -> Vec<Vec2> {
    points.iter().map(|&(x, y)| Vec2::new(x, y)).collect()
}

/// Take the traced shapes automatic setup gave `tiles` off them.
fn clear(app: &mut App, tiles: &str) {
    call(
        app,
        "tileset.collision.set",
        &[("tiles", tiles), ("preset", "none")],
    );
}

#[test]
fn set_polygon_adds_and_replaces_polygons_of_a_custom_shape() {
    let (mut app, dir) = overworld();
    clear(&mut app, "0:3");
    let notch = [
        ("tile", "0:3"),
        ("points", "0,0;0.5,0;0.5,0.5"),
        ("polygon", "new"),
    ];
    let (_, after) = edit(&mut app, &dir, "tileset.collision.set_polygon", &notch);
    let shapes = collision_of(&after, 3);
    assert_eq!(shapes.len(), 1);
    assert_eq!(shapes[0].preset, CollisionPreset::Custom);
    assert_eq!(
        shapes[0].polygons,
        [ring(&[(0.0, 0.0), (0.5, 0.0), (0.5, 0.5)])]
    );
    let described = after.describe().expect("describes");
    assert!(described.contains(", 1 custom"), "{described}");

    let clockwise = [
        ("tile", "0:3"),
        ("points", "0.5,0.5;1,1;1,0.5"),
        ("polygon", ""),
    ];
    let (_, after) = edit(&mut app, &dir, "tileset.collision.set_polygon", &clockwise);
    let second = &collision_of(&after, 3)[0].polygons[1];
    assert_eq!(
        second,
        &ring(&[(1.0, 0.5), (1.0, 1.0), (0.5, 0.5)]),
        "turned around"
    );

    let whole = [
        ("tile", "0:3"),
        ("points", "0,0;1,0;1,1;0,1"),
        ("polygon", "1"),
    ];
    edit(&mut app, &dir, "tileset.collision.set_polygon", &whole);
    let only = [("tile", "0:3"), ("polygon", "0")];
    let (_, after) = edit(&mut app, &dir, "tileset.collision.remove_polygon", &only);
    let shapes = collision_of(&after, 3);
    assert_eq!(shapes[0].polygons.len(), 1);
    assert_eq!(
        shapes[0].preset,
        CollisionPreset::Solid,
        "it draws a preset"
    );
}

#[test]
fn removing_the_last_polygon_clears_the_shape() {
    let (mut app, dir) = overworld();
    call(
        &mut app,
        "tileset.collision.set",
        &[("tiles", "0:4"), ("preset", "slope_up")],
    );
    let last = [("tile", "0:4"), ("polygon", "0")];
    let (_, after) = edit(&mut app, &dir, "tileset.collision.remove_polygon", &last);
    assert!(collision_of(&after, 4).is_empty());
    refused(
        &mut app,
        &dir,
        "tileset.collision.remove_polygon",
        &last,
        "no polygon 0",
    );
}

#[test]
fn transform_flips_and_turns_a_shape_and_keeps_its_flags() {
    let (mut app, dir) = overworld();
    let set = [
        ("tiles", "0:4"),
        ("preset", "slope_up"),
        ("trigger", "true"),
        ("one_way", "true"),
    ];
    call(&mut app, "tileset.collision.set", &set);
    let flip = [("tile", "0:4"), ("turn", "flip_x")];
    let (_, after) = edit(&mut app, &dir, "tileset.collision.transform", &flip);
    let shape = &collision_of(&after, 4)[0];
    assert_eq!(shape.preset, CollisionPreset::SlopeDown);
    assert!(shape.trigger && shape.one_way, "flags kept");
    let turn = [("tile", "0:4"), ("turn", "flip_y")];
    let (_, after) = edit(&mut app, &dir, "tileset.collision.transform", &turn);
    assert_eq!(collision_of(&after, 4)[0].preset, CollisionPreset::Custom);
    let rotate = [("tile", "0:4"), ("turn", "rotate")];
    edit(&mut app, &dir, "tileset.collision.transform", &rotate);
    let bad = [("tile", "0:4"), ("turn", "spin")];
    refused(
        &mut app,
        &dir,
        "tileset.collision.transform",
        &bad,
        "not `spin`",
    );
    clear(&mut app, "0:5");
    let empty = [("tile", "0:5"), ("turn", "rotate")];
    refused(
        &mut app,
        &dir,
        "tileset.collision.transform",
        &empty,
        "no shape",
    );
}

#[test]
fn set_with_custom_keeps_the_shape_and_sets_only_its_flags() {
    let (mut app, dir) = overworld();
    clear(&mut app, "0:3,0:4");
    let drawn = [("tile", "0:3"), ("points", "0,0;0.5,0;0.5,0.5")];
    call(&mut app, "tileset.collision.set_polygon", &drawn);
    let before = collision_of(&on_disk(&dir), 3)[0].clone();
    let flags = [
        ("tiles", "0:3,0:4"),
        ("preset", "custom"),
        ("one_way", "true"),
    ];
    let (_, after) = edit(&mut app, &dir, "tileset.collision.set", &flags);
    let shape = &collision_of(&after, 3)[0];
    assert_eq!(shape.polygons, before.polygons);
    assert_eq!(shape.preset, CollisionPreset::Custom);
    assert!(shape.one_way && !shape.trigger);
    assert!(collision_of(&after, 4).is_empty(), "no shape to keep");
}

#[test]
fn set_polygon_refuses_points_that_draw_no_polygon() {
    let (mut app, dir) = overworld();
    clear(&mut app, "0:3");
    let id = "tileset.collision.set_polygon";
    for (points, says) in [
        ("0,0;1,1", "at least three points"),
        ("0,0;1.5,0;1,1", "outside the tile"),
        ("0,0;0.5,0.5;1,1", "no area"),
        ("0,0;x,0;1,1", "no x,y point"),
    ] {
        refused(
            &mut app,
            &dir,
            id,
            &[("tile", "0:3"), ("points", points)],
            says,
        );
    }
    let far = [("tile", "0:3"), ("points", "0,0;1,0;1,1"), ("polygon", "2")];
    refused(&mut app, &dir, id, &far, "no polygon 2");
    let two = [("tile", "0:3,0:4"), ("points", "0,0;1,0;1,1")];
    refused(&mut app, &dir, id, &two, "one tile");
}

#[test]
fn written_points_read_back_the_same() {
    let points = ring(&[(0.0, 0.0), (0.3125, 0.0), (0.1, 0.9)]);
    let text = jackdaw_editor_tile::tileset_shape::format_points(&points);
    assert_eq!(parse_points(&text), Ok(points));
}

#[test]
fn snapping_puts_points_on_the_pixel_or_half_pixel_grid() {
    let tile = Vec2::splat(16.0);
    let at = Vec2::new(0.52, 0.04);
    assert_eq!(ShapeSnap::Pixel.apply(at, tile), Vec2::new(0.5, 0.0625));
    assert_eq!(
        ShapeSnap::HalfPixel.apply(at, tile),
        Vec2::new(0.53125, 0.03125)
    );
    assert_eq!(
        ShapeSnap::Off.apply(Vec2::new(1.2, 0.3), tile),
        Vec2::new(1.0, 0.3)
    );
}

/// The open dock under `root`, in the collision mode.
fn colliding(app: &mut App) -> Entity {
    let (_, root) = opened(app);
    press(app, root, DockAction::Mode(ModeId::COLLISION));
    root
}

fn tile_center(app: &mut App, root: Entity, tile: u32) -> Vec2 {
    let cell = control(app, root, DockAction::Tile(0, tile));
    screen_rect(app, cell).center()
}

fn redo_entries(app: &App) -> usize {
    app.world().resource::<CommandHistory>().redo_stack.len()
}

fn editors(app: &mut App, root: Entity) -> Vec<ShapePopover> {
    (app.world_mut().query::<&ShapePopover>().iter(app.world()))
        .filter(|p| p.root == root)
        .copied()
        .collect()
}

/// The screen point of `at`, in tile fractions y up, on the editor's tile.
fn on_canvas(app: &mut App, root: Entity, at: Vec2) -> Vec2 {
    let canvas = (app.world_mut().query::<(Entity, &ShapeCanvas)>())
        .iter(app.world())
        .find(|(_, c)| c.root == root)
        .map(|(e, _)| e)
        .expect("a canvas");
    let rect = screen_rect(app, canvas);
    rect.min + Vec2::new(at.x, 1.0 - at.y) * rect.size()
}

fn click(app: &mut App, at: Vec2, button: &str) {
    input(
        app,
        &format!(
            "input.pointer x={} y={} action=click button={button}",
            at.x, at.y
        ),
    );
    settle(app);
}

fn handles(app: &mut App, root: Entity) -> Vec<ShapeHandle> {
    (app.world_mut().query::<&ShapeHandle>().iter(app.world()))
        .filter(|h| h.root == root)
        .copied()
        .collect()
}

/// Open the shape editor of the dock under `root` on tile `tile` with the
/// custom row.
fn edit_tile(app: &mut App, root: Entity, tile: u32) {
    press(
        app,
        root,
        DockAction::CollisionPreset(CollisionPreset::Custom),
    );
    let at = tile_center(app, root, tile);
    click(app, at, "primary");
}

#[test]
fn double_clicking_a_tile_opens_the_editor_and_takes_back_the_first_click() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    let (before, count) = (on_disk(&dir), entries(&app));
    let redo = redo_entries(&app);
    let at = tile_center(&mut app, root, 1);
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=dblclick", at.x, at.y),
    );
    settle(&mut app);
    let open = editors(&mut app, root);
    assert_eq!(open.len(), 1, "the editor opens");
    assert_eq!(open[0].tile, (0, 1));
    assert_eq!(on_disk(&dir), before, "the first click is taken back");
    assert_eq!(entries(&app), count);
    assert_eq!(redo_entries(&app), redo, "nothing to redo");
    run(&mut app, "history.redo");
    settle(&mut app);
    assert_eq!(on_disk(&dir), before, "redo does not paint the tile");

    input(&mut app, "input.key key=Escape");
    settle(&mut app);
    assert!(editors(&mut app, root).is_empty(), "Escape closes it");
    let view = app
        .world()
        .resource::<TilesetDockViews>()
        .0
        .values()
        .next()
        .cloned();
    assert!(view.expect("a view").shape.is_none());
}

#[test]
fn the_custom_row_opens_the_editor_on_a_clicked_tile_without_painting() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    let before = on_disk(&dir);
    edit_tile(&mut app, root, 2);
    assert_eq!(editors(&mut app, root)[0].tile, (0, 2));
    assert_eq!(on_disk(&dir), before);
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
}

#[test]
fn clicks_draw_a_polygon_closed_on_its_first_point_as_one_undo_entry() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    edit_tile(&mut app, root, 1);
    let count = entries(&app);
    for at in [(0.1, 0.1), (0.8, 0.12), (0.5, 0.9)] {
        let point = on_canvas(&mut app, root, Vec2::new(at.0, at.1));
        click(&mut app, point, "primary");
    }
    let drawn = handles(&mut app, root);
    assert_eq!(drawn.iter().filter(|h| h.polygon.is_none()).count(), 3);
    assert_eq!(entries(&app), count, "nothing saved while drawing");
    let first = on_canvas(&mut app, root, Vec2::new(0.125, 0.125));
    click(&mut app, first, "primary");

    let shapes = collision_of(&on_disk(&dir), 1);
    assert_eq!(shapes.len(), 1);
    assert_eq!(shapes[0].preset, CollisionPreset::Custom);
    assert_eq!(
        shapes[0].polygons,
        [ring(&[(0.125, 0.125), (0.8125, 0.125), (0.5, 0.875)])],
        "snapped to pixels"
    );
    assert_eq!(entries(&app), count + 1);
    let polygon_handles = handles(&mut app, root);
    assert_eq!(
        polygon_handles
            .iter()
            .filter(|h| h.polygon == Some(0))
            .count(),
        3
    );
    let fills = (app.world_mut().query::<&ShapePiece>().iter(app.world()))
        .filter(|p| matches!(p, ShapePiece::Fill { polygon: 0, .. }))
        .count();
    assert!(fills > 0, "the polygon is filled");
}

#[test]
fn enter_closes_the_polygon_being_drawn() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    edit_tile(&mut app, root, 1);
    for at in [(0.01, 0.01), (0.99, 0.01), (0.99, 0.99)] {
        let point = on_canvas(&mut app, root, Vec2::new(at.0, at.1));
        click(&mut app, point, "primary");
    }
    input(&mut app, "input.key key=Enter");
    settle(&mut app);
    let shapes = collision_of(&on_disk(&dir), 1);
    assert_eq!(shapes[0].preset, CollisionPreset::SlopeUp);
    assert!(!editors(&mut app, root).is_empty(), "the editor stays open");
}

#[test]
fn dragging_a_point_moves_it_as_one_undo_entry() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    edit_tile(&mut app, root, 1);
    press(&mut app, root, DockAction::ShapeAddPolygon);
    let count = entries(&app);
    let from = on_canvas(&mut app, root, Vec2::new(0.75, 0.75));
    let to = on_canvas(&mut app, root, Vec2::new(1.0, 1.0));
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=move", from.x, from.y),
    );
    input(
        &mut app,
        &format!(
            "input.pointer x={} y={} action=drag_to steps=8 button=primary",
            to.x, to.y
        ),
    );
    settle(&mut app);
    let shape = &collision_of(&on_disk(&dir), 1)[0];
    assert_eq!(
        shape.polygons,
        [ring(&[
            (0.25, 0.25),
            (0.75, 0.25),
            (1.0, 1.0),
            (0.25, 0.75)
        ])]
    );
    assert_eq!(entries(&app), count + 1, "one entry for the drag");
}

#[test]
fn right_clicking_points_deletes_them_and_a_polygon_under_three_goes() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    edit_tile(&mut app, root, 1);
    press(&mut app, root, DockAction::ShapeAddPolygon);
    let corner = on_canvas(&mut app, root, Vec2::new(0.25, 0.75));
    click(&mut app, corner, "secondary");
    assert_eq!(collision_of(&on_disk(&dir), 1)[0].polygons[0].len(), 3);
    let count = entries(&app);
    let next = on_canvas(&mut app, root, Vec2::new(0.75, 0.75));
    click(&mut app, next, "secondary");
    assert!(collision_of(&on_disk(&dir), 1).is_empty());
    assert_eq!(entries(&app), count + 1);
    assert!(handles(&mut app, root).is_empty());
}

#[test]
fn a_click_on_an_edge_inserts_a_point_there() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    edit_tile(&mut app, root, 1);
    press(&mut app, root, DockAction::ShapeAddPolygon);
    let edge = on_canvas(&mut app, root, Vec2::new(0.5, 0.26));
    click(&mut app, edge, "primary");
    let shape = &collision_of(&on_disk(&dir), 1)[0];
    assert_eq!(shape.polygons[0][1], Vec2::new(0.5, 0.25));
    assert_eq!(shape.polygons[0].len(), 5);
}

#[test]
fn the_editor_buttons_flip_trace_and_flag_the_shape() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    edit_tile(&mut app, root, 0);
    let drawn = [("tile", "0:0"), ("points", "0,0;1,0;1,1")];
    call(&mut app, "tileset.collision.set_polygon", &drawn);
    settle(&mut app);
    press(&mut app, root, DockAction::ShapeTurn(ShapeTurn::FlipX));
    assert_eq!(
        collision_of(&on_disk(&dir), 0)[0].preset,
        CollisionPreset::SlopeDown
    );
    press(&mut app, root, DockAction::ShapeTrigger);
    let shape = &collision_of(&on_disk(&dir), 0)[0];
    assert!(shape.trigger && !shape.one_way);
    assert_eq!(shape.preset, CollisionPreset::SlopeDown);
    press(&mut app, root, DockAction::ShapeFromTraced);
    let traced = collision_of(&on_disk(&dir), 0)[0].clone();
    assert_eq!(traced.preset, CollisionPreset::Traced);
    assert!(traced.trigger, "flags kept");
    press(&mut app, root, DockAction::ShapeOneWay);
    let shape = &collision_of(&on_disk(&dir), 0)[0];
    assert!(shape.one_way);
    assert_eq!(
        (shape.preset, &shape.polygons),
        (CollisionPreset::Traced, &traced.polygons),
        "a flag keeps the traced polygons"
    );
    press(&mut app, root, DockAction::ShapeRemovePolygon(0));
    assert!(
        collision_of(&on_disk(&dir), 0).is_empty(),
        "the traced tile has one polygon"
    );
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    run(&mut app, "history.undo");
    settle(&mut app);
    assert_eq!(
        collision_of(&on_disk(&dir), 0)[0].preset,
        CollisionPreset::Traced
    );
}

#[test]
fn a_click_outside_the_open_editor_closes_it_without_painting() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    edit_tile(&mut app, root, 2);
    press(
        &mut app,
        root,
        DockAction::CollisionPreset(CollisionPreset::Solid),
    );
    assert_eq!(editors(&mut app, root).len(), 1, "the editor stays open");
    let (before, count) = (on_disk(&dir), entries(&app));
    let at = tile_center(&mut app, root, 3);
    click(&mut app, at, "primary");
    assert!(editors(&mut app, root).is_empty(), "the press closes it");
    assert_eq!(on_disk(&dir), before, "and paints nothing");
    assert_eq!(entries(&app), count);
    click(&mut app, at, "primary");
    assert_eq!(
        collision_of(&on_disk(&dir), 3).len(),
        1,
        "the next click paints"
    );
}

/// Mouse button events for the primary window, all delivered in the next
/// frame, as a slow frame gets them from the window.
fn buttons_in_one_frame(app: &mut App, states: &[ButtonState]) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .single(app.world())
        .expect("a window");
    for state in states {
        app.world_mut()
            .write_message(WindowEvent::MouseButtonInput(MouseButtonInput {
                button: MouseButton::Left,
                state: *state,
                window,
            }));
    }
    app.update();
}

#[test]
fn a_double_click_whose_events_share_frames_still_opens_the_editor() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    let before = on_disk(&dir);
    let at = tile_center(&mut app, root, 1);
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=move", at.x, at.y),
    );
    buttons_in_one_frame(&mut app, &[ButtonState::Pressed]);
    buttons_in_one_frame(
        &mut app,
        &[
            ButtonState::Released,
            ButtonState::Pressed,
            ButtonState::Released,
        ],
    );
    settle(&mut app);
    settle(&mut app);
    assert_eq!(editors(&mut app, root).len(), 1, "the editor opens");
    assert_eq!(on_disk(&dir), before, "the first click is taken back");
}

#[test]
fn the_shape_editor_is_as_tall_as_its_content() {
    let (mut app, _dir) = handmade();
    let root = colliding(&mut app);
    edit_tile(&mut app, root, 1);
    settle(&mut app);
    let popover = (app.world_mut().query::<(Entity, &ShapePopover)>())
        .iter(app.world())
        .find(|(_, p)| p.root == root)
        .map(|(e, _)| e)
        .expect("an editor");
    let height = |app: &App, entity: Entity| {
        let node = app.world().get::<ComputedNode>(entity).expect("laid out");
        node.size().y * node.inverse_scale_factor()
    };
    let children: Vec<Entity> = app
        .world()
        .get::<Children>(popover)
        .expect("content")
        .to_vec();
    let content: f32 = children.iter().map(|c| height(&app, *c)).sum();
    let gaps = tokens::SPACING_MD * (children.len() - 1) as f32;
    let frame = 2.0 * tokens::SPACING_MD + 2.0;
    let expected = content + gaps + frame;
    let shown = height(&app, popover);
    assert!(
        (shown - expected).abs() < 1.0,
        "{shown} tall for {expected} of content"
    );
}

fn tile_entity(app: &mut App, root: Entity, tile: u32) -> Entity {
    control(app, root, DockAction::Tile(0, tile))
}

#[test]
fn a_double_click_after_the_sheet_rebuilds_opens_the_editor() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    let before = on_disk(&dir);
    let at = tile_center(&mut app, root, 1);
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=move", at.x, at.y),
    );
    let first = tile_entity(&mut app, root, 1);
    buttons_in_one_frame(&mut app, &[ButtonState::Pressed]);
    buttons_in_one_frame(&mut app, &[ButtonState::Released]);
    settle(&mut app);
    assert_ne!(
        tile_entity(&mut app, root, 1),
        first,
        "the paint rebuilt the sheet"
    );
    assert_ne!(on_disk(&dir), before, "the first click painted");
    buttons_in_one_frame(&mut app, &[ButtonState::Pressed]);
    buttons_in_one_frame(&mut app, &[ButtonState::Released]);
    settle(&mut app);
    settle(&mut app);
    assert_eq!(editors(&mut app, root).len(), 1, "the editor opens");
    assert_eq!(on_disk(&dir), before, "the first click is taken back");
}

#[test]
fn a_double_click_whose_second_press_meets_the_rebuild_opens_the_editor() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    let before = on_disk(&dir);
    let at = tile_center(&mut app, root, 1);
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=move", at.x, at.y),
    );
    for state in [
        ButtonState::Pressed,
        ButtonState::Released,
        ButtonState::Pressed,
        ButtonState::Released,
    ] {
        buttons_in_one_frame(&mut app, &[state]);
    }
    settle(&mut app);
    settle(&mut app);
    assert_eq!(editors(&mut app, root).len(), 1, "the editor opens");
    assert_eq!(on_disk(&dir), before, "the first click is taken back");
}

/// Milliseconds the next frame stalls for, as a frame busy with a large
/// rebuild does, and how long every frame takes.
#[derive(Resource, Default)]
struct Stalls {
    next: u64,
    every: u64,
}

fn stalling(app: &mut App) {
    app.init_resource::<Stalls>();
    app.add_systems(Update, |mut stalls: ResMut<Stalls>| {
        let ms = std::mem::take(&mut stalls.next) + stalls.every;
        std::thread::sleep(std::time::Duration::from_millis(ms));
    });
}

#[test]
fn a_double_click_held_up_by_a_slow_paint_still_opens_the_editor() {
    let (mut app, dir) = handmade();
    stalling(&mut app);
    let root = colliding(&mut app);
    let before = on_disk(&dir);
    let at = tile_center(&mut app, root, 1);
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=move", at.x, at.y),
    );
    let first = tile_entity(&mut app, root, 1);
    buttons_in_one_frame(&mut app, &[ButtonState::Pressed]);
    app.world_mut().resource_mut::<Stalls>().next = 700;
    buttons_in_one_frame(&mut app, &[ButtonState::Released]);
    app.update();
    assert_ne!(
        tile_entity(&mut app, root, 1),
        first,
        "the paint rebuilt the sheet"
    );
    assert_ne!(on_disk(&dir), before, "the first click painted");
    buttons_in_one_frame(&mut app, &[ButtonState::Pressed]);
    buttons_in_one_frame(&mut app, &[ButtonState::Released]);
    settle(&mut app);
    settle(&mut app);
    assert_eq!(editors(&mut app, root).len(), 1, "the editor opens");
    assert_eq!(on_disk(&dir), before, "the first click is taken back");
}

#[test]
fn two_clicks_far_apart_in_busy_frames_do_not_open_the_editor() {
    let (mut app, dir) = handmade();
    stalling(&mut app);
    let root = colliding(&mut app);
    let at = tile_center(&mut app, root, 1);
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=move", at.x, at.y),
    );
    app.world_mut().resource_mut::<Stalls>().every = 60;
    buttons_in_one_frame(&mut app, &[ButtonState::Pressed]);
    buttons_in_one_frame(&mut app, &[ButtonState::Released]);
    for _ in 0..10 {
        app.update();
    }
    buttons_in_one_frame(&mut app, &[ButtonState::Pressed]);
    buttons_in_one_frame(&mut app, &[ButtonState::Released]);
    settle(&mut app);
    assert!(editors(&mut app, root).is_empty());
    assert_eq!(
        collision_of(&on_disk(&dir), 1).len(),
        1,
        "the tile stays painted"
    );
}

#[test]
fn a_double_click_held_up_between_frames_still_opens_the_editor() {
    let (mut app, dir) = handmade();
    let root = colliding(&mut app);
    let before = on_disk(&dir);
    let at = tile_center(&mut app, root, 1);
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=move", at.x, at.y),
    );
    let first = tile_entity(&mut app, root, 1);
    buttons_in_one_frame(&mut app, &[ButtonState::Pressed]);
    buttons_in_one_frame(&mut app, &[ButtonState::Released]);
    std::thread::sleep(std::time::Duration::from_millis(700));
    app.update();
    assert_ne!(
        tile_entity(&mut app, root, 1),
        first,
        "the paint rebuilt the sheet"
    );
    buttons_in_one_frame(&mut app, &[ButtonState::Pressed]);
    buttons_in_one_frame(&mut app, &[ButtonState::Released]);
    settle(&mut app);
    settle(&mut app);
    assert_eq!(editors(&mut app, root).len(), 1, "the editor opens");
    assert_eq!(on_disk(&dir), before, "the first click is taken back");
}
