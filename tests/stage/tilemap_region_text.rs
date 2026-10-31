//! `tilemap.describe`, `tilemap.read_region` and `tilemap.write_region`: a
//! map as text an agent reads, and region text written back as one history
//! entry, validated whole before any cell changes.

use std::collections::BTreeMap;

use crate::tile_tools::{bytes, entries, run, stored, tile_map};
use crate::tilemap_render::{
    TILESET, grass_patch, ground_terrain, layer, project_app, settle, spawn_map, tile_layer,
};
use crate::tilemap_shape_ops::{
    Params, assert_refused, call, int, last_label, rect, terrain_at, text, warnings, with,
};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::OperatorReports;
use jackdaw_scene_types::{
    LayerTerrainMatch, PropertyValue, Tilemap, TilemapLayer, TilemapLayerKind,
};
use jackdaw_tilemap::{CellCoord, TileRef, TilemapData};
use jackdaw_tileset::Tileset;

const DESCRIBE: &str = "tilemap.describe";
const READ: &str = "tilemap.read_region";
const WRITE: &str = "tilemap.write_region";

fn report(app: &App) -> String {
    (app.world().get_resource::<OperatorReports>()).map_or(String::new(), |r| r.0.join("\n"))
}

#[track_caller]
fn finished(app: &mut App, id: &str, params: &[(&'static str, PropertyValue)]) -> String {
    let result = call(app, id, params);
    assert_eq!(result, OperatorResult::Finished, "{id}: {}", warnings(app));
    report(app)
}

fn at(map: Entity, x: i64, y: i64, region: &str) -> Params {
    vec![
        ("entity", PropertyValue::Entity(map)),
        ("x0", int(x)),
        ("y0", int(y)),
        ("text", text(region)),
    ]
}

fn read(map: Entity, x0: i64, y0: i64, x1: i64, y1: i64) -> Params {
    with(
        rect(x0, y0, x1, y1),
        &[("entity", PropertyValue::Entity(map))],
    )
}

fn props_layer() -> TilemapLayer {
    TilemapLayer {
        name: "Props".to_string(),
        ..layer(TilemapLayerKind::Tile, default())
    }
}

/// The grass patch with dirt at 1,1 and tile 5 placed by hand at 1,0.
fn marked_patch() -> TilemapData {
    let mut data = grass_patch();
    if let Some(a) = data.layers[0].authoring.as_mut() {
        a.terrain.set(CellCoord::new(1, 1), 7);
        a.overrides.insert(CellCoord::new(1, 0), TileRef::new(5));
    }
    data
}

fn edit_tileset(app: &mut App, edit: impl FnOnce(&mut Tileset)) {
    let id = (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(id, _)| id)
        .expect("the tileset");
    let mut sets = app.world_mut().resource_mut::<Assets<Tileset>>();
    edit(&mut sets.get_mut(id).expect("the tileset"));
}

fn two_layer_map(app: &mut App) -> Entity {
    let mut data = grass_patch();
    let props = tile_layer(&[(0, 0, TileRef::new(3)), (7, -2, TileRef::new(4))]);
    data.layers.extend(props.layers);
    spawn_map(app, vec![ground_terrain(), props_layer()], data)
}

#[test]
fn describe_lists_the_layers_with_their_tilesets_and_terrains() {
    let (mut app, _dir) = project_app();
    let map = two_layer_map(&mut app);
    let said = finished(
        &mut app,
        DESCRIBE,
        &[("entity", PropertyValue::Entity(map))],
    );
    assert_eq!(
        said,
        format!(
            "tilemap \"map\": cell 16x16, unbounded, painted 0,-2 to 7,5\n\
             layer 0 \"Ground\": terrain, tileset \"{TILESET}\", match corners_and_sides, 36 cells painted\n  \
             terrains: 1 \"grass\", 2 \"grass 2\", 5 \"grass 3\", 6 \"grass 4\", 7 \"dirt\", \
             9 \"grass 5\", 11 \"water\", 12 \"water 2\"\n\
             layer 1 \"Props\": tile, tileset \"{TILESET}\", 2 cells painted\n"
        )
    );
}

#[test]
fn a_terrain_layer_reads_as_terrain_names_and_hand_placed_tiles() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], marked_patch());
    let said = finished(&mut app, READ, &read(map, -1, 2, 2, -1));
    assert_eq!(
        said,
        "legend: {\".\": \"empty\", \"A\": \"tile:0:5\", \"d\": \"dirt\", \"g\": \"grass\"}\n\
         rows:\n\
         .ggg\n\
         .gdg\n\
         .gAg\n\
         ....\n"
    );
    let again = finished(&mut app, READ, &read(map, 2, -1, -1, 2));
    assert_eq!(again, said, "the same cells read the same text");
}

#[test]
fn a_tile_layer_reads_as_tiles() {
    let (mut app, _dir) = project_app();
    let cells = [
        (0, 0, TileRef::new(3)),
        (1, 0, TileRef::new(3)),
        (0, 1, TileRef::new(7)),
    ];
    let map = tile_map(&mut app, &cells);
    let said = finished(&mut app, READ, &read(map, 0, 1, 1, 0));
    assert_eq!(
        said,
        "legend: {\".\": \"empty\", \"A\": \"tile:0:3\", \"B\": \"tile:0:7\"}\nrows:\nB.\nAA\n"
    );
}

#[test]
fn tiles_reads_the_tiles_a_terrain_layer_shows() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], marked_patch());
    let params = with(
        read(map, 0, 1, 2, 0),
        &[("tiles", PropertyValue::Bool(true))],
    );
    let said = finished(&mut app, READ, &params);
    assert!(said.contains("\"tile:0:5\""), "{said}");
    assert!(!said.contains("grass") && !said.contains("dirt"), "{said}");
}

#[test]
fn a_tile_on_a_terrain_layer_is_placed_by_hand_even_where_it_resolves() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], marked_patch());
    let at00 = CellCoord::new(0, 0);
    let shown = stored(&mut app, map).layers[0].tiles.get(at00);
    assert!(!shown.is_empty(), "the patch resolves");
    let region = format!(
        "legend: {{\"R\": \"tile:0:{}\", \"F\": \"tile:0:5\"}}\nrows:\nRF\n",
        shown.index()
    );
    let count = entries(&app);
    let said = finished(&mut app, WRITE, &at(map, 0, 0, &region));
    assert_eq!(said, "1 cells written on layer `Ground`");
    assert_eq!(entries(&app), count + 1);
    let data = stored(&mut app, map);
    let placed = &data.layers[0]
        .authoring
        .as_ref()
        .expect("authoring")
        .overrides;
    assert_eq!(placed.get(&at00), Some(&shown));
    assert_eq!(placed.get(&CellCoord::new(1, 0)), Some(&TileRef::new(5)));
}

#[test]
fn orientation_is_checked_and_a_name_holding_at_resolves_whole() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let bad = "legend: {\"T\": \"tile:0:5@99\"}\nrows:\nT\n";
    assert_refused(
        &mut app,
        map,
        WRITE,
        &at(map, 0, 0, bad),
        "line 1, column 10: legend `T`: orientation 99 is not one of 0 to 15",
    );
    let flipped = "legend: {\"T\": \"tile:0:5@3\"}\nrows:\nT\n";
    finished(&mut app, WRITE, &at(map, 0, 0, flipped));
    let data = stored(&mut app, map);
    let placed = &data.layers[0]
        .authoring
        .as_ref()
        .expect("authoring")
        .overrides;
    assert_eq!(
        placed.get(&CellCoord::new(0, 0)),
        Some(&TileRef::from_raw(5, 3))
    );

    edit_tileset(&mut app, |tileset| {
        let name = BTreeMap::from([("name".to_string(), "gem@2".to_string())]);
        tileset.properties.insert(9, name);
    });
    let gem = "legend: {\"T\": \"tile:gem@2\"}\nrows:\nT\n";
    finished(&mut app, WRITE, &at(map, 1, 0, gem));
    let data = stored(&mut app, map);
    let placed = &data.layers[0]
        .authoring
        .as_ref()
        .expect("authoring")
        .overrides;
    assert_eq!(placed.get(&CellCoord::new(1, 0)), Some(&TileRef::new(9)));
}

#[test]
fn a_terrain_named_like_a_keyword_round_trips() {
    let (mut app, _dir) = project_app();
    edit_tileset(&mut app, |tileset| {
        if let Some(dirt) = tileset.terrains.iter_mut().find(|t| t.id == 7) {
            dirt.name = "erase".to_string();
        }
    });
    let map = spawn_map(&mut app, vec![ground_terrain()], marked_patch());
    let said = finished(&mut app, READ, &read(map, 0, 1, 1, 1));
    assert_eq!(
        said,
        "legend: {\".\": \"empty\", \"e\": \"terrain:erase\", \"g\": \"grass\"}\nrows:\nge\n"
    );
    let (before, count) = (bytes(&mut app, map), entries(&app));
    assert_eq!(
        finished(&mut app, WRITE, &at(map, 0, 1, &said)),
        "no cell changed"
    );
    assert_eq!((bytes(&mut app, map), entries(&app)), (before, count));
    let erase = "legend: {\"x\": \"erase\"}\nrows:\nxx\n";
    finished(&mut app, WRITE, &at(map, 0, 1, erase));
    assert_eq!(terrain_at(&mut app, map, 1, 1), 0, "erase still erases");
}

#[test]
fn cells_past_the_bounds_or_of_unknown_terrain_read_as_keep() {
    let (mut app, _dir) = project_app();
    let mut data = grass_patch();
    if let Some(a) = data.layers[0].authoring.as_mut() {
        a.terrain.set(CellCoord::new(2, 2), 3);
    }
    let map = spawn_map(&mut app, vec![ground_terrain()], data);
    let bounded = Tilemap {
        bounds: Some(IRect::new(0, 0, 4, 4)),
        ..app.world().get::<Tilemap>(map).expect("a map").clone()
    };
    app.world_mut().entity_mut(map).insert(bounded);
    settle(&mut app);
    let said = finished(&mut app, READ, &read(map, 2, 4, 4, 2));
    assert_eq!(
        said,
        "legend: {\".\": \"empty\", \"?\": \"keep\", \"g\": \"grass\"}\nrows:\n???\ngg?\n?g?\n"
    );
    let (before, count) = (bytes(&mut app, map), entries(&app));
    assert_eq!(
        finished(&mut app, WRITE, &at(map, 2, 4, &said)),
        "no cell changed"
    );
    assert_eq!((bytes(&mut app, map), entries(&app)), (before, count));
}

#[test]
fn a_layer_switched_to_tiles_reads_and_writes_its_tiles() {
    let (mut app, _dir) = project_app();
    let auto = layer(TilemapLayerKind::Terrain, LayerTerrainMatch::Auto);
    let map = spawn_map(&mut app, vec![auto], grass_patch());
    if let Some(mut tilemap) = app.world_mut().get_mut::<Tilemap>(map) {
        tilemap.layers[0].kind = TilemapLayerKind::Tile;
    }
    settle(&mut app);
    let said = finished(&mut app, READ, &read(map, 0, 1, 1, 0));
    assert_eq!(said, "legend: {\".\": \"empty\"}\nrows:\n..\n..\n");
    let (before, count) = (bytes(&mut app, map), entries(&app));
    assert_eq!(
        finished(&mut app, WRITE, &at(map, 0, 1, &said)),
        "no cell changed"
    );
    assert_eq!((bytes(&mut app, map), entries(&app)), (before, count));
    let tile = "legend: {\"A\": \"tile:0:3\"}\nrows:\n.A\n";
    let said = finished(&mut app, WRITE, &at(map, 0, 0, tile));
    assert_eq!(said, "1 cells written on layer `Ground`");
    let data = stored(&mut app, map);
    assert_eq!(
        data.layers[0].tiles.get(CellCoord::new(1, 0)),
        TileRef::new(3)
    );
}

/// Runs `write_region` with `params`, takes its cells, undoes it, then runs
/// `tilemap.fill_rect` with `twin` and expects the same cells.
#[track_caller]
fn assert_writes_like(app: &mut App, map: Entity, params: &Params, twin: &Params) {
    let said = finished(app, WRITE, params);
    assert_ne!(said, "no cell changed");
    let written = bytes(app, map);
    run(app, "history.undo");
    finished(app, "tilemap.fill_rect", twin);
    assert_eq!(bytes(app, map), written);
}

#[test]
fn a_path_write_links_across_cells_already_holding_its_terrain() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let entity = ("entity", PropertyValue::Entity(map));
    let one = with(
        rect(1, 8, 1, 8),
        &[entity.clone(), ("terrain", text("water"))],
    );
    finished(&mut app, "tilemap.fill_rect", &one);
    let region = "legend: {\"w\": \"water\"}\nrows:\nwww\n";
    let path = with(at(map, 0, 8, region), &[("mode", text("path"))]);
    let twin = with(
        rect(0, 8, 2, 8),
        &[entity, ("terrain", text("water")), ("mode", text("path"))],
    );
    assert_writes_like(&mut app, map, &path, &twin);
}

#[test]
fn a_connect_write_over_path_links_clears_them() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let entity = ("entity", PropertyValue::Entity(map));
    let water = ("terrain", text("water"));
    let path = with(
        rect(0, 8, 2, 8),
        &[entity.clone(), water.clone(), ("mode", text("path"))],
    );
    finished(&mut app, "tilemap.fill_rect", &path);
    let region = "legend: {\"w\": \"water\"}\nrows:\nwww\n";
    let connect = rect(0, 8, 2, 8);
    assert_writes_like(
        &mut app,
        map,
        &at(map, 0, 8, region),
        &with(connect, &[entity, water]),
    );
}

#[test]
fn write_region_is_one_entry_undone_and_redone_exactly() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let (before, count) = (bytes(&mut app, map), entries(&app));
    let region = "legend: {\"d\": \"dirt\"}\nrows:\n  dd?\n  ?.d\n";
    let said = finished(&mut app, WRITE, &at(map, 0, 5, region));
    assert_eq!(said, "4 cells written on layer `Ground`");
    assert_eq!(entries(&app), count + 1);
    assert_eq!(last_label(&app), "Write tilemap region");
    let terrain: Vec<u8> = [(0, 5), (1, 5), (2, 5), (0, 4), (1, 4), (2, 4)]
        .into_iter()
        .map(|(x, y)| terrain_at(&mut app, map, x, y))
        .collect();
    assert_eq!(terrain, [7, 7, 1, 1, 0, 7]);
    let after = bytes(&mut app, map);
    run(&mut app, "history.undo");
    assert_eq!(bytes(&mut app, map), before);
    run(&mut app, "history.redo");
    assert_eq!(bytes(&mut app, map), after);
}

#[test]
fn writing_back_what_was_read_changes_nothing() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], marked_patch());
    let said = finished(&mut app, READ, &read(map, -1, 6, 6, -1));
    let (before, count) = (bytes(&mut app, map), entries(&app));
    assert_eq!(
        finished(&mut app, WRITE, &at(map, -1, 6, &said)),
        "no cell changed"
    );
    assert_eq!((bytes(&mut app, map), entries(&app)), (before, count));

    let path = with(
        rect(2, 8, 4, 9),
        &[
            ("entity", PropertyValue::Entity(map)),
            ("terrain", text("water")),
            ("mode", text("path")),
        ],
    );
    finished(&mut app, "tilemap.fill_rect", &path);
    let said = finished(&mut app, READ, &read(map, 2, 9, 4, 8));
    let (before, count) = (bytes(&mut app, map), entries(&app));
    let params = with(at(map, 2, 9, &said), &[("mode", text("path"))]);
    assert_eq!(finished(&mut app, WRITE, &params), "no cell changed");
    assert_eq!((bytes(&mut app, map), entries(&app)), (before, count));
}

#[test]
fn writing_back_a_tile_layer_changes_nothing() {
    let (mut app, _dir) = project_app();
    let cells = [(0, 0, TileRef::new(3)), (2, 1, TileRef::new(7))];
    let tiles = tile_map(&mut app, &cells);
    let said = finished(&mut app, READ, &read(tiles, 0, 1, 2, 0));
    let (before, count) = (bytes(&mut app, tiles), entries(&app));
    assert_eq!(
        finished(&mut app, WRITE, &at(tiles, 0, 1, &said)),
        "no cell changed"
    );
    assert_eq!((bytes(&mut app, tiles), entries(&app)), (before, count));
}

#[test]
fn malformed_text_is_refused_with_its_line_and_column() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    for (region, why) in [
        (
            "legend: {\"d\": \"dirt\"}\nrows:\ndd\ndz\n",
            "line 4, column 2: unknown symbol 'z'",
        ),
        (
            "legend: {\"d\": \"dirt\"}\nrows:\nddd\nd\n",
            "line 4, column 2: row has 1 cells, expected 3",
        ),
        ("legend: {\"d\": dirt}\nrows:\nd\n", "line 1, column 15"),
        (
            "legend: {\"dd\": \"dirt\"}\nrows:\nd\n",
            "line 1: invalid legend key",
        ),
        ("legend: {\"d\": \"dirt\"}\nd\n", "no `rows:` line"),
        (
            "legend: {\"d\": \"dirt\"}\nrows:\n\n",
            "line 3, column 1: region has no rows",
        ),
    ] {
        assert_refused(&mut app, map, WRITE, &at(map, 0, 5, region), why);
    }
}

#[test]
fn unknown_names_are_refused_before_any_cell_is_written() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let lava = "legend: {\"d\": \"dirt\", \"l\": \"lava\"}\nrows:\ndd\n";
    assert_refused(
        &mut app,
        map,
        WRITE,
        &at(map, 0, 5, lava),
        "legend `l`: no terrain is `lava`; terrains are grass, grass 2",
    );
    let tile = "legend: {\"T\": \"tile:0:99999\"}\nrows:\n.T\n";
    assert_refused(&mut app, map, WRITE, &at(map, 0, 5, tile), "out of range");
}

#[test]
fn terrain_is_refused_on_a_tile_layer() {
    let (mut app, _dir) = project_app();
    let tiles = tile_map(&mut app, &[(0, 0, TileRef::new(3))]);
    let grass = "legend: {\"g\": \"grass\"}\nrows:\ng\n";
    assert_refused(
        &mut app,
        tiles,
        WRITE,
        &at(tiles, 0, 0, grass),
        "layer `Ground` is a tile layer",
    );
}

#[test]
fn a_region_past_the_size_cap_is_refused() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    assert_refused(
        &mut app,
        map,
        READ,
        &read(map, 0, 0, 199, 199),
        "the region holds 40000 cells, more than 16384",
    );
    let row = format!("{}\n", ".".repeat(200));
    let region = format!("rows:\n{}", row.repeat(100));
    assert_refused(
        &mut app,
        map,
        WRITE,
        &at(map, 0, 0, &region),
        "the region holds 20000 cells, more than 16384",
    );
}

#[test]
fn the_map_and_layer_are_named_or_taken_from_the_selection() {
    let (mut app, _dir) = project_app();
    let map = two_layer_map(&mut app);
    app.world_mut()
        .resource_mut::<jackdaw::selection::Selection>()
        .entities
        .clear();
    let write = vec![("x0", int(0)), ("y0", int(0)), ("text", text("rows:\n.\n"))];
    for (id, params) in [(DESCRIBE, vec![]), (READ, rect(0, 0, 0, 0)), (WRITE, write)] {
        assert_refused(&mut app, map, id, &params, "no tilemap given or selected");
    }
    app.world_mut()
        .resource_mut::<jackdaw::selection::Selection>()
        .entities = vec![map];
    let props = with(rect(0, 0, 0, 0), &[("layer", text("Props"))]);
    let said = finished(&mut app, READ, &props);
    assert_eq!(
        said,
        "legend: {\".\": \"empty\", \"A\": \"tile:0:3\"}\nrows:\nA\n"
    );
    let by_index = with(rect(0, 0, 0, 0), &[("layer", int(1))]);
    assert_eq!(finished(&mut app, READ, &by_index), said);
    let sky = with(rect(0, 0, 0, 0), &[("layer", text("Sky"))]);
    assert_refused(&mut app, map, READ, &sky, "no layer `Sky`");
    let missing = vec![("x0", int(0)), ("y0", int(0))];
    assert_refused(&mut app, map, READ, &missing, "x1 is required");
    assert_refused(&mut app, map, WRITE, &missing, "text is required");
    let mode = vec![
        ("x0", int(0)),
        ("y0", int(0)),
        ("text", text("rows:\n.\n")),
        ("mode", text("spiral")),
    ];
    assert_refused(&mut app, map, WRITE, &mode, "mode is connect or path");
}
