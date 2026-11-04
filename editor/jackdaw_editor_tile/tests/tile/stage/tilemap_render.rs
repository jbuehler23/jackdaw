//! A tilemap draws its cells as Bevy tilemap chunks under the map: one per
//! layer and filled 32 x 32 block, drawn again from the store when the cells,
//! the map or its tileset change, and never part of the saved scene.

use crate::util;

use std::collections::BTreeSet;

use bevy::prelude::*;
use bevy::sprite_render::TilemapChunkTileData;
use bevy::sprite_render::{MeshMaterial2d, TileOrientation, TilemapChunk, TilemapChunkMaterial};
use jackdaw_api::prelude::*;
use jackdaw_editor_tile::tilemap::TilemapDataStore;
use jackdaw_editor_tile::tilemap_render::TilemapChunkOf;
use jackdaw_scene_types::{
    EditorHidden, LayerTerrainMatch, PropertyValue, Tilemap, TilemapLayer, TilemapLayerKind,
};
use jackdaw_tilemap::{
    CellCoord, ChunkedLayer, LayerData, LayerKind, Placement, TerrainAuthoring, TerrainId, TileRef,
    TilemapData,
};
use jackdaw_tileset::chunk::TilemapEdgePiece;
use jackdaw_tileset::{TerrainMatch, Tileset};

const TILEMAP_TYPE_PATH: &str = "jackdaw_scene_types::tilemap::Tilemap";
pub(crate) const TILESET: &str = "overworld.tileset.bsn";

/// An editor on a temp project holding the overworld sheet imported as
/// `overworld.tileset.bsn` with auto setup.
pub(crate) fn project_app() -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let assets = dir.path().join("assets");
    std::fs::create_dir_all(&assets).expect("assets");
    std::fs::copy(util::OVERWORLD_SHEET, assets.join("overworld.png")).expect("copy the sheet");
    let mut app = util::editor_test_app_reading(&assets);
    app.world_mut()
        .insert_resource(jackdaw_editor_core::project::ProjectRoot {
            root: dir.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw_editor_core::AppState>>()
        .set(jackdaw_editor_core::AppState::Editor);
    settle(&mut app);
    let result = app
        .world_mut()
        .operator("tileset.import")
        .param("paths", PropertyValue::from("overworld.png".to_string()))
        .param("setup", PropertyValue::from("auto".to_string()))
        .call()
        .expect("tileset.import dispatches");
    assert_eq!(result, OperatorResult::Finished);
    settle(&mut app);
    let mut scenes = app
        .world_mut()
        .resource_mut::<jackdaw_editor_core::scenes::Scenes>();
    if scenes.tabs.is_empty() {
        scenes
            .tabs
            .push(jackdaw_editor_core::scenes::SceneTab::new_untitled(1));
        scenes.active = 0;
    }
    (app, dir)
}

pub(crate) fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

pub(crate) fn layer(kind: TilemapLayerKind, terrain_match: LayerTerrainMatch) -> TilemapLayer {
    TilemapLayer {
        name: "Ground".to_string(),
        tileset: TILESET.to_string(),
        kind,
        terrain_match,
    }
}

/// Spawn a map linked into the live document and fill its cells with `data`.
pub(crate) fn spawn_map(app: &mut App, layers: Vec<TilemapLayer>, data: TilemapData) -> Entity {
    let tilemap = Tilemap {
        data_path: "zone.tilemap-0.jdtiles".to_string(),
        layers,
        ..default()
    };
    let entity = app
        .world_mut()
        .spawn((tilemap.clone(), Name::new("map")))
        .id();
    let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Name("map".to_string())]);
    ast.add_to_roots(node);
    ast.link(entity, node);
    jackdaw_editor_core::commands::sync_component_to_ast(
        app.world_mut(),
        entity,
        TILEMAP_TYPE_PATH,
        &tilemap,
    );
    settle(app);
    let key = key_of(app, entity);
    app.world_mut()
        .resource_mut::<TilemapDataStore>()
        .insert(key, data);
    settle(app);
    entity
}

pub(crate) fn key_of(app: &mut App, map: Entity) -> String {
    jackdaw_editor_tile::tilemap::store_key_of(app.world_mut(), map).expect("a valid data path")
}

fn edit(app: &mut App, map: Entity, change: impl FnOnce(&mut TilemapData)) {
    let key = key_of(app, map);
    let mut store = app.world_mut().resource_mut::<TilemapDataStore>();
    change(&mut store.get_mut(&key).expect("the map has cells"));
    settle(app);
}

pub(crate) fn tile_layer(cells: &[(i32, i32, TileRef)]) -> TilemapData {
    let mut tiles = ChunkedLayer::new();
    for &(x, y, tile) in cells {
        tiles.set(CellCoord::new(x, y), tile);
    }
    TilemapData {
        layers: vec![LayerData {
            kind: LayerKind::Tile,
            placement: Default::default(),
            tiles,
            authoring: None,
        }],
    }
}

pub(crate) fn the_map(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<Tilemap>>()
        .single(app.world())
        .expect("one map")
}

/// Each drawn chunk of `map` by its layer and chunk coordinate.
pub(crate) fn chunks(app: &mut App, map: Entity) -> Vec<(usize, IVec2, Entity)> {
    let mut found: Vec<_> = app
        .world_mut()
        .query::<(Entity, &TilemapChunkOf, &ChildOf)>()
        .iter(app.world())
        .filter(|(_, _, parent)| parent.parent() == map)
        .map(|(entity, of, _)| (of.layer, of.chunk.0, entity))
        .collect();
    found.sort_by_key(|(layer, chunk, _)| (*layer, chunk.x, chunk.y));
    found
}

pub(crate) fn chunk_at(app: &mut App, map: Entity, layer: usize, chunk: IVec2) -> Entity {
    (chunks(app, map).into_iter())
        .find(|(l, c, _)| *l == layer && *c == chunk)
        .map(|(.., entity)| entity)
        .unwrap_or_else(|| panic!("no chunk {chunk} on layer {layer}"))
}

pub(crate) fn index_at(app: &App, chunk: Entity, local: (i32, i32)) -> Option<u16> {
    let data = app
        .world()
        .get::<TilemapChunkTileData>(chunk)
        .expect("tile data");
    let index = CellCoord::new(local.0, local.1).local_index();
    data.0[index].map(|tile| tile.tileset_index)
}

pub(crate) fn data_tick(app: &App, chunk: Entity) -> bevy::ecs::change_detection::Tick {
    app.world()
        .entity(chunk)
        .get_ref::<TilemapChunkTileData>()
        .expect("tile data")
        .last_changed()
}

#[test]
fn filled_blocks_draw_as_chunks_under_the_map() {
    let (mut app, _dir) = project_app();
    let flipped = TileRef::new(4).with_flip_x(true).with_transpose(true);
    let data = tile_layer(&[
        (0, 0, TileRef::new(3)),
        (31, 31, flipped),
        (32, 0, TileRef::new(5)),
        (-1, -1, TileRef::new(6)),
    ]);
    let map = spawn_map(
        &mut app,
        vec![layer(TilemapLayerKind::Tile, default())],
        data,
    );

    let drawn: Vec<(usize, IVec2)> = (chunks(&mut app, map).into_iter())
        .map(|(layer, chunk, _)| (layer, chunk))
        .collect();
    assert_eq!(
        drawn,
        [
            (0, IVec2::new(-1, -1)),
            (0, IVec2::ZERO),
            (0, IVec2::new(1, 0))
        ]
    );

    let origin = chunk_at(&mut app, map, 0, IVec2::ZERO);
    let world = app.world();
    let data = world
        .get::<TilemapChunkTileData>(origin)
        .expect("tile data");
    assert_eq!(data.0.len(), 1024);
    assert_eq!(data.0.iter().flatten().count(), 2);
    let corner = data.0[CellCoord::new(31, 31).local_index()].expect("filled");
    assert_eq!(corner.tileset_index, 4);
    assert_eq!(
        corner.orientation,
        TileOrientation::from_bools(true, false, true)
    );
    let chunk = world.get::<TilemapChunk>(origin).expect("a chunk");
    assert_eq!(chunk.chunk_size, UVec2::splat(32));
    assert_eq!(chunk.tile_display_size, UVec2::splat(16));
    assert_eq!(
        chunk.tileset.path().map(ToString::to_string).as_deref(),
        Some("overworld.tiles.png")
    );
    assert_eq!(
        world.get::<Transform>(origin).expect("placed").translation,
        Vec3::new(256.0, 256.0, 0.0),
        "the chunk mesh is centred, so cell (0,0) starts at the map origin"
    );
    assert_eq!(world.get::<ChildOf>(origin).map(ChildOf::parent), Some(map));
    assert!(world.get::<EditorHidden>(origin).is_some());
    assert!(
        world
            .get::<MeshMaterial2d<TilemapChunkMaterial>>(origin)
            .is_some(),
        "the chunk's insert hook found its tile data"
    );
    assert!(
        app.world().get::<Visibility>(map).is_some(),
        "a map spawned without a transform gets one"
    );
    assert_eq!(
        (app.world().get::<GlobalTransform>(origin)).map(GlobalTransform::translation),
        Some(Vec3::new(256.0, 256.0, 0.0)),
        "it draws at the origin"
    );
    assert_eq!(index_at(&app, origin, (0, 0)), Some(3));
    let west = chunk_at(&mut app, map, 0, IVec2::new(-1, -1));
    assert_eq!(index_at(&app, west, (31, 31)), Some(6));
    assert_eq!(
        app.world()
            .get::<Transform>(west)
            .expect("placed")
            .translation,
        Vec3::new(-256.0, -256.0, 0.0)
    );
}

#[test]
fn an_edit_updates_only_its_chunk_and_an_emptied_chunk_goes() {
    let (mut app, _dir) = project_app();
    let data = tile_layer(&[(0, 0, TileRef::new(3)), (40, 0, TileRef::new(5))]);
    let map = spawn_map(
        &mut app,
        vec![layer(TilemapLayerKind::Tile, default())],
        data,
    );
    let origin = chunk_at(&mut app, map, 0, IVec2::ZERO);
    let east = chunk_at(&mut app, map, 0, IVec2::new(1, 0));
    let (origin_tick, east_tick) = (data_tick(&app, origin), data_tick(&app, east));

    edit(&mut app, map, |data| {
        data.layers[0]
            .tiles
            .set(CellCoord::new(1, 0), TileRef::new(9));
    });
    assert_eq!(chunk_at(&mut app, map, 0, IVec2::ZERO), origin, "kept");
    assert_eq!(index_at(&app, origin, (1, 0)), Some(9));
    assert_ne!(data_tick(&app, origin), origin_tick);
    assert_eq!(
        data_tick(&app, east),
        east_tick,
        "the other chunk is untouched"
    );

    edit(&mut app, map, |data| {
        data.layers[0].tiles.clear(CellCoord::new(40, 0));
    });
    assert!(
        app.world().get_entity(east).is_err(),
        "the emptied chunk despawns"
    );
    assert_eq!(chunks(&mut app, map).len(), 1);
}

#[test]
fn a_new_tileset_image_re_inserts_the_chunks() {
    let (mut app, dir) = project_app();
    let assets = dir.path().join("assets");
    std::fs::copy(
        assets.join("overworld.tiles.png"),
        assets.join("other.tiles.png"),
    )
    .expect("copy the packed image");
    std::fs::copy(
        assets.join("overworld.tiles.png.meta"),
        assets.join("other.tiles.png.meta"),
    )
    .expect("copy its meta");
    let data = tile_layer(&[(0, 0, TileRef::new(3))]);
    let map = spawn_map(
        &mut app,
        vec![layer(TilemapLayerKind::Tile, default())],
        data,
    );
    let chunk = chunk_at(&mut app, map, 0, IVec2::ZERO);
    let material = |app: &App| {
        app.world()
            .get::<MeshMaterial2d<TilemapChunkMaterial>>(chunk)
            .expect("a material")
            .id()
    };
    let before = material(&app);

    let id = app
        .world()
        .resource::<Assets<Tileset>>()
        .iter()
        .next()
        .map(|(id, _)| id)
        .expect("the tileset is loaded");
    let mut tilesets = app.world_mut().resource_mut::<Assets<Tileset>>();
    tilesets.get_mut(id).expect("the tileset").image = "other.tiles.png".to_string();
    settle(&mut app);

    let tileset = &app
        .world()
        .get::<TilemapChunk>(chunk)
        .expect("a chunk")
        .tileset;
    assert_eq!(
        tileset.path().map(ToString::to_string).as_deref(),
        Some("other.tiles.png")
    );
    assert_ne!(material(&app), before, "the chunk was inserted again");
    assert_eq!(index_at(&app, chunk, (0, 0)), Some(3));
}

/// A 6 x 6 patch of grass: terrain 1, which pairs with 2 in a corners and
/// sides set of the overworld tileset.
pub(crate) fn grass_patch() -> TilemapData {
    let mut terrain = ChunkedLayer::new();
    for y in 0..6 {
        for x in 0..6 {
            terrain.set(CellCoord::new(x, y), 1);
        }
    }
    TilemapData {
        layers: vec![LayerData {
            kind: LayerKind::Terrain,
            placement: Default::default(),
            tiles: ChunkedLayer::new(),
            authoring: Some(TerrainAuthoring {
                terrain,
                ..default()
            }),
        }],
    }
}

#[test]
fn a_terrain_layer_draws_tiles_its_terrain_set_resolves() {
    let (mut app, _dir) = project_app();
    let ground = layer(
        TilemapLayerKind::Terrain,
        LayerTerrainMatch::CornersAndSides,
    );
    let map = spawn_map(&mut app, vec![ground], grass_patch());
    let chunk = chunk_at(&mut app, map, 0, IVec2::ZERO);
    let centre = index_at(&app, chunk, (3, 3)).expect("the centre resolves to a tile");

    let tileset = (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(_, t)| t.clone())
        .expect("the tileset");
    let pure_grass: BTreeSet<u32> = (tileset.tiles.iter())
        .filter(|tile| tile.peers.iter().all(|&p| p == 1))
        .filter_map(|tile| tileset.layer(tile.sheet, tile.tile))
        .collect();
    assert!(
        pure_grass.contains(&u32::from(centre)),
        "tile {centre} is not one of the pure grass tiles {pure_grass:?}"
    );
}

#[test]
fn a_terrain_layer_with_no_single_mode_draws_nothing() {
    let (mut app, _dir) = project_app();
    let ground = layer(TilemapLayerKind::Terrain, LayerTerrainMatch::Auto);
    let map = spawn_map(&mut app, vec![ground], grass_patch());
    assert!(chunks(&mut app, map).is_empty());
}

#[test]
fn chunks_are_drawn_again_after_a_tab_swap_and_play_stop() {
    let (mut app, _dir) = project_app();
    let data = tile_layer(&[(0, 0, TileRef::new(3)), (-5, 2, TileRef::new(7))]);
    let map = spawn_map(
        &mut app,
        vec![layer(TilemapLayerKind::Tile, default())],
        data,
    );
    let drawn = |app: &mut App, map: Entity| -> Vec<(usize, IVec2, Option<u16>)> {
        (chunks(app, map).into_iter())
            .map(|(layer, chunk, entity)| (layer, chunk, index_at(app, entity, (0, 0))))
            .collect()
    };
    let before = drawn(&mut app, map);
    assert_eq!(before.len(), 2);

    let mut scenes = app
        .world_mut()
        .resource_mut::<jackdaw_editor_core::scenes::Scenes>();
    scenes
        .tabs
        .push(jackdaw_editor_core::scenes::SceneTab::new_untitled(2));
    let other = scenes.tabs.len() - 1;
    assert!(jackdaw_editor_core::scenes::swap::swap_active_tab(
        app.world_mut(),
        other
    ));
    settle(&mut app);
    assert!(
        (app.world_mut().query::<&TilemapChunkOf>().iter(app.world())).count() == 0,
        "the other tab draws no chunks"
    );
    assert!(jackdaw_editor_core::scenes::swap::swap_active_tab(
        app.world_mut(),
        0
    ));
    settle(&mut app);
    let swapped = the_map(&mut app);
    assert_ne!(swapped, map, "the map respawns from the document");
    assert_eq!(drawn(&mut app, swapped), before);

    jackdaw_editor_core::pie_projection::revert_preview(app.world_mut());
    settle(&mut app);
    let stopped = the_map(&mut app);
    assert_ne!(stopped, swapped);
    assert_eq!(drawn(&mut app, stopped), before);
    let total = (app.world_mut().query::<&TilemapChunkOf>().iter(app.world())).count();
    assert_eq!(total, 2, "no chunk outlives its map");
}

#[test]
fn the_saved_scene_holds_no_chunk() {
    let (mut app, dir) = project_app();
    let data = tile_layer(&[(0, 0, TileRef::new(3)), (40, 40, TileRef::new(5))]);
    let map = spawn_map(
        &mut app,
        vec![layer(TilemapLayerKind::Tile, default())],
        data,
    );
    assert_eq!(chunks(&mut app, map).len(), 2);

    let scene = dir.path().join("assets").join("zone.bsn");
    jackdaw_editor_core::scene_io::retarget_active_scene(app.world_mut(), &scene.to_string_lossy())
        .expect("retarget");
    assert!(jackdaw_editor_core::scene_io::save_scene(app.world_mut()));
    let text = std::fs::read_to_string(&scene).expect("the scene is written");
    assert!(text.contains("Tilemap"), "{text}");
    for derived in ["TilemapChunk", "Mesh2d", "tilemap_chunk", "TileData"] {
        assert!(!text.contains(derived), "{derived} leaked into:\n{text}");
    }
    for required in ["Transform", "Visibility"] {
        assert!(!text.contains(required), "{required} was written:\n{text}");
    }
    assert!(text.len() < 1024, "the scene text stays small:\n{text}");
}

/// Grass over cells 0..70 x 0..20: three chunks in a row.
pub(crate) fn grass_strip() -> TilemapData {
    let mut data = grass_patch();
    let terrain = &mut data.layers[0]
        .authoring
        .as_mut()
        .expect("authoring")
        .terrain;
    for y in 0..20 {
        for x in 0..70 {
            terrain.set(CellCoord::new(x, y), 1);
        }
    }
    data
}

pub(crate) fn ground_terrain() -> TilemapLayer {
    layer(
        TilemapLayerKind::Terrain,
        LayerTerrainMatch::CornersAndSides,
    )
}

#[test]
fn an_unloaded_tileset_takes_its_layer_away() {
    let (mut app, _dir) = project_app();
    let data = tile_layer(&[(0, 0, TileRef::new(3))]);
    let map = spawn_map(
        &mut app,
        vec![layer(TilemapLayerKind::Tile, default())],
        data,
    );
    assert_eq!(chunks(&mut app, map).len(), 1);
    let id = (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(id, _)| id)
        .expect("the tileset is loaded");
    app.world_mut().resource_mut::<Assets<Tileset>>().remove(id);
    settle(&mut app);
    assert!(chunks(&mut app, map).is_empty());
    let warnings = app
        .world()
        .resource::<jackdaw_editor_tile::tilemap_render::TilemapDrawWarnings>();
    assert!(!warnings.is_empty());
}

#[test]
fn a_layer_that_draws_again_forgets_its_warning() {
    let (mut app, _dir) = project_app();
    let ground = layer(TilemapLayerKind::Terrain, LayerTerrainMatch::Auto);
    let map = spawn_map(&mut app, vec![ground], grass_patch());
    let warned = |app: &App| {
        !app.world()
            .resource::<jackdaw_editor_tile::tilemap_render::TilemapDrawWarnings>()
            .is_empty()
    };
    assert!(warned(&app));
    app.world_mut()
        .get_mut::<Tilemap>(map)
        .expect("the map")
        .layers[0]
        .terrain_match = LayerTerrainMatch::CornersAndSides;
    settle(&mut app);
    assert!(!warned(&app));
    assert_eq!(chunks(&mut app, map).len(), 1);
}

#[test]
fn a_direct_terrain_edit_resolves_a_bounded_region_and_writes_one_chunk() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_strip());
    let slots: Vec<Entity> = (chunks(&mut app, map).into_iter())
        .map(|(.., entity)| entity)
        .collect();
    assert_eq!(slots.len(), 3);
    let ticks: Vec<_> = slots.iter().map(|&c| data_tick(&app, c)).collect();
    let resolved = |app: &App| app.world().resource::<TilemapDataStore>().resolved_cells();
    let start = resolved(&app);

    edit(&mut app, map, |data| {
        let authoring = data.layers[0].authoring.as_mut().expect("authoring");
        authoring.terrain.set(CellCoord::new(40, 3), 2);
    });
    let ring = 4 * 32 + 4;
    let count = resolved(&app) - start;
    assert!(
        count > 0 && count <= 1024 + ring,
        "resolved {count} cells for one edited chunk"
    );
    let now: Vec<_> = slots.iter().map(|&c| data_tick(&app, c)).collect();
    assert_eq!(now[0], ticks[0]);
    assert_ne!(now[1], ticks[1], "the edited chunk is written");
    assert_eq!(now[2], ticks[2]);

    let key = key_of(&mut app, map);
    let set = (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(_, t)| t.terrain_set(Some(jackdaw_tileset::TerrainMatch::CornersAndSides)))
        .expect("the tileset")
        .expect("a set");
    let store = app.world().resource::<TilemapDataStore>();
    let data = store.get(&key).expect("the map");
    let authoring = data.layers[0].authoring.as_ref().expect("authoring");
    let fresh = jackdaw_editor_tile::tilemap_resolve::full_resolve(&set, authoring, None).0;
    assert_eq!(data.layers[0].tiles, fresh, "the edit left no stale tile");
}

/// The inner and outer terrain of the overworld's first corners set.
pub(crate) fn corner_pair(app: &App) -> (TerrainId, TerrainId) {
    let tileset = (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(_, t)| t.clone())
        .expect("the tileset");
    (tileset.terrain_sets.iter())
        .find(|set| set.mode == TerrainMatch::Corners)
        .map(|set| (set.inner, set.outer))
        .expect("a corners set")
}

/// A 6 x 6 patch of `outer` with one `inner` cell at (2, 2), and `overrides`.
pub(crate) fn corner_patch(
    (inner, outer): (TerrainId, TerrainId),
    overrides: &[(i32, i32, TileRef)],
) -> TilemapData {
    let mut data = grass_patch();
    let authoring = data.layers[0].authoring.as_mut().expect("authoring");
    for (cell, _) in authoring.terrain.clone().cells() {
        authoring.terrain.set(cell, outer);
    }
    authoring.terrain.set(CellCoord::new(2, 2), inner);
    for &(x, y, tile) in overrides {
        authoring.overrides.insert(CellCoord::new(x, y), tile);
    }
    data
}

fn corner_set(app: &App) -> jackdaw_tilemap::TerrainSet {
    (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(_, t)| t.terrain_set(Some(TerrainMatch::Corners)))
        .expect("the tileset")
        .expect("a corners set")
}

#[test]
fn a_corner_layer_draws_half_a_cell_up_and_right_of_an_on_cells_layer() {
    let (mut app, _dir) = project_app();
    let pair = corner_pair(&app);
    let mut data = corner_patch(pair, &[]);
    data.layers
        .push(tile_layer(&[(0, 0, TileRef::new(3))]).layers.remove(0));
    let layers = vec![
        layer(TilemapLayerKind::Terrain, LayerTerrainMatch::Corners),
        layer(TilemapLayerKind::Tile, default()),
    ];
    let map = spawn_map(&mut app, layers, data);
    let at = |app: &mut App, layer: usize| {
        let chunk = chunk_at(app, map, layer, IVec2::ZERO);
        let transform = app.world().get::<Transform>(chunk).expect("a transform");
        transform.translation.truncate()
    };
    assert_eq!(at(&mut app, 1), Vec2::splat(256.0));
    assert_eq!(at(&mut app, 0), Vec2::splat(264.0));
    let key = key_of(&mut app, map);
    let store = app.world().resource::<TilemapDataStore>();
    let data = store.get(&key).expect("the map");
    assert_eq!(data.layers[0].placement, Placement::BetweenCells);
    assert_eq!(data.layers[1].placement, Placement::OnCells);
    let lowest = data.layers[0]
        .tiles
        .cells()
        .map(|(c, _)| c.0)
        .reduce(IVec2::min);
    assert_eq!(
        lowest,
        Some(IVec2::splat(-1)),
        "tiles hang half a cell past the cells"
    );
}

fn edge_pieces(app: &mut App) -> usize {
    (app.world_mut().query::<(&TilemapEdgePiece, &ChildOf)>())
        .iter(app.world())
        .count()
}

#[test]
fn a_bounded_corner_layer_crops_the_tiles_its_bounds_cut() {
    let (mut app, _dir) = project_app();
    let pair = corner_pair(&app);
    let layers = vec![layer(TilemapLayerKind::Terrain, LayerTerrainMatch::Corners)];
    let map = spawn_map(&mut app, layers, corner_patch(pair, &[]));
    assert_eq!(edge_pieces(&mut app), 0, "an unbounded map crops nothing");
    app.world_mut()
        .get_mut::<Tilemap>(map)
        .expect("a map")
        .bounds = Some(IRect::new(0, 0, 6, 6));
    settle(&mut app);
    assert_eq!(edge_pieces(&mut app), 7 * 7 - 5 * 5);
    let key = key_of(&mut app, map);
    let set = corner_set(&app);
    let store = app.world().resource::<TilemapDataStore>();
    let data = store.get(&key).expect("the map");
    let authoring = data.layers[0].authoring.as_ref().expect("authoring");
    let bounds = Some(jackdaw_tilemap::CellRect {
        min: IVec2::ZERO,
        max: IVec2::splat(6),
    });
    let fresh = jackdaw_editor_tile::tilemap_resolve::full_resolve(&set, authoring, bounds).0;
    assert_eq!(
        data.layers[0].tiles, fresh,
        "the bounds resolved the edge again"
    );
}

#[test]
fn an_old_corner_layer_resolves_between_cells_and_hides_its_hand_placed_tiles() {
    let (mut app, _dir) = project_app();
    let patch = corner_patch(corner_pair(&app), &[(1, 1, TileRef::new(5))]);
    let mut old = patch.clone();
    old.layers[0]
        .tiles
        .set(CellCoord::new(2, 2), TileRef::new(9));
    let old = jackdaw_tilemap::decode(&jackdaw_tilemap::encode(&old)).expect("an old sidecar");
    assert_eq!(old.layers[0].placement, Placement::OnCells);
    let layers = vec![layer(TilemapLayerKind::Terrain, LayerTerrainMatch::Corners)];
    let map = spawn_map(&mut app, layers, old);
    let key = key_of(&mut app, map);
    let set = corner_set(&app);
    let store = app.world().resource::<TilemapDataStore>();
    let data = store.get(&key).expect("the map");
    let authoring = data.layers[0].authoring.as_ref().expect("authoring");
    let kept = authoring.overrides.get(&CellCoord::new(1, 1));
    assert_eq!(kept, Some(&TileRef::new(5)), "the hand-placed tile is kept");
    let painted = patch.layers[0].authoring.as_ref().expect("authoring");
    assert_eq!(authoring.terrain, painted.terrain, "the painted cells stay");
    assert_eq!(data.layers[0].placement, Placement::BetweenCells);
    let fresh = jackdaw_editor_tile::tilemap_resolve::full_resolve(&set, authoring, None).0;
    assert_eq!(data.layers[0].tiles, fresh, "but not drawn");
}

#[test]
fn a_new_corner_layer_paints_its_first_cells_between_cells() {
    let (mut app, _dir) = project_app();
    let (inner, _) = corner_pair(&app);
    let data = TilemapData {
        layers: vec![LayerData {
            kind: LayerKind::Terrain,
            placement: Placement::OnCells,
            tiles: ChunkedLayer::new(),
            authoring: None,
        }],
    };
    let layers = vec![layer(TilemapLayerKind::Terrain, LayerTerrainMatch::Corners)];
    let map = spawn_map(&mut app, layers, data);
    let result = app
        .world_mut()
        .operator("tilemap.set_cells")
        .param("entity", PropertyValue::Entity(map))
        .param("layer", PropertyValue::from("0".to_string()))
        .param("cells", PropertyValue::from("2,2".to_string()))
        .param("kind", PropertyValue::from("terrain".to_string()))
        .param("value", PropertyValue::from(inner.to_string()))
        .call()
        .expect("tilemap.set_cells dispatches");
    assert_eq!(result, OperatorResult::Finished);
    settle(&mut app);
    let key = key_of(&mut app, map);
    let store = app.world().resource::<TilemapDataStore>();
    let data = store.get(&key).expect("the map");
    assert_eq!(data.layers[0].placement, Placement::BetweenCells);
    assert_eq!(data.layers[0].painted_cells(), vec![CellCoord::new(2, 2)]);
    let chunk = chunk_at(&mut app, map, 0, IVec2::ZERO);
    let at = app.world().get::<Transform>(chunk).expect("a transform");
    assert_eq!(at.translation.truncate(), Vec2::splat(264.0));
}

fn set_match(app: &mut App, map: Entity, terrain_match: LayerTerrainMatch) {
    (app.world_mut().get_mut::<Tilemap>(map).expect("a map")).layers[0].terrain_match =
        terrain_match;
    settle(app);
}

fn stored_layer(app: &mut App, map: Entity) -> LayerData {
    let key = key_of(app, map);
    let store = app.world().resource::<TilemapDataStore>();
    store.get(&key).expect("the map").layers[0].clone()
}

#[test]
fn hand_placed_tiles_survive_a_switch_between_cells_and_back() {
    let (mut app, _dir) = project_app();
    let mut data = grass_patch();
    let placed = TileRef::new(5);
    let at = CellCoord::new(1, 1);
    (data.layers[0].authoring.as_mut().expect("authoring"))
        .overrides
        .insert(at, placed);
    let map = spawn_map(&mut app, vec![ground_terrain()], data);
    assert_eq!(stored_layer(&mut app, map).tiles.get(at), placed);
    set_match(&mut app, map, LayerTerrainMatch::Corners);
    let corners = stored_layer(&mut app, map);
    assert_eq!(corners.placement, Placement::BetweenCells);
    let authoring = corners.authoring.as_ref().expect("authoring");
    assert_eq!(
        authoring.overrides.get(&at),
        Some(&placed),
        "kept, not drawn"
    );
    let fresh =
        jackdaw_editor_tile::tilemap_resolve::full_resolve(&corner_set(&app), authoring, None).0;
    assert_eq!(corners.tiles, fresh);
    set_match(&mut app, map, LayerTerrainMatch::CornersAndSides);
    let back = stored_layer(&mut app, map);
    assert_eq!(back.placement, Placement::OnCells);
    assert_eq!(back.tiles.get(at), placed, "shown again on cells");
}

#[test]
fn a_corner_layer_made_a_tile_layer_sits_on_cells() {
    let (mut app, _dir) = project_app();
    let layers = vec![layer(TilemapLayerKind::Terrain, LayerTerrainMatch::Corners)];
    let patch = corner_patch(corner_pair(&app), &[]);
    let map = spawn_map(&mut app, layers, patch);
    assert_eq!(
        stored_layer(&mut app, map).placement,
        Placement::BetweenCells
    );
    (app.world_mut().get_mut::<Tilemap>(map).expect("a map")).layers[0].kind =
        TilemapLayerKind::Tile;
    settle(&mut app);
    let layer = stored_layer(&mut app, map);
    assert_eq!(layer.kind, LayerKind::Tile);
    assert_eq!(layer.placement, Placement::OnCells);
}

fn bounded(min: i32, max: i32) -> Option<jackdaw_tilemap::CellRect> {
    Some(jackdaw_tilemap::CellRect {
        min: IVec2::splat(min),
        max: IVec2::splat(max),
    })
}

#[test]
fn growing_the_bounds_of_a_corner_layer_saves_the_ring_it_resolves() {
    let (mut app, dir) = project_app();
    let layers = vec![layer(TilemapLayerKind::Terrain, LayerTerrainMatch::Corners)];
    let patch = corner_patch(corner_pair(&app), &[]);
    let map = spawn_map(&mut app, layers, patch);
    let scene = dir.path().join("assets").join("zone.bsn");
    jackdaw_editor_core::scene_io::retarget_active_scene(app.world_mut(), &scene.to_string_lossy())
        .expect("retarget");
    assert!(jackdaw_editor_core::scene_io::save_scene(app.world_mut()));
    settle(&mut app);
    let key = key_of(&mut app, map);
    assert!(
        !app.world()
            .resource::<TilemapDataStore>()
            .has_unsaved_edits(&key)
    );

    app.world_mut()
        .get_mut::<Tilemap>(map)
        .expect("a map")
        .bounds = Some(IRect::new(0, 0, 6, 6));
    settle(&mut app);
    assert!(
        app.world()
            .resource::<TilemapDataStore>()
            .has_unsaved_edits(&key),
        "the new ring is saved"
    );
    assert!(jackdaw_editor_core::scene_io::save_scene(app.world_mut()));
    settle(&mut app);
    let data_path = app
        .world()
        .get::<Tilemap>(map)
        .expect("a map")
        .data_path
        .clone();
    let file = std::fs::read(dir.path().join("assets").join(data_path)).expect("the sidecar");
    let saved = jackdaw_tilemap::decode(&file).expect("a sidecar");
    let authoring = saved.layers[0].authoring.as_ref().expect("authoring");
    let fresh = jackdaw_editor_tile::tilemap_resolve::full_resolve(
        &corner_set(&app),
        authoring,
        bounded(0, 6),
    );
    assert_eq!(saved.layers[0].tiles, fresh.0);
    assert_eq!(saved.layers[0].placement, Placement::BetweenCells);
}

#[test]
fn bounds_moved_while_the_set_is_away_resolve_once_it_is_back() {
    let (mut app, _dir) = project_app();
    let layers = vec![layer(TilemapLayerKind::Terrain, LayerTerrainMatch::Corners)];
    let patch = corner_patch(corner_pair(&app), &[]);
    let map = spawn_map(&mut app, layers, patch);
    let tileset = |app: &mut App, path: &str| {
        (app.world_mut().get_mut::<Tilemap>(map).expect("a map")).layers[0].tileset =
            path.to_string();
        settle(app);
    };
    tileset(&mut app, "missing.tileset.bsn");
    app.world_mut()
        .get_mut::<Tilemap>(map)
        .expect("a map")
        .bounds = Some(IRect::new(0, 0, 6, 6));
    settle(&mut app);
    tileset(&mut app, TILESET);
    let key = key_of(&mut app, map);
    let store = app.world().resource::<TilemapDataStore>();
    assert_eq!(store.resolved_bounds(&key, 0), bounded(0, 6));
    let layer = stored_layer(&mut app, map);
    let authoring = layer.authoring.as_ref().expect("authoring");
    let fresh = jackdaw_editor_tile::tilemap_resolve::full_resolve(
        &corner_set(&app),
        authoring,
        bounded(0, 6),
    );
    assert_eq!(layer.tiles, fresh.0, "the ring follows the new bounds");
}
