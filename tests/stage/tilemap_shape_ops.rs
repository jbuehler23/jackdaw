//! The parametric tile operators `tilemap.fill_rect`, `tilemap.draw_line`,
//! `tilemap.fill` and `tilemap.clear`: one history entry per call, refused
//! with a reason on bad input, and the same cells and entry as the tile tool
//! they stand in for.

use crate::tile_tools::{
    BRUSH, BUCKET, LINE, RECTANGLE, available, bytes, click, enter, entries, fill, hold, run,
    set_brush, stored, stroke, tile_map, two_islands,
};
use crate::tilemap_render::{grass_patch, ground_terrain, layer, project_app, settle, spawn_map};

use bevy::prelude::*;
use jackdaw::tile_tools::MAX_FILL_CELLS;
use jackdaw::tile_tools::TileEditMode;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{OperatorReports, OperatorWarnings};
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::{PropertyValue, Tilemap, TilemapLayerKind};
use jackdaw_tilemap::{CellCoord, LayerData, LayerKind, TileRef};
use jackdaw_tileset::Tileset;

const FILL_RECT: &str = "tilemap.fill_rect";
const DRAW_LINE: &str = "tilemap.draw_line";
const FILL: &str = "tilemap.fill";
const CLEAR: &str = "tilemap.clear";
const TWINS: [&str; 4] = [FILL_RECT, DRAW_LINE, FILL, CLEAR];

pub(crate) type Params = Vec<(&'static str, PropertyValue)>;

pub(crate) fn call(
    app: &mut App,
    id: &str,
    params: &[(&'static str, PropertyValue)],
) -> OperatorResult {
    app.world_mut()
        .get_resource_or_init::<OperatorWarnings>()
        .0
        .clear();
    app.world_mut()
        .get_resource_or_init::<OperatorReports>()
        .0
        .clear();
    let mut op = app.world_mut().operator(id.to_string());
    for (name, value) in params {
        op = op.param(*name, value.clone());
    }
    let result = op.call().expect("dispatches");
    settle(app);
    result
}

pub(crate) fn text(value: &str) -> PropertyValue {
    PropertyValue::from(value.to_string())
}

pub(crate) fn int(value: i64) -> PropertyValue {
    PropertyValue::Int(value)
}

pub(crate) fn rect(x0: i64, y0: i64, x1: i64, y1: i64) -> Params {
    vec![
        ("x0", int(x0)),
        ("y0", int(y0)),
        ("x1", int(x1)),
        ("y1", int(y1)),
    ]
}

pub(crate) fn with(mut params: Params, more: &[(&'static str, PropertyValue)]) -> Params {
    params.extend(more.iter().cloned());
    params
}

pub(crate) fn warnings(app: &App) -> String {
    (app.world().get_resource::<OperatorWarnings>()).map_or(String::new(), |w| w.0.join("\n"))
}

pub(crate) fn last_label(app: &App) -> String {
    let history = app.world().resource::<CommandHistory>();
    (history.undo_stack.last()).map_or(String::new(), |entry| entry.description().to_string())
}

fn tileset(app: &App) -> Tileset {
    (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(_, t)| t.clone())
        .expect("the tileset")
}

fn terrain_name(app: &App, id: u8) -> String {
    (tileset(app).terrains.iter())
        .find(|t| t.id == id)
        .map(|t| t.name.clone())
        .expect("the terrain")
}

pub(crate) fn terrain_at(app: &mut App, map: Entity, x: i32, y: i32) -> u8 {
    let data = stored(app, map);
    (data.layers[0].authoring.as_ref()).map_or(0, |a| a.terrain.get(CellCoord::new(x, y)))
}

fn tile_at(app: &mut App, map: Entity, x: i32, y: i32) -> TileRef {
    stored(app, map).layers[0].tiles.get(CellCoord::new(x, y))
}

fn terrain_map(app: &mut App) -> Entity {
    let map = spawn_map(app, vec![ground_terrain()], grass_patch());
    enter(app, map);
    map
}

/// Runs the tool gesture, takes its cells and entry, undoes it, then calls
/// `twin` and expects the same cells and the same entry, undone and redone
/// exactly.
#[track_caller]
fn assert_parity(
    app: &mut App,
    map: Entity,
    gesture: impl FnOnce(&mut App),
    twin: &str,
    params: &[(&'static str, PropertyValue)],
) {
    let (before, count) = (bytes(app, map), entries(app));
    gesture(app);
    assert_eq!(entries(app), count + 1, "the gesture makes one entry");
    let (after, label) = (bytes(app, map), last_label(app));
    assert_ne!(after, before, "the gesture changed cells");
    run(app, "history.undo");
    assert_eq!(bytes(app, map), before);

    let result = call(app, twin, params);
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(app));
    assert_eq!(entries(app), count + 1, "{twin} makes one entry");
    assert_eq!(
        bytes(app, map),
        after,
        "{twin} writes what the gesture wrote"
    );
    assert_eq!(
        last_label(app),
        label,
        "{twin} labels its entry as the gesture"
    );
    run(app, "history.undo");
    assert_eq!(bytes(app, map), before);
    run(app, "history.redo");
    assert_eq!(bytes(app, map), after);
}

#[track_caller]
pub(crate) fn assert_refused(
    app: &mut App,
    map: Entity,
    id: &str,
    params: &[(&'static str, PropertyValue)],
    why: &str,
) {
    let (before, count) = (bytes(app, map), entries(app));
    assert_eq!(
        call(app, id, params),
        OperatorResult::Cancelled,
        "{id} {params:?}"
    );
    let said = warnings(app);
    assert!(said.contains(why), "{id}: wanted `{why}`, got `{said}`");
    assert_eq!(bytes(app, map), before);
    assert_eq!(entries(app), count);
}

#[test]
fn the_twins_are_available_unless_a_stroke_is_held() {
    let (mut app, _dir) = project_app();
    for id in TWINS {
        assert!(available(&mut app, id), "{id} without a map");
        assert_eq!(call(&mut app, id, &[]), OperatorResult::Cancelled, "{id}");
        assert!(
            warnings(&app).contains("no tilemap"),
            "{id}: {}",
            warnings(&app)
        );
    }
    let map = terrain_map(&mut app);
    assert!(set_brush(&mut app, "terrain", "2") == OperatorResult::Finished);
    hold(&mut app, BRUSH, MouseButton::Left, &[(2, 2)]);
    for id in TWINS {
        assert!(!available(&mut app, id), "{id} while a stroke is held");
    }
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    settle(&mut app);
    for id in TWINS {
        assert!(available(&mut app, id), "{id} after the stroke");
    }
    assert_eq!(terrain_at(&mut app, map, 2, 2), 2);
}

#[test]
fn fill_rect_matches_the_rectangle_with_terrain_by_name() {
    let (mut app, _dir) = project_app();
    let map = terrain_map(&mut app);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    let name = terrain_name(&app, 2);
    assert_parity(
        &mut app,
        map,
        |app| stroke(app, RECTANGLE, MouseButton::Left, &[(4, 5), (2, 2)]),
        FILL_RECT,
        &with(rect(4, 5, 2, 2), &[("terrain", text(&name))]),
    );
    assert_eq!(terrain_at(&mut app, map, 3, 4), 2);
}

#[test]
fn fill_rect_in_path_mode_matches_the_rectangle() {
    let (mut app, _dir) = project_app();
    let map = terrain_map(&mut app);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    assert_eq!(
        set_brush(&mut app, "mode", "path"),
        OperatorResult::Finished
    );
    assert_parity(
        &mut app,
        map,
        |app| stroke(app, RECTANGLE, MouseButton::Left, &[(1, 1), (3, 2)]),
        FILL_RECT,
        &with(
            rect(1, 1, 3, 2),
            &[("terrain", text("2")), ("mode", text("path"))],
        ),
    );
}

#[test]
fn fill_rect_erase_matches_the_secondary_rectangle() {
    let (mut app, _dir) = project_app();
    let cells: Vec<_> = (0..4)
        .flat_map(|y| (0..4).map(move |x| (x, y, TileRef::new(5))))
        .collect();
    let map = tile_map(&mut app, &cells);
    assert_parity(
        &mut app,
        map,
        |app| stroke(app, RECTANGLE, MouseButton::Right, &[(1, 1), (2, 3)]),
        FILL_RECT,
        &with(rect(1, 1, 2, 3), &[("erase", PropertyValue::Bool(true))]),
    );
}

#[test]
fn fill_rect_is_clipped_to_the_bounds_like_the_rectangle() {
    let (mut app, _dir) = project_app();
    let map = tile_map(&mut app, &[]);
    app.world_mut()
        .get_mut::<Tilemap>(map)
        .expect("the map")
        .bounds = Some(IRect::new(0, 0, 8, 8));
    assert_eq!(set_brush(&mut app, "tile", "7"), OperatorResult::Finished);
    assert_parity(
        &mut app,
        map,
        |app| stroke(app, RECTANGLE, MouseButton::Left, &[(2, 2), (3000, 3000)]),
        FILL_RECT,
        &with(rect(2, 2, 3000, 3000), &[("tile", text("7"))]),
    );
    assert_eq!(stored(&mut app, map).layers[0].tiles.cells().count(), 36);
}

#[test]
fn draw_line_matches_the_line_tool() {
    let (mut app, _dir) = project_app();
    let map = tile_map(&mut app, &[]);
    assert_eq!(set_brush(&mut app, "tile", "7"), OperatorResult::Finished);
    assert_parity(
        &mut app,
        map,
        |app| stroke(app, LINE, MouseButton::Left, &[(0, 0), (2, 5), (6, 3)]),
        DRAW_LINE,
        &with(rect(0, 0, 6, 3), &[("tile", text("7"))]),
    );
}

#[test]
fn draw_line_of_terrain_matches_the_line_tool() {
    let (mut app, _dir) = project_app();
    let map = terrain_map(&mut app);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    assert_parity(
        &mut app,
        map,
        |app| stroke(app, LINE, MouseButton::Left, &[(0, 3), (8, 1)]),
        DRAW_LINE,
        &with(rect(0, 3, 8, 1), &[("terrain", text("2"))]),
    );
}

#[test]
fn fill_matches_the_bucket_on_terrain_contiguous_or_not() {
    for contiguous in [true, false] {
        let (mut app, _dir) = project_app();
        let map = spawn_map(&mut app, vec![ground_terrain()], two_islands());
        enter(&mut app, map);
        assert_eq!(
            set_brush(&mut app, "terrain", "2"),
            OperatorResult::Finished
        );
        let name = terrain_name(&app, 2);
        assert_parity(
            &mut app,
            map,
            |app| assert_eq!(fill(app, (2, 2), contiguous), OperatorResult::Finished),
            FILL,
            &[
                ("x", int(2)),
                ("y", int(2)),
                ("terrain", text(&name)),
                ("contiguous", PropertyValue::Bool(contiguous)),
            ],
        );
        let island = if contiguous { 1 } else { 2 };
        assert_eq!(
            terrain_at(&mut app, map, 10, 10),
            island,
            "contiguous {contiguous}"
        );
    }
}

#[test]
fn fill_defaults_to_contiguous() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], two_islands());
    enter(&mut app, map);
    let result = call(
        &mut app,
        FILL,
        &[("x", int(2)), ("y", int(2)), ("terrain", text("2"))],
    );
    assert_eq!(result, OperatorResult::Finished, "{}", warnings(&app));
    assert_eq!(terrain_at(&mut app, map, 2, 2), 2);
    assert_eq!(terrain_at(&mut app, map, 10, 10), 1, "the island stays");
}

/// A tile filled over a terrain layer matches cells by the tile they show.
#[test]
fn fill_with_a_tile_on_a_terrain_layer_matches_the_bucket() {
    let (mut app, _dir) = project_app();
    let map = terrain_map(&mut app);
    assert_eq!(set_brush(&mut app, "tile", "7"), OperatorResult::Finished);
    assert_parity(
        &mut app,
        map,
        |app| assert_eq!(fill(app, (2, 2), true), OperatorResult::Finished),
        FILL,
        &[("x", int(2)), ("y", int(2)), ("tile", text("7"))],
    );
}

#[test]
fn fill_erase_matches_the_secondary_bucket() {
    let (mut app, _dir) = project_app();
    let five = TileRef::new(5);
    let map = tile_map(&mut app, &[(0, 0, five), (1, 0, five), (3, 0, five)]);
    assert_parity(
        &mut app,
        map,
        |app| {
            assert_eq!(
                click(app, BUCKET, MouseButton::Right, (1, 0)),
                OperatorResult::Finished
            );
        },
        FILL,
        &[
            ("x", int(1)),
            ("y", int(0)),
            ("erase", PropertyValue::Bool(true)),
        ],
    );
}

#[test]
fn clear_of_a_rect_matches_the_secondary_rectangle() {
    let (mut app, _dir) = project_app();
    let map = terrain_map(&mut app);
    assert_parity(
        &mut app,
        map,
        |app| stroke(app, RECTANGLE, MouseButton::Right, &[(1, 1), (3, 4)]),
        CLEAR,
        &rect(1, 1, 3, 4),
    );
}

#[test]
fn clear_empties_a_terrain_layer_with_its_hand_placed_tiles() {
    let (mut app, _dir) = project_app();
    let map = terrain_map(&mut app);
    assert_eq!(
        call(
            &mut app,
            FILL_RECT,
            &with(rect(1, 1, 1, 1), &[("tile", text("7"))])
        ),
        OperatorResult::Finished
    );
    let (before, count) = (bytes(&mut app, map), entries(&app));

    assert_eq!(
        call(&mut app, CLEAR, &[]),
        OperatorResult::Finished,
        "{}",
        warnings(&app)
    );
    assert_eq!(entries(&app), count + 1);
    assert_eq!(last_label(&app), "Clear layer");
    let data = stored(&mut app, map);
    let authoring = data.layers[0].authoring.as_ref().expect("authoring");
    assert_eq!(authoring.terrain.cells().count(), 0);
    assert!(authoring.overrides.is_empty());
    assert_eq!(data.layers[0].tiles.cells().count(), 0);
    let after = bytes(&mut app, map);
    run(&mut app, "history.undo");
    assert_eq!(bytes(&mut app, map), before);
    run(&mut app, "history.redo");
    assert_eq!(bytes(&mut app, map), after);

    assert_eq!(call(&mut app, CLEAR, &[]), OperatorResult::Finished);
    assert_eq!(
        entries(&app),
        count + 1,
        "clearing an empty layer adds nothing"
    );
}

#[test]
fn tiles_go_by_sheet_and_tile_or_by_name() {
    let (mut app, _dir) = project_app();
    let map = tile_map(&mut app, &[]);
    let tileset = tileset(&app);
    let layer = tileset.layer(0, 9).expect("tile 9 of sheet 0");
    let id = (app.world().resource::<Assets<Tileset>>().ids())
        .next()
        .expect("the tileset id");
    app.world_mut()
        .resource_mut::<Assets<Tileset>>()
        .get_mut(id)
        .expect("the tileset")
        .properties
        .insert(3, [("name".to_string(), "chest".to_string())].into());
    settle(&mut app);

    let painted = |app: &mut App, tile: &str| {
        let params = with(rect(0, 0, 0, 0), &[("tile", text(tile))]);
        assert_eq!(
            call(app, FILL_RECT, &params),
            OperatorResult::Finished,
            "{}",
            warnings(app)
        );
        tile_at(app, map, 0, 0)
    };
    let packed = |layer: u32| TileRef::new(u16::try_from(layer).expect("a packed layer"));
    assert_eq!(painted(&mut app, "0:9"), packed(layer));
    assert_eq!(painted(&mut app, "chest"), packed(3));
}

#[test]
fn nothing_to_change_leaves_no_entry() {
    let (mut app, _dir) = project_app();
    let map = terrain_map(&mut app);
    let (before, count) = (bytes(&mut app, map), entries(&app));
    for (id, params) in [
        (FILL_RECT, with(rect(0, 0, 3, 3), &[("terrain", text("1"))])),
        (DRAW_LINE, with(rect(0, 0, 3, 0), &[("terrain", text("1"))])),
        (
            FILL,
            vec![("x", int(2)), ("y", int(2)), ("terrain", text("1"))],
        ),
        (CLEAR, rect(20, 20, 24, 24)),
        (
            FILL_RECT,
            with(
                rect(20, 20, 24, 24),
                &[("erase", PropertyValue::Bool(true))],
            ),
        ),
    ] {
        assert_eq!(
            call(&mut app, id, &params),
            OperatorResult::Finished,
            "{id}"
        );
        assert!(warnings(&app).is_empty(), "{id}: {}", warnings(&app));
    }
    assert_eq!(bytes(&mut app, map), before);
    assert_eq!(entries(&app), count);
}

#[test]
fn a_map_is_named_by_entity_or_taken_from_the_selection() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    app.world_mut()
        .resource_mut::<jackdaw::selection::Selection>()
        .entities
        .clear();
    let params = with(rect(0, 0, 0, 0), &[("terrain", text("2"))]);
    assert_refused(
        &mut app,
        map,
        FILL_RECT,
        &params,
        "no tilemap given or selected",
    );
    let named = with(params.clone(), &[("entity", PropertyValue::Entity(map))]);
    assert_eq!(
        call(&mut app, FILL_RECT, &named),
        OperatorResult::Finished,
        "{}",
        warnings(&app)
    );
    assert_eq!(terrain_at(&mut app, map, 0, 0), 2);
    app.world_mut()
        .resource_mut::<jackdaw::selection::Selection>()
        .entities = vec![map];
    let params = with(
        rect(1, 0, 1, 0),
        &[("terrain", text("2")), ("layer", text("Ground"))],
    );
    assert_eq!(
        call(&mut app, FILL_RECT, &params),
        OperatorResult::Finished,
        "{}",
        warnings(&app)
    );
    assert_eq!(terrain_at(&mut app, map, 1, 0), 2);
}

#[test]
fn bad_input_is_refused_with_a_reason() {
    let cap = MAX_FILL_CELLS.to_string();
    let (mut app, _dir) = project_app();
    let map = terrain_map(&mut app);
    let terrain = |more: &[(&'static str, PropertyValue)]| with(rect(0, 0, 1, 1), more);
    let cases: Vec<(&str, Params, &str)> = vec![
        (
            FILL_RECT,
            terrain(&[("terrain", text("2")), ("layer", text("Sky"))]),
            "no layer `Sky`",
        ),
        (
            FILL_RECT,
            terrain(&[("terrain", text("2")), ("layer", text("4"))]),
            "no layer `4`",
        ),
        (FILL_RECT, rect(0, 0, 1, 1), "one of terrain, tile or erase"),
        (
            FILL_RECT,
            terrain(&[("terrain", text("2")), ("tile", text("7"))]),
            "one of terrain, tile or erase",
        ),
        (
            FILL_RECT,
            terrain(&[("terrain", text("lava"))]),
            "no terrain is `lava`",
        ),
        (
            FILL_RECT,
            terrain(&[("tile", text("9999"))]),
            "tile `9999` is out of range",
        ),
        (
            FILL_RECT,
            terrain(&[("terrain", text("2")), ("mode", text("zigzag"))]),
            "mode is connect or path",
        ),
        (
            FILL_RECT,
            vec![("x0", int(0)), ("terrain", text("2"))],
            "y0 is required",
        ),
        (DRAW_LINE, vec![("terrain", text("2"))], "x0 is required"),
        (FILL, vec![("terrain", text("2"))], "x is required"),
        (
            FILL_RECT,
            with(rect(0, 0, 1 << 40, 0), &[("terrain", text("2"))]),
            "x1 is out of range",
        ),
        (
            FILL_RECT,
            with(rect(-3000, -3000, 3000, 3000), &[("terrain", text("2"))]),
            &cap,
        ),
        (
            DRAW_LINE,
            with(
                rect(i64::from(i32::MIN), 0, i64::from(i32::MAX), 0),
                &[("terrain", text("2"))],
            ),
            &cap,
        ),
        (
            CLEAR,
            vec![("x0", int(0)), ("y0", int(0))],
            "give x0, y0, x1 and y1, or none",
        ),
    ];
    for (id, params, why) in &cases {
        assert_refused(&mut app, map, id, params, why);
    }
}

#[test]
fn terrain_is_refused_on_a_tile_layer_and_a_seed_outside_the_bounds() {
    let (mut app, _dir) = project_app();
    let map = tile_map(&mut app, &[(0, 0, TileRef::new(5))]);
    assert_refused(
        &mut app,
        map,
        FILL_RECT,
        &with(rect(0, 0, 1, 1), &[("terrain", text("2"))]),
        "is a tile layer",
    );
    app.world_mut()
        .get_mut::<Tilemap>(map)
        .expect("the map")
        .bounds = Some(IRect::new(0, 0, 8, 8));
    assert_refused(
        &mut app,
        map,
        FILL,
        &[("x", int(9)), ("y", int(0)), ("tile", text("7"))],
        "cell 9,0 is outside the map's bounds",
    );
}

#[test]
fn a_fill_past_the_cap_is_refused() {
    let (mut app, _dir) = project_app();
    let five = TileRef::new(5);
    let map = tile_map(&mut app, &[(0, 0, five), (1100, 1100, five)]);
    assert_refused(
        &mut app,
        map,
        FILL,
        &[("x", int(5)), ("y", int(5)), ("tile", text("7"))],
        &MAX_FILL_CELLS.to_string(),
    );
}

fn reports(app: &App) -> String {
    (app.world().get_resource::<OperatorReports>()).map_or(String::new(), |r| r.0.join("\n"))
}

#[test]
fn malformed_coordinates_are_refused_not_ignored() {
    let (mut app, _dir) = project_app();
    let map = terrain_map(&mut app);
    let float = |v: f64| PropertyValue::Float(v);
    let corners = vec![
        ("x0", float(0.0)),
        ("y0", float(0.0)),
        ("x1", float(5.0)),
        ("y1", float(5.0)),
    ];
    assert_refused(&mut app, map, CLEAR, &corners, "x0 is not an integer");
    let worded = with(
        vec![
            ("x0", text("left")),
            ("y0", int(0)),
            ("x1", int(1)),
            ("y1", int(1)),
        ],
        &[("terrain", text("2"))],
    );
    assert_refused(&mut app, map, FILL_RECT, &worded, "x0 is not an integer");
    assert_refused(
        &mut app,
        map,
        DRAW_LINE,
        &with(
            rect(0, 0, 1, 1),
            &[("terrain", text("2")), ("y1", float(1.5))],
        ),
        "y1 is not an integer",
    );
    assert_refused(
        &mut app,
        map,
        FILL,
        &[("x", float(2.0)), ("y", int(2)), ("terrain", text("2"))],
        "x is not an integer",
    );
}

#[test]
fn a_layer_is_a_name_an_index_or_refused() {
    let (mut app, _dir) = project_app();
    let map = terrain_map(&mut app);
    let params = with(
        rect(0, 0, 0, 0),
        &[("terrain", text("2")), ("layer", int(0))],
    );
    assert_eq!(
        call(&mut app, FILL_RECT, &params),
        OperatorResult::Finished,
        "{}",
        warnings(&app)
    );
    assert_eq!(terrain_at(&mut app, map, 0, 0), 2);
    for bad in [
        PropertyValue::Float(0.0),
        PropertyValue::Bool(true),
        int(-1),
        int(3),
    ] {
        let params = with(
            rect(1, 1, 1, 1),
            &[("terrain", text("2")), ("layer", bad.clone())],
        );
        assert_refused(&mut app, map, FILL_RECT, &params, "layer");
    }
}

#[test]
fn a_stale_tile_name_is_refused() {
    let (mut app, _dir) = project_app();
    let map = tile_map(&mut app, &[]);
    let id = (app.world().resource::<Assets<Tileset>>().ids())
        .next()
        .expect("the tileset id");
    app.world_mut()
        .resource_mut::<Assets<Tileset>>()
        .get_mut(id)
        .expect("the tileset")
        .properties
        .insert(60_000, [("name".to_string(), "ghost".to_string())].into());
    settle(&mut app);
    assert_refused(
        &mut app,
        map,
        FILL_RECT,
        &with(rect(0, 0, 0, 0), &[("tile", text("ghost"))]),
        "tile `ghost` is out of range",
    );
}

/// The count reported is of the cells that changed, after clipping.
#[test]
fn the_reported_count_is_of_changed_cells() {
    let (mut app, _dir) = project_app();
    let map = tile_map(&mut app, &[(3, 0, TileRef::new(7))]);
    app.world_mut()
        .get_mut::<Tilemap>(map)
        .expect("the map")
        .bounds = Some(IRect::new(0, 0, 8, 8));
    let line = with(rect(2, 0, 20, 0), &[("tile", text("7"))]);
    assert_eq!(call(&mut app, DRAW_LINE, &line), OperatorResult::Finished);
    assert!(
        reports(&app).contains("5 cells written"),
        "{}",
        reports(&app)
    );
    let line = with(rect(2, 1, 20, 1), &[("tile", text("7"))]);
    assert_eq!(call(&mut app, DRAW_LINE, &line), OperatorResult::Finished);
    assert!(
        reports(&app).contains("6 cells written"),
        "{}",
        reports(&app)
    );
    let clear = rect(-5, 0, 2, 0);
    assert_eq!(call(&mut app, CLEAR, &clear), OperatorResult::Finished);
    assert!(
        reports(&app).contains("1 cells written"),
        "{}",
        reports(&app)
    );
}

#[test]
fn a_shape_lands_on_the_layer_it_was_pressed_on() {
    let (mut app, _dir) = project_app();
    let mut data = grass_patch();
    data.layers.push(LayerData::empty(LayerKind::Tile));
    let map = spawn_map(
        &mut app,
        vec![ground_terrain(), layer(TilemapLayerKind::Tile, default())],
        data,
    );
    enter(&mut app, map);
    assert_eq!(
        set_brush(&mut app, "terrain", "2"),
        OperatorResult::Finished
    );
    let count = entries(&app);
    hold(&mut app, RECTANGLE, MouseButton::Left, &[(1, 1), (2, 2)]);
    app.world_mut().resource_mut::<TileEditMode>().layer = 1;
    app.world_mut()
        .resource_mut::<ButtonInput<MouseButton>>()
        .release(MouseButton::Left);
    settle(&mut app);
    assert_eq!(entries(&app), count + 1);
    assert_eq!(
        terrain_at(&mut app, map, 2, 2),
        2,
        "painted on the press layer"
    );
    assert_eq!(stored(&mut app, map).layers[1].tiles.cells().count(), 0);
}
