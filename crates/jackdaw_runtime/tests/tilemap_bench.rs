#![cfg(feature = "tilemap")]
//! Timings for a 512 x 512 map of five layers: its sidecar's encode and
//! decode, and a game loading it to its first drawn frame, with and without
//! collision. Run with
//! `cargo test -p jackdaw_runtime --features tilemap --test tilemap_bench -- --ignored --nocapture --test-threads=1`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use bevy::asset::AssetApp;
use bevy::prelude::*;
use bevy::sprite_render::{TilemapChunkMaterial, TilemapChunkMeshCache};
use jackdaw_runtime::{
    JackdawCatalogPath, JackdawPlugin, JackdawScene, JackdawSceneRoot, TilemapChunkOf,
    TilemapCollision,
};
use jackdaw_scene_types::Tilemap;
use jackdaw_tilemap::{
    CellCoord, ChunkedLayer, LayerData, LayerKind, Placement, TerrainAuthoring, TileRef,
    TilemapData,
};
use jackdaw_tileset::CollisionPreset;

const SHEET: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../jackdaw_tilemap/tests/data/punyworld/punyworld-overworld-tileset.png"
);

const SIZE: i32 = 512;
const RUNS: usize = 7;
const SIDECAR: &str = "large.tilemap-0.jdtiles";

/// Tiles the walls layer draws; each has a shape when the map collides.
const WALL_TILES: [u16; 4] = [100, 101, 102, 103];
/// Tiles of the walls layer with a slope instead of a full box.
const SLOPE_TILE: u16 = 104;

/// The layers by id, bottom to top: one terrain layer and four tile layers.
const LAYERS: [(u32, &str, &str); 5] = [
    (0, "Ground", "Terrain"),
    (1, "Floor", "Tile"),
    (2, "Walls", "Tile"),
    (3, "Decor", "Tile"),
    (4, "Overlay", "Tile"),
];

fn hash(x: i32, y: i32, salt: u32) -> u32 {
    let mut h = (x as u32).wrapping_mul(0x9e37_79b9) ^ (y as u32).wrapping_mul(0x85eb_ca6b) ^ salt;
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
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

/// Rooms walled every 12 cells with gaps for doors, and a slope now and then.
fn wall(x: i32, y: i32) -> Option<TileRef> {
    let h = hash(x, y, 2);
    if (x % 12 == 0 || y % 12 == 0) && !h.is_multiple_of(5) {
        Some(TileRef::new(WALL_TILES[(h % 4) as usize]))
    } else if h.is_multiple_of(97) {
        Some(TileRef::new(SLOPE_TILE))
    } else {
        None
    }
}

/// The same map on every run: terrain in two blobby kinds under a full
/// floor, walls, sparse decor and a sparser overlay, all over 256 chunks per
/// layer.
fn large_map() -> TilemapData {
    let mut terrain = ChunkedLayer::new();
    let mut ground = ChunkedLayer::new();
    for y in 0..SIZE {
        for x in 0..SIZE {
            let kind = if (x / 24 + y / 24) % 3 == 0 { 2 } else { 1 };
            terrain.set(CellCoord::new(x, y), kind);
            let tile = 27 * u16::from(kind) + (hash(x, y, 0) % 4) as u16;
            ground.set(CellCoord::new(x, y), TileRef::new(tile));
        }
    }
    let ground = LayerData {
        kind: LayerKind::Terrain,
        placement: Placement::OnCells,
        tiles: ground,
        authoring: Some(TerrainAuthoring {
            terrain,
            ..default()
        }),
    };
    let floor = tile_layer(|x, y| Some(TileRef::new(200 + (hash(x, y, 1) % 8) as u16)));
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
    TilemapData {
        layers: [
            (0, ground),
            (1, floor),
            (2, tile_layer(wall)),
            (3, decor),
            (4, overlay),
        ]
        .into(),
    }
}

fn polygons_text(preset: CollisionPreset) -> String {
    let rings = preset.polygons().expect("a preset shape");
    let rings: Vec<String> = (rings.iter())
        .map(|ring| {
            let points: Vec<String> = (ring.iter())
                .map(|p| format!("glam::Vec2 {{ x: {:?}, y: {:?} }}", p.x, p.y))
                .collect();
            format!("[{}]", points.join(", "))
        })
        .collect();
    format!("[{}]", rings.join(", "))
}

/// The overworld sheet as one 27 x 65 sheet of 16 px tiles, its wall tiles
/// solid when `collides`.
fn tileset_text(collides: bool) -> String {
    let mut tiles = Vec::new();
    if collides {
        let shapes = (WALL_TILES.iter().map(|&t| (t, CollisionPreset::Solid)))
            .chain([(SLOPE_TILE, CollisionPreset::SlopeUp)]);
        for (tile, preset) in shapes {
            tiles.push(format!(
                "jackdaw_tileset::TilesetTile {{ tile: {tile}, collision: [\
                 jackdaw_tileset::TilesetCollision {{ preset: jackdaw_tileset::CollisionPreset::{preset:?}, \
                 polygons: {} }}] }}",
                polygons_text(preset)
            ));
        }
    }
    format!(
        "#overworld\n\
         jackdaw_tileset::Tileset {{\n    \
         image: \"overworld.png\",\n    \
         tile_size: glam::UVec2 {{ x: 16, y: 16 }},\n    \
         grid: glam::UVec2 {{ x: 27, y: 65 }},\n    \
         sheets: [\n        \
         jackdaw_tileset::TilesetSheet {{ source: \"source.png\", columns: 27, rows: 65, first_layer: 0 }},\n    \
         ],\n    \
         physics_layers: [\"World\"],\n    \
         tiles: [{}],\n\
         }}\n",
        tiles.join(", ")
    )
}

fn map_text() -> String {
    let layers: Vec<String> = (LAYERS.iter())
        .map(|(id, name, kind)| {
            format!(
                "jackdaw_scene_types::tilemap::TilemapLayer {{ id: {id}, name: \"{name}\", \
                 tileset: \"overworld.tileset.bsn\", \
                 kind: jackdaw_scene_types::tilemap::TilemapLayerKind::{kind} }}"
            )
        })
        .collect();
    format!(
        "#Map\n\
         jackdaw_scene_types::tilemap::Tilemap {{\n    \
         data_path: \"{SIDECAR}\",\n    \
         layers: [{}],\n    \
         next_layer_id: {},\n\
         }}\n",
        layers.join(", "),
        LAYERS.len()
    )
}

fn project(sidecar: &[u8], collides: bool) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let assets = tmp.path().join("assets");
    std::fs::create_dir_all(&assets).expect("assets");
    std::fs::write(assets.join("catalog.bsn"), "// no catalog entries\n").expect("catalog");
    std::fs::copy(SHEET, assets.join("overworld.png")).expect("sheet");
    std::fs::write(assets.join("overworld.tileset.bsn"), tileset_text(collides)).expect("tileset");
    std::fs::write(assets.join(SIDECAR), sidecar).expect("sidecar");
    tmp
}

fn game(assets: &Path, collides: bool) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin {
        file_path: assets.to_string_lossy().into_owned(),
        ..default()
    });
    app.insert_resource(JackdawCatalogPath(assets.join("catalog.bsn")));
    app.init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_asset::<TilemapChunkMaterial>()
        .init_resource::<TilemapChunkMeshCache>();
    if collides {
        app.register_required_components::<Tilemap, TilemapCollision>();
    }
    app.add_plugins(JackdawPlugin);
    app.update();
    app
}

fn drawn_chunks(app: &mut App) -> usize {
    (app.world_mut().query::<&TilemapChunkOf>())
        .iter(app.world())
        .count()
}

fn collision_chunks(app: &mut App) -> usize {
    (app.world_mut().query::<&TilemapCollision>())
        .iter(app.world())
        .map(|collision| collision.chunks.len())
        .sum()
}

/// Spawns the map's scene and returns how long it took until every chunk
/// of every layer drew.
fn load_once(sidecar: &[u8], collides: bool, chunks: usize) -> (Duration, usize) {
    let project = project(sidecar, collides);
    let assets = project.path().join("assets");
    let mut app = game(&assets, collides);
    let started = Instant::now();
    let handle = (app.world_mut().resource_mut::<Assets<JackdawScene>>())
        .add(JackdawScene::new(map_text(), PathBuf::new()));
    app.world_mut().spawn(JackdawSceneRoot(handle));
    while drawn_chunks(&mut app) < chunks {
        assert!(
            started.elapsed() < Duration::from_secs(120),
            "the map never drew"
        );
        app.update();
    }
    (started.elapsed(), collision_chunks(&mut app))
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

fn time(runs: usize, mut f: impl FnMut()) -> Vec<Duration> {
    (0..runs)
        .map(|_| {
            let started = Instant::now();
            f();
            started.elapsed()
        })
        .collect()
}

fn chunk_count(data: &TilemapData) -> usize {
    data.layers.values().map(|l| l.tiles.chunk_count()).sum()
}

#[test]
#[ignore = "benchmark"]
#[expect(clippy::print_stderr, reason = "reports the map's size")]
fn large_map_sidecar_encode_and_decode() {
    let data = large_map();
    let bytes = jackdaw_tilemap::encode(&data);
    eprintln!(
        "map: {SIZE} x {SIZE} cells, {} layers, {} chunks, sidecar {} bytes",
        data.layers.len(),
        chunk_count(&data),
        bytes.len()
    );
    report(
        "sidecar encode",
        time(RUNS, || {
            std::hint::black_box(jackdaw_tilemap::encode(std::hint::black_box(&data)));
        }),
    );
    report(
        "sidecar decode",
        time(RUNS, || {
            let decoded = jackdaw_tilemap::decode(std::hint::black_box(&bytes));
            std::hint::black_box(decoded.expect("the sidecar decodes"));
        }),
    );
}

#[test]
#[ignore = "benchmark"]
fn large_map_runtime_load_to_first_drawn_frame() {
    let data = large_map();
    let bytes = jackdaw_tilemap::encode(&data);
    let chunks = chunk_count(&data);
    let times = (0..RUNS)
        .map(|_| load_once(&bytes, false, chunks).0)
        .collect();
    report("runtime load to first drawn frame", times);
}

#[test]
#[ignore = "benchmark"]
fn large_map_runtime_load_with_collision() {
    let data = large_map();
    let bytes = jackdaw_tilemap::encode(&data);
    let chunks = chunk_count(&data);
    let walls = data.layers[&2].tiles.chunk_count();
    let times = (0..RUNS)
        .map(|_| {
            let (took, collided) = load_once(&bytes, true, chunks);
            assert_eq!(collided, walls, "every walls chunk collides");
            took
        })
        .collect();
    report("runtime load with collision", times);
}
