#![cfg(feature = "tilemap")]
//! A scene's tilemap drawn the way a built game draws it: the sidecar is read
//! from beside the document that names the map, and each layer's stored tiles
//! spawn as tilemap chunks laid out by the builder the editor draws with.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy::asset::AssetApp;
use bevy::prelude::*;
use bevy::sprite_render::{
    TilemapChunk, TilemapChunkMaterial, TilemapChunkMeshCache, TilemapChunkTileData,
};
use jackdaw_runtime::{
    JackdawCatalogPath, JackdawPlugin, JackdawScene, JackdawSceneRoot, RuntimeOnly, TilemapChunkOf,
    TilemapSidecarMissing,
};
use jackdaw_scene_types::Tilemap;
use jackdaw_tilemap::{
    CellCoord, ChunkCoord, ChunkedLayer, LayerData, LayerKind, TileRef, TilemapData,
};
use jackdaw_tileset::Tileset;
use jackdaw_tileset::chunk::{ChunkDraw, chunk_transform, tile_data};

const SHEET: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../jackdaw_tilemap/tests/data/punyworld/punyworld-overworld-tileset.png"
);

/// The overworld sheet as one 27 x 65 sheet of 16 px tiles.
const TILESET: &str = "\
#overworld
jackdaw_tileset::Tileset {
    image: \"overworld.png\",
    tile_size: glam::UVec2 { x: 16, y: 16 },
    grid: glam::UVec2 { x: 27, y: 65 },
    sheets: [
        jackdaw_tileset::TilesetSheet {
            source: \"overworld.png\",
            columns: 27,
            rows: 65,
            first_layer: 0,
        },
    ],
}
";

fn map_text(name: &str, data_path: &str) -> String {
    map_text_with(name, data_path, "overworld.tileset.bsn")
}

fn map_text_with(name: &str, data_path: &str, tileset: &str) -> String {
    format!(
        "#{name}\n\
         jackdaw_scene_types::tilemap::Tilemap {{\n    \
         data_path: \"{data_path}\",\n    \
         layers: [\n        \
         jackdaw_scene_types::tilemap::TilemapLayer {{ name: \"Ground\", tileset: \"{tileset}\", kind: jackdaw_scene_types::tilemap::TilemapLayerKind::Tile }},\n        \
         jackdaw_scene_types::tilemap::TilemapLayer {{ name: \"Decor\", tileset: \"{tileset}\", kind: jackdaw_scene_types::tilemap::TilemapLayerKind::Tile }},\n    \
         ],\n\
         }}\n"
    )
}

fn layer(cells: &[(i32, i32, TileRef)]) -> LayerData {
    let mut tiles = ChunkedLayer::new();
    for &(x, y, tile) in cells {
        tiles.set(CellCoord::new(x, y), tile);
    }
    LayerData {
        kind: LayerKind::Tile,
        tiles,
        authoring: None,
    }
}

/// Two layers: ground across two chunks, one of them at negative cells, and
/// a flipped decor tile.
fn painted() -> TilemapData {
    TilemapData {
        layers: vec![
            layer(&[
                (0, 0, TileRef::new(3)),
                (5, 7, TileRef::from_raw(40, 0b011)),
                (-1, -1, TileRef::new(100)),
            ]),
            layer(&[(2, 2, TileRef::from_raw(7, 0b100))]),
        ],
    }
}

fn project() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let assets = tmp.path().join("assets");
    std::fs::create_dir_all(&assets).expect("assets");
    std::fs::write(assets.join("catalog.bsn"), "// no catalog entries\n").expect("catalog");
    std::fs::copy(SHEET, assets.join("overworld.png")).expect("sheet");
    std::fs::write(assets.join("overworld.tileset.bsn"), TILESET).expect("tileset");
    let other = TILESET
        .replace("#overworld", "#other")
        .replace("overworld.png", "other.png");
    std::fs::write(assets.join("other.tileset.bsn"), other).expect("other tileset");
    let blank = TILESET
        .replace("#overworld", "#blank")
        .replace("    image: \"overworld.png\",\n", "");
    std::fs::write(assets.join("blank.tileset.bsn"), blank).expect("blank tileset");
    tmp
}

fn write_sidecar(path: &Path, data: &TilemapData) {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).expect("sidecar dir");
    }
    std::fs::write(path, jackdaw_tilemap::encode(data)).expect("sidecar");
}

fn game(assets_root: &Path, scene: &str) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin {
        file_path: assets_root.to_string_lossy().into_owned(),
        ..default()
    });
    // What a windowed game gets from its sprite plugins.
    app.init_asset::<Mesh>()
        .init_asset::<Image>()
        .init_asset::<TilemapChunkMaterial>()
        .init_resource::<TilemapChunkMeshCache>();
    app.insert_resource(JackdawCatalogPath(assets_root.join("catalog.bsn")));
    app.add_plugins(JackdawPlugin);
    let handle = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(scene.to_string(), PathBuf::new()));
    app.world_mut().spawn(JackdawSceneRoot(handle));
    app.update();
    app.update();
    app
}

/// Runs `f` with the warnings it logs written to a buffer, and returns them.
fn warnings_of<T>(f: impl FnOnce() -> T) -> (T, String) {
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().expect("buffer").extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = bevy::log::tracing_subscriber::fmt()
        .with_max_level(bevy::log::Level::WARN)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let out = bevy::log::tracing::subscriber::with_default(subscriber, f);
    let text = String::from_utf8_lossy(&buffer.0.lock().expect("buffer")).into_owned();
    (out, text)
}

fn map_named(app: &mut App, name: &str) -> Entity {
    app.world_mut()
        .query::<(Entity, &Name, &Tilemap)>()
        .iter(app.world())
        .find(|(_, n, _)| n.as_str() == name)
        .map(|(entity, _, _)| entity)
        .expect("the map spawned")
}

/// Each chunk under `map`, by layer and chunk coordinate.
fn chunks_of(
    app: &mut App,
    map: Entity,
) -> Vec<(usize, IVec2, TilemapChunk, TilemapChunkTileData, Transform)> {
    let mut found: Vec<_> = app
        .world_mut()
        .query::<(
            &TilemapChunkOf,
            &ChildOf,
            &TilemapChunk,
            &TilemapChunkTileData,
            &Transform,
            Has<RuntimeOnly>,
        )>()
        .iter(app.world())
        .filter(|(_, parent, ..)| parent.parent() == map)
        .map(|(of, _, chunk, data, transform, runtime_only)| {
            assert!(runtime_only, "a drawn chunk is never streamed or saved");
            (
                of.layer,
                of.chunk.0,
                chunk.clone(),
                data.clone(),
                *transform,
            )
        })
        .collect();
    found.sort_by_key(|(layer, at, ..)| (*layer, at.x, at.y));
    found
}

fn tileset_of(app: &App) -> Tileset {
    let assets = app.world().resource::<Assets<Tileset>>();
    let (_, tileset) = (assets.iter())
        .find(|(_, tileset)| tileset.image == "overworld.png")
        .expect("the tileset loaded");
    tileset.clone()
}

#[test]
fn a_painted_map_draws_the_chunks_the_editor_builds() {
    let project = project();
    let assets = project.path().join("assets");
    let data = painted();
    write_sidecar(&assets.join("zone.tilemap-0.jdtiles"), &data);
    let mut app = game(&assets, &map_text("Map", "zone.tilemap-0.jdtiles"));

    let map = map_named(&mut app, "Map");
    let drawn = chunks_of(&mut app, map);
    let tileset = tileset_of(&app);
    let tile_size = UVec2::splat(16);

    let mut expected = Vec::new();
    for (index, layer) in data.layers.iter().enumerate() {
        for (coord, cells) in layer.tiles.chunks() {
            expected.push((index, coord.0, cells));
        }
    }
    expected.sort_by_key(|(layer, at, _)| (*layer, at.x, at.y));
    assert_eq!(
        drawn
            .iter()
            .map(|(l, at, ..)| (*l, *at))
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|(l, at, _)| (*l, *at))
            .collect::<Vec<_>>(),
        "one chunk per layer and filled block"
    );
    for ((layer, at, chunk, tiles, transform), (_, _, cells)) in drawn.iter().zip(&expected) {
        let draw = ChunkDraw::new(&tileset, chunk.tileset.clone());
        let (want_chunk, want_tiles, want_transform) =
            draw.bundle(ChunkCoord(*at), cells, tile_size, *layer);
        assert_eq!(chunk.tile_display_size, want_chunk.tile_display_size);
        assert_eq!(chunk.chunk_size, UVec2::splat(32));
        assert_eq!(tiles.len(), 1024);
        assert_eq!(*transform, want_transform);
        assert!(tiles.iter().zip(want_tiles.iter()).all(|(a, b)| {
            a.map(|t| (t.tileset_index, t.visible, t.orientation))
                == b.map(|t| (t.tileset_index, t.visible, t.orientation))
        }));
    }

    let (_, _, _, ground, at_origin) = &drawn[1];
    assert_eq!(
        *at_origin,
        chunk_transform(ChunkCoord(IVec2::ZERO), tile_size, 0)
    );
    assert_eq!(at_origin.translation, Vec3::new(256.0, 256.0, 0.0));
    let flipped = ground[7 * 32 + 5].expect("cell (5, 7) draws");
    let want = tile_data(TileRef::from_raw(40, 0b011)).expect("a filled cell");
    assert_eq!(
        (flipped.tileset_index, flipped.orientation),
        (want.tileset_index, want.orientation)
    );
    let (_, _, _, _, decor) = &drawn[2];
    assert_eq!(decor.translation.z, 1.0, "the second layer stacks above");
    assert!(app.world().get::<TilemapSidecarMissing>(map).is_none());
}

#[test]
fn a_missing_sidecar_loads_an_empty_map_and_warns() {
    let project = project();
    let assets = project.path().join("assets");
    let (mut app, logged) =
        warnings_of(|| game(&assets, &map_text("Map", "zone.tilemap-0.jdtiles")));

    let map = map_named(&mut app, "Map");
    assert!(chunks_of(&mut app, map).is_empty());
    assert!(app.world().get::<TilemapSidecarMissing>(map).is_some());
    assert!(
        logged.contains("zone.tilemap-0.jdtiles is missing"),
        "the missing file is named: {logged}"
    );
}

#[test]
fn a_prefab_instanced_map_reads_the_prefab_sidecar() {
    let project = project();
    let assets = project.path().join("assets");
    let prefab = format!(
        "#Hut\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         bevy_ecs::hierarchy::Children [\n\
         jackdaw::prefab::components::PrefabEntityId(1)\n\
         {}]\n",
        map_text("HutMap", "hut.tilemap-0.jdtiles")
    );
    std::fs::create_dir_all(assets.join("prefabs")).expect("prefabs");
    std::fs::write(assets.join("prefabs/hut.bsn"), prefab).expect("prefab");
    let inherited = TilemapData {
        layers: vec![layer(&[(1, 1, TileRef::new(9))]), layer(&[])],
    };
    write_sidecar(&assets.join("prefabs/hut.tilemap-0.jdtiles"), &inherited);
    write_sidecar(&assets.join("hut.tilemap-0.jdtiles"), &painted());

    let mut app = game(
        &assets,
        "jackdaw::prefab::components::IsA { source: \"prefabs/hut.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n",
    );

    let map = map_named(&mut app, "HutMap");
    let drawn = chunks_of(&mut app, map);
    assert_eq!(drawn.len(), 1, "the prefab's sidecar, not the scene's");
    let (layer, at, _, tiles, _) = &drawn[0];
    assert_eq!((*layer, *at), (0, IVec2::ZERO));
    assert_eq!(tiles[32 + 1].map(|t| t.tileset_index), Some(9));
}

#[test]
fn a_tileset_without_an_image_skips_its_layers_with_a_warning() {
    let project = project();
    let assets = project.path().join("assets");
    write_sidecar(&assets.join("zone.tilemap-0.jdtiles"), &painted());
    let scene = map_text_with("Map", "zone.tilemap-0.jdtiles", "blank.tileset.bsn");
    let (mut app, logged) = warnings_of(|| game(&assets, &scene));

    let map = map_named(&mut app, "Map");
    assert!(chunks_of(&mut app, map).is_empty());
    assert!(
        logged.contains("its tileset has no image"),
        "the editor's warning: {logged}"
    );
}

#[test]
fn a_sidecar_with_fewer_layers_draws_the_ones_that_line_up() {
    let project = project();
    let assets = project.path().join("assets");
    let one = TilemapData {
        layers: vec![layer(&[(0, 0, TileRef::new(3))])],
    };
    write_sidecar(&assets.join("zone.tilemap-0.jdtiles"), &one);
    let (mut app, logged) =
        warnings_of(|| game(&assets, &map_text("Map", "zone.tilemap-0.jdtiles")));

    let map = map_named(&mut app, "Map");
    let drawn = chunks_of(&mut app, map);
    assert_eq!(drawn.len(), 1);
    assert_eq!((drawn[0].0, drawn[0].1), (0, IVec2::ZERO));
    assert!(
        logged.contains("Tilemap Map: its sidecar holds 1 layers and the map names 2"),
        "the map is named: {logged}"
    );
}

#[test]
fn an_instance_of_a_missing_prefab_reads_beside_the_scene() {
    let project = project();
    let assets = project.path().join("assets");
    write_sidecar(&assets.join("here.tilemap-0.jdtiles"), &painted());

    let scene = format!(
        "{}jackdaw::prefab::components::IsA {{ source: \"prefabs/gone.bsn\", deleted: [] }}\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n",
        map_text("Here", "here.tilemap-0.jdtiles")
    );
    let mut app = game(&assets, &scene);

    let map = map_named(&mut app, "Here");
    assert_eq!(chunks_of(&mut app, map).len(), 3, "the scene's own sidecar");
    assert!(app.world().get::<TilemapSidecarMissing>(map).is_none());
}

#[test]
fn a_tileset_edit_or_swap_draws_the_map_again() {
    let project = project();
    let assets = project.path().join("assets");
    write_sidecar(&assets.join("zone.tilemap-0.jdtiles"), &painted());
    let mut app = game(&assets, &map_text("Map", "zone.tilemap-0.jdtiles"));
    let map = map_named(&mut app, "Map");

    let id = {
        let assets = app.world().resource::<Assets<Tileset>>();
        let (id, _) = (assets.iter())
            .find(|(_, tileset)| tileset.image == "overworld.png")
            .expect("the tileset loaded");
        id
    };
    app.world_mut()
        .resource_mut::<Assets<Tileset>>()
        .get_mut(id)
        .expect("the tileset")
        .tile_size = UVec2::splat(8);
    app.update();
    app.update();
    let drawn = chunks_of(&mut app, map);
    assert_eq!(drawn.len(), 3, "drawn again, not added to");
    assert_eq!(drawn[1].2.tile_display_size, UVec2::splat(8));
    assert_eq!(
        drawn[1].4,
        chunk_transform(ChunkCoord(IVec2::ZERO), UVec2::splat(8), 0)
    );
    let before = drawn[1].2.tileset.clone();

    app.world_mut()
        .get_mut::<Tilemap>(map)
        .expect("the map")
        .layers[0]
        .tileset = "other.tileset.bsn".to_string();
    app.update();
    let drawn = chunks_of(&mut app, map);
    assert_eq!(drawn.len(), 3);
    let path = |handle: &Handle<Image>| handle.path().map(ToString::to_string);
    assert_eq!(path(&drawn[1].2.tileset).as_deref(), Some("other.png"));
    assert_ne!(drawn[1].2.tileset, before);
    assert_eq!(path(&drawn[2].2.tileset).as_deref(), Some("overworld.png"));
}
