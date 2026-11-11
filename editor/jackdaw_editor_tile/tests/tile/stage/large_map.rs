//! Timings for a 512 x 512 map of five layers in the editor: opening its saved
//! scene to the first drawn frame, resolving its terrain layer whole, a 64 cell
//! stroke, its undo and the frame time with the map open. Run with
//! `cargo test -p jackdaw_editor_tile --test tile large_map -- --ignored --nocapture --test-threads=1`.

use crate::stage::tilemap_render::{TILESET, chunks, key_of, settle, spawn_map, the_map};
use crate::util;

use std::path::Path;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::sprite_render::TilemapChunkTileData;
use jackdaw_api::prelude::*;
use jackdaw_editor_tile::tilemap::TilemapDataStore;
use jackdaw_editor_tile::tilemap_render::TilemapChunkOf;
use jackdaw_editor_tile::tilemap_resolve::full_resolve;
use jackdaw_scene_types::{LayerTerrainMatch, PropertyValue, TilemapLayer, TilemapLayerKind};
use jackdaw_tilemap::{
    CellCoord, ChunkedLayer, LayerData, LayerKind, Placement, TerrainAuthoring, TileRef,
    TilemapData,
};
use jackdaw_tileset::{TerrainMatch, Tileset};

const SIZE: i32 = 512;
const RUNS: usize = 5;
const STROKES: usize = 7;
const FRAMES: usize = 120;
const STROKE_FROM: i32 = 24;
const STROKE_CELLS: i32 = 64;

fn hash(x: i32, y: i32, salt: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9e37_79b9) ^ (y as u32).wrapping_mul(0x85eb_ca6b) ^ salt;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

/// Grass and the terrain it pairs with in the overworld's corners and sides
/// set, in blobs.
fn terrain_at(x: i32, y: i32) -> u8 {
    if (x / 24 + y / 24) % 3 == 0 { 2 } else { 1 }
}

fn tile_layer(tile: impl Fn(i32, i32) -> Option<TileRef>) -> LayerData {
    let mut tiles = ChunkedLayer::new();
    for y in 0..SIZE {
        for x in 0..SIZE {
            if let Some(t) = tile(x, y) {
                tiles.set(CellCoord::new(x, y), t);
            }
        }
    }
    LayerData {
        kind: LayerKind::Tile,
        placement: Placement::OnCells,
        tiles,
        authoring: None,
    }
}

fn named(name: &str, kind: TilemapLayerKind) -> TilemapLayer {
    TilemapLayer {
        name: name.to_string(),
        tileset: TILESET.to_string(),
        kind,
        terrain_match: LayerTerrainMatch::CornersAndSides,
        ..default()
    }
}

/// The same map on every run: terrain in two kinds under a full floor, walled
/// rooms, sparse decor and a sparser overlay, over 256 chunks per layer. The
/// terrain layer's tiles are left for the editor to resolve.
fn large_map() -> (Vec<TilemapLayer>, TilemapData) {
    let mut terrain = ChunkedLayer::new();
    for y in 0..SIZE {
        for x in 0..SIZE {
            terrain.set(CellCoord::new(x, y), terrain_at(x, y));
        }
    }
    let ground = LayerData {
        kind: LayerKind::Terrain,
        placement: Placement::OnCells,
        tiles: ChunkedLayer::new(),
        authoring: Some(TerrainAuthoring {
            terrain,
            ..default()
        }),
    };
    let floor = tile_layer(|x, y| Some(TileRef::new(200 + (hash(x, y, 1) % 8) as u16)));
    let walls = tile_layer(|x, y| {
        let h = hash(x, y, 2);
        ((x % 12 == 0 || y % 12 == 0) && !h.is_multiple_of(5))
            .then(|| TileRef::new(100 + (h % 4) as u16))
    });
    let decor = tile_layer(|x, y| {
        let h = hash(x, y, 3);
        h.is_multiple_of(10)
            .then(|| TileRef::from_raw(300 + (h / 10 % 16) as u16, (h % 8) as u8))
    });
    let overlay = tile_layer(|x, y| {
        let h = hash(x, y, 4);
        h.is_multiple_of(20)
            .then(|| TileRef::new(400 + (h / 20 % 8) as u16))
    });
    let layers = vec![
        named("Ground", TilemapLayerKind::Terrain),
        named("Floor", TilemapLayerKind::Tile),
        named("Walls", TilemapLayerKind::Tile),
        named("Decor", TilemapLayerKind::Tile),
        named("Overlay", TilemapLayerKind::Tile),
    ];
    let data = TilemapData {
        layers: [
            (0, ground),
            (1, floor),
            (2, walls),
            (3, decor),
            (4, overlay),
        ]
        .into(),
    };
    (layers, data)
}

fn update_until(app: &mut App, what: &str, mut done: impl FnMut(&mut App) -> bool) {
    let started = Instant::now();
    while !done(app) {
        assert!(started.elapsed() < Duration::from_secs(300), "{what}");
        app.update();
    }
}

fn drawn_chunks(app: &mut App) -> usize {
    (app.world_mut().query::<&TilemapChunkOf>())
        .iter(app.world())
        .count()
}

/// Generates the map in an editor on a fresh project, lets it resolve and
/// draw, and saves it. Returns the project, the scene and its chunk count.
fn saved_project() -> (tempfile::TempDir, std::path::PathBuf, usize) {
    let (mut app, dir) = crate::stage::tilemap_render::project_app();
    let (layers, data) = large_map();
    let map = spawn_map(&mut app, layers, data);
    let key = key_of(&mut app, map);
    update_until(&mut app, "the generated map resolves and draws", |app| {
        let store = app.world().resource::<TilemapDataStore>();
        let data = store.get(&key).expect("the map has cells");
        let resolved = data.layers[&0].tiles.chunk_count() > 0;
        let stored: usize = data.layers.values().map(|l| l.tiles.chunk_count()).sum();
        resolved && drawn_chunks(app) == stored
    });
    let scene = dir.path().join("assets").join("large_map.bsn");
    jackdaw_editor_core::scene_io::retarget_active_scene(app.world_mut(), &scene.to_string_lossy())
        .expect("retarget");
    assert!(jackdaw_editor_core::scene_io::save_scene(app.world_mut()));
    settle(&mut app);
    let count = chunks(&mut app, map).len();
    (dir, scene, count)
}

fn editor_on(project: &Path) -> App {
    let mut app = util::editor_test_app_reading(&project.join("assets"));
    app.world_mut()
        .insert_resource(jackdaw_editor_core::project::ProjectRoot {
            root: project.to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw_editor_core::AppState>>()
        .set(jackdaw_editor_core::AppState::Editor);
    settle(&mut app);
    app
}

fn frame_times(app: &mut App) -> Vec<Duration> {
    for _ in 0..30 {
        app.update();
    }
    time(FRAMES, || app.update())
}

fn time(runs: usize, mut f: impl FnMut()) -> Vec<Duration> {
    (0..runs)
        .map(|_| {
            let started = Instant::now();
            f();
            started.elapsed()
        })
        .collect()
}

fn median(mut times: Vec<Duration>) -> f64 {
    times.sort();
    times[times.len() / 2].as_secs_f64() * 1000.0
}

#[expect(clippy::print_stderr, reason = "reports the timings")]
fn report(name: &str, times: Vec<Duration>) {
    let runs = times.len();
    eprintln!("bench {name}: {:.2} ms (n={runs})", median(times));
}

/// The tile the chunk drawn for `cell` on layer `layer` shows there.
fn drawn_at(app: &mut App, map: Entity, layer: usize, cell: CellCoord) -> Option<u16> {
    let chunk = cell.to_chunk_local().0;
    let entity = (chunks(app, map).into_iter())
        .find(|(l, c, _)| *l == layer && *c == chunk.0)
        .map(|(.., entity)| entity)?;
    let data = app.world().get::<TilemapChunkTileData>(entity)?;
    data.0[cell.local_index()].map(|tile| tile.tileset_index)
}

/// Times `f` from its call until `cell` on `layer` draws something other
/// than it did before.
fn until_redrawn(
    app: &mut App,
    map: Entity,
    layer: usize,
    cell: CellCoord,
    f: impl FnOnce(&mut App),
) -> Duration {
    let before = drawn_at(app, map, layer, cell);
    let started = Instant::now();
    f(app);
    update_until(app, "the edit draws", |app| {
        drawn_at(app, map, layer, cell) != before
    });
    started.elapsed()
}

fn set_cells(app: &mut App, map: Entity, layer: &str, kind: &str, cells: &str, value: &str) {
    let result = app
        .world_mut()
        .operator("tilemap.set_cells")
        .param("entity", PropertyValue::Entity(map))
        .param("layer", PropertyValue::from(layer.to_string()))
        .param("kind", PropertyValue::from(kind.to_string()))
        .param("cells", PropertyValue::from(cells.to_string()))
        .param("value", PropertyValue::from(value.to_string()))
        .call()
        .expect("tilemap.set_cells dispatches");
    assert_eq!(result, OperatorResult::Finished);
}

fn undo(app: &mut App) {
    let result = (app.world_mut().operator("history.undo").call()).expect("dispatches");
    assert_eq!(result, OperatorResult::Finished);
}

fn row(y: i32) -> String {
    let cells: Vec<String> = (STROKE_FROM..STROKE_FROM + STROKE_CELLS)
        .map(|x| format!("{x},{y}"))
        .collect();
    cells.join(";")
}

/// Paints a row of tiles on the walls layer and a row of terrain on the
/// ground, `STROKES` times each, undoing each stroke after it draws.
fn strokes(app: &mut App, map: Entity) {
    let (mut tile, mut tile_undo) = (Vec::new(), Vec::new());
    for (run, y) in (0..STROKES).zip((101..).step_by(2)) {
        let cell = CellCoord::new(STROKE_FROM + 1, y);
        let value = (500 + run).to_string();
        let cells = row(y);
        tile.push(until_redrawn(app, map, 2, cell, |app| {
            set_cells(app, map, "Walls", "tile", &cells, &value);
        }));
        tile_undo.push(until_redrawn(app, map, 2, cell, undo));
    }
    report("editor stroke of 64 tiles", tile);
    report("editor undo of a tile stroke", tile_undo);

    let (mut terrain, mut terrain_undo) = (Vec::new(), Vec::new());
    for y in (201..).step_by(2).take(STROKES) {
        let x = (STROKE_FROM..STROKE_FROM + STROKE_CELLS)
            .find(|&x| terrain_at(x, y) == 1)
            .expect("grass under the stroke");
        let cell = CellCoord::new(x, y);
        let cells = row(y);
        terrain.push(until_redrawn(app, map, 0, cell, |app| {
            set_cells(app, map, "Ground", "terrain", &cells, "2");
        }));
        terrain_undo.push(until_redrawn(app, map, 0, cell, undo));
    }
    report("editor stroke of 64 terrain cells", terrain);
    report("editor undo of a terrain stroke", terrain_undo);
}

/// Resolves the map's terrain layer whole, as opening the map does.
fn full_resolves(app: &mut App, map: Entity) -> Vec<Duration> {
    let tileset = (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(_, t)| t.clone())
        .expect("the tileset");
    let set = (tileset.terrain_set(Some(TerrainMatch::CornersAndSides)))
        .expect("a corners and sides set");
    let key = key_of(app, map);
    let store = app.world().resource::<TilemapDataStore>();
    let data = store.get(&key).expect("the map has cells");
    let authoring = data.layers[&0].authoring.as_ref().expect("authoring");
    time(RUNS, || {
        std::hint::black_box(full_resolve(&set, authoring, None));
    })
}

#[test]
#[ignore = "benchmark"]
fn large_map_in_the_editor() {
    let (project, scene, count) = saved_project();
    let mut open = Vec::new();
    let mut app = None;
    for _ in 0..RUNS {
        let mut editor = editor_on(project.path());
        if app.is_none() {
            report("editor frame with an empty scene", frame_times(&mut editor));
        }
        let started = Instant::now();
        jackdaw_editor_core::scene_io::load_scene_from_file(editor.world_mut(), &scene);
        update_until(&mut editor, "the opened map draws", |app| {
            drawn_chunks(app) >= count
        });
        open.push(started.elapsed());
        assert_eq!(drawn_chunks(&mut editor), count);
        app = Some(editor);
    }
    report("editor open to first drawn frame", open);

    let mut app = app.expect("an editor");
    report("editor frame with the map open", frame_times(&mut app));
    let map = the_map(&mut app);
    report(
        "editor terrain layer resolved whole",
        full_resolves(&mut app, map),
    );
    strokes(&mut app, map);
    report("editor frame after the strokes", frame_times(&mut app));
}
