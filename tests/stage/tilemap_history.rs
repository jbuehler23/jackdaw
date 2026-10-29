//! Tilemap strokes through `tilemap.set_cells`: one history entry per stroke
//! that undo and redo walk exactly, terrain layers holding their resolved
//! tiles, and tileset edits resolving the open maps again.

use crate::tilemap_render::{
    TILESET, chunk_at, chunks, data_tick, grass_patch, grass_strip, ground_terrain, index_at,
    key_of, layer, project_app, settle, spawn_map, the_map, tile_layer,
};

use bevy::prelude::*;
use jackdaw::tilemap::TilemapDataStore;
use jackdaw::tilemap_render::TilemapDrawStats;
use jackdaw::tilemap_resolve::{TilemapResolveStats, full_resolve};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::OperatorWarnings;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::{
    LayerTerrainMatch, PropertyValue, Tilemap, TilemapLayer, TilemapLayerKind,
};
use jackdaw_tilemap::LayerKind;
use jackdaw_tilemap::{CellCoord, TileRef, TilemapData};
use jackdaw_tileset::{TerrainMatch, Tileset};

fn set_cells(
    app: &mut App,
    map: Entity,
    layer: &str,
    kind: &str,
    cells: &str,
    value: &str,
) -> OperatorResult {
    app.world_mut()
        .get_resource_or_init::<OperatorWarnings>()
        .0
        .clear();
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
    settle(app);
    result
}

#[track_caller]
fn paint(app: &mut App, map: Entity, kind: &str, cells: &str, value: &str) {
    let result = set_cells(app, map, "Ground", kind, cells, value);
    assert_eq!(result, OperatorResult::Finished, "{:?}", warnings(app));
}

#[track_caller]
fn refused(app: &mut App, map: Entity, layer: &str, kind: &str, cells: &str, why: &str) {
    let count = entries(app);
    let result = set_cells(app, map, layer, kind, cells, "2");
    assert_eq!(result, OperatorResult::Cancelled);
    let said = warnings(app).join("\n");
    assert!(said.contains(why), "expected {why:?} in {said:?}");
    assert_eq!(entries(app), count);
}

fn warnings(app: &App) -> Vec<String> {
    (app.world().get_resource::<OperatorWarnings>()).map_or(Vec::new(), |w| w.0.clone())
}

fn entries(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn run(app: &mut App, id: &'static str) {
    let result = app.world_mut().operator(id).call().expect("dispatches");
    assert_eq!(result, OperatorResult::Finished, "{id}");
    settle(app);
}

fn stored(app: &mut App, map: Entity) -> TilemapData {
    let key = key_of(app, map);
    (app.world().resource::<TilemapDataStore>().get(&key))
        .cloned()
        .expect("the map has cells")
}

fn bytes(app: &mut App, map: Entity) -> Vec<u8> {
    jackdaw_tilemap::encode(&stored(app, map))
}

fn tileset(app: &App) -> Tileset {
    (app.world().resource::<Assets<Tileset>>().iter())
        .next()
        .map(|(_, t)| t.clone())
        .expect("the tileset")
}

/// The map's terrain tiles as a fresh resolve of its authoring gives them.
fn fresh_tiles(app: &mut App, map: Entity) -> jackdaw_tilemap::ChunkedLayer<TileRef> {
    let set = tileset(app)
        .terrain_set(Some(TerrainMatch::CornersAndSides))
        .expect("a corners and sides set");
    let data = stored(app, map);
    full_resolve(&set, data.layers[0].authoring.as_ref().expect("authoring")).0
}

fn resolved_cells(app: &App) -> usize {
    app.world().resource::<TilemapDataStore>().resolved_cells()
}

fn sets_built(app: &App) -> usize {
    app.world().resource::<TilemapResolveStats>().sets_built
}

#[test]
fn one_stroke_is_one_entry_that_undo_and_redo_restore_byte_for_byte() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let before = bytes(&mut app, map);
    let count = entries(&app);

    paint(&mut app, map, "terrain", "2,2,4,4", "2");
    assert_eq!(entries(&app), count + 1, "one stroke, one entry");
    let after = bytes(&mut app, map);
    assert_ne!(after, before);

    run(&mut app, "history.undo");
    assert_eq!(bytes(&mut app, map), before);
    run(&mut app, "history.redo");
    assert_eq!(bytes(&mut app, map), after);
}

#[test]
fn undo_reaches_the_map_after_a_tab_swap() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let before = bytes(&mut app, map);
    paint(&mut app, map, "terrain", "1,1;2,1;3,1", "2");

    let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
    scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(2));
    let other = scenes.tabs.len() - 1;
    assert!(jackdaw::scenes::swap::swap_active_tab(
        app.world_mut(),
        other
    ));
    settle(&mut app);
    assert!(jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 0));
    settle(&mut app);
    let map = the_map(&mut app);

    run(&mut app, "history.undo");
    assert_eq!(bytes(&mut app, map), before);
    let chunk = chunk_at(&mut app, map, 0, IVec2::ZERO);
    let drawn = index_at(&app, chunk, (2, 1));
    let tiles = &stored(&mut app, map).layers[0].tiles;
    assert_eq!(drawn, Some(tiles.get(CellCoord::new(2, 1)).index()));
}

#[test]
fn a_stroke_over_a_chunk_border_records_both_chunks() {
    let (mut app, _dir) = project_app();
    let ground = layer(TilemapLayerKind::Tile, default());
    let data = tile_layer(&[(0, 0, TileRef::new(3)), (40, 0, TileRef::new(5))]);
    let map = spawn_map(&mut app, vec![ground], data);
    let before = bytes(&mut app, map);
    let (west, east) = (
        chunk_at(&mut app, map, 0, IVec2::ZERO),
        chunk_at(&mut app, map, 0, IVec2::X),
    );

    paint(&mut app, map, "tile", "31,0;32,0", "7");
    assert_eq!(index_at(&app, west, (31, 0)), Some(7));
    assert_eq!(index_at(&app, east, (0, 0)), Some(7));
    let ticks = (data_tick(&app, west), data_tick(&app, east));

    run(&mut app, "history.undo");
    assert_eq!(bytes(&mut app, map), before);
    assert_eq!(index_at(&app, west, (31, 0)), None);
    assert_eq!(index_at(&app, east, (0, 0)), None);
    assert_ne!(
        data_tick(&app, west),
        ticks.0,
        "both chunks are written back"
    );
    assert_ne!(data_tick(&app, east), ticks.1);
}

#[test]
fn set_cells_refuses_what_it_cannot_write() {
    let (mut app, dir) = project_app();
    let tiles = layer(TilemapLayerKind::Tile, default());
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    refused(&mut app, map, "Nope", "terrain", "0,0", "no layer `Nope`");
    refused(
        &mut app,
        map,
        "Ground",
        "paint",
        "0,0",
        "kind is tile or terrain",
    );
    refused(
        &mut app,
        map,
        "Ground",
        "terrain",
        "0,0,9",
        "cells are x,y pairs",
    );

    (app.world_mut().get_mut::<Tilemap>(map).expect("the map")).bounds =
        Some(IRect::new(0, 0, 8, 8));
    settle(&mut app);
    refused(
        &mut app,
        map,
        "Ground",
        "terrain",
        "7,7;8,7",
        "outside the map's bounds",
    );
    paint(&mut app, map, "terrain", "7,7", "2");

    (app.world_mut().get_mut::<Tilemap>(map).expect("the map")).layers[0].terrain_match =
        LayerTerrainMatch::Auto;
    settle(&mut app);
    refused(
        &mut app,
        map,
        "Ground",
        "terrain",
        "0,0",
        "layer `Ground`: ",
    );

    (app.world_mut().get_mut::<Tilemap>(map).expect("the map")).layers = vec![tiles];
    settle(&mut app);
    refused(&mut app, map, "Ground", "terrain", "0,0", "is a tile layer");

    let scene = dir.path().join("assets").join("zone.bsn");
    jackdaw::scene_io::retarget_active_scene(app.world_mut(), &scene.to_string_lossy())
        .expect("retarget");
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    let data_path = (app.world().get::<Tilemap>(map).expect("the map").data_path).clone();
    std::fs::write(dir.path().join("assets").join(data_path), b"not a tilemap").expect("corrupt");
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene);
    settle(&mut app);
    let map = the_map(&mut app);
    refused(
        &mut app,
        map,
        "Ground",
        "tile",
        "0,0",
        "could not be loaded",
    );
}

#[test]
fn a_map_inherited_from_a_prefab_no_tab_holds_is_not_edited() {
    let (mut app, dir) = project_app();
    let assets = dir.path().join("assets");
    std::fs::create_dir_all(assets.join("levels")).expect("levels");
    std::fs::create_dir_all(assets.join("prefabs")).expect("prefabs");
    let prefab = assets.join("prefabs/p.bsn");
    std::fs::write(
        &prefab,
        "#p\n\
         jackdaw::prefab::components::Prefab\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n\
         jackdaw_scene_types::tilemap::Tilemap { data_path: \"p.tilemap-0.jdtiles\" }\n",
    )
    .expect("prefab");
    let scene = assets.join("levels/zone.bsn");
    std::fs::write(
        &scene,
        "#instance\n\
         jackdaw::prefab::components::IsA { source: \"../prefabs/p.bsn\", deleted: [] }\n\
         jackdaw::prefab::components::PrefabEntityId(0)\n",
    )
    .expect("scene");
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene);
    settle(&mut app);
    let map = the_map(&mut app);
    refused(
        &mut app,
        map,
        "Ground",
        "tile",
        "0,0",
        "open the prefab in a tab",
    );

    let mut tab = jackdaw::scenes::SceneTab::new_untitled(9);
    tab.path = Some(prefab);
    (app.world_mut()
        .resource_mut::<jackdaw::scenes::Scenes>()
        .tabs)
        .push(tab);
    refused(&mut app, map, "Ground", "tile", "0,0", "no layer `Ground`");
}

#[test]
fn stored_terrain_tiles_equal_a_fresh_resolve() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    assert_eq!(
        stored(&mut app, map).layers[0].tiles,
        fresh_tiles(&mut app, map)
    );

    paint(&mut app, map, "terrain", "28,2,36,8", "1");
    paint(&mut app, map, "terrain", "30,3,33,5", "2");
    paint(&mut app, map, "terrain", "2,2;3,3", "erase");
    paint(&mut app, map, "tile", "31,4", "7");
    let data = stored(&mut app, map);
    let overrides = &data.layers[0]
        .authoring
        .as_ref()
        .expect("authoring")
        .overrides;
    assert_eq!(
        overrides.get(&CellCoord::new(31, 4)),
        Some(&TileRef::new(7))
    );
    assert_eq!(data.layers[0].tiles, fresh_tiles(&mut app, map));

    paint(&mut app, map, "tile", "31,4", "erase");
    assert_eq!(
        stored(&mut app, map).layers[0].tiles,
        fresh_tiles(&mut app, map)
    );
}

#[test]
fn the_renderer_draws_stored_terrain_tiles_without_resolving() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_strip());
    assert_eq!(chunks(&mut app, map).len(), 3);

    let (resolved, built) = (resolved_cells(&app), sets_built(&app));
    paint(&mut app, map, "terrain", "40,3", "2");
    let count = resolved_cells(&app) - resolved;
    assert!(
        count > 0 && count <= 9,
        "one cell and its ring, not {count}"
    );
    assert_eq!(sets_built(&app), built, "the stroke builds no terrain set");
    let written = app.world().resource::<TilemapDrawStats>().chunks_written;
    assert_eq!(written, 1, "only the edited chunk is drawn again");

    let resolved = resolved_cells(&app);
    (app.world_mut().get_mut::<Tilemap>(map).expect("the map")).set_changed();
    settle(&mut app);
    assert_eq!(sets_built(&app), built, "nor does a whole redraw");
    assert_eq!(
        resolved_cells(&app),
        resolved,
        "a whole redraw resolves nothing"
    );
    assert_eq!(app.world().resource::<TilemapDrawStats>().chunks_written, 3);
    let tiles = stored(&mut app, map).layers[0].tiles.clone();
    for (_, coord, chunk) in chunks(&mut app, map) {
        let data = app
            .world()
            .get::<bevy::sprite_render::TilemapChunkTileData>(chunk)
            .expect("tile data");
        let cells = tiles
            .chunk(jackdaw_tilemap::ChunkCoord(coord))
            .expect("stored");
        let drawn: Vec<Option<u16>> = data.0.iter().map(|t| t.map(|t| t.tileset_index)).collect();
        let want: Vec<Option<u16>> = (cells.iter())
            .map(|t| (!t.is_empty()).then(|| t.index()))
            .collect();
        assert_eq!(drawn, want);
    }
}

#[test]
fn a_tileset_edit_resolves_the_map_again_and_its_undo_restores_both() {
    let (mut app, _dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let centre = stored(&mut app, map).layers[0]
        .tiles
        .get(CellCoord::new(3, 3));
    let (before, tileset_before) = (bytes(&mut app, map), tileset(&app));
    let count = entries(&app);

    let result = app
        .world_mut()
        .operator("tileset.tile.set_bits")
        .param("path", PropertyValue::from(TILESET))
        .param("tile", PropertyValue::from(centre.index().to_string()))
        .param("peers", PropertyValue::from("2,2,2,2,2,2,2,2,2"))
        .call()
        .expect("dispatches");
    assert_eq!(result, OperatorResult::Finished);
    settle(&mut app);
    assert_eq!(
        entries(&app),
        count + 1,
        "the tileset edit is the one entry"
    );
    let tiles = stored(&mut app, map).layers[0].tiles.clone();
    assert_ne!(
        tiles.get(CellCoord::new(3, 3)),
        centre,
        "the centre resolves again"
    );
    assert_eq!(tiles, fresh_tiles(&mut app, map));

    run(&mut app, "history.undo");
    assert_eq!(tileset(&app), tileset_before);
    assert_eq!(bytes(&mut app, map), before);
}

#[test]
fn undo_follows_the_map_to_the_file_a_save_moved_it_to() {
    let (mut app, dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let before = bytes(&mut app, map);
    paint(&mut app, map, "terrain", "1,1", "2");

    let scene = dir.path().join("assets").join("zone.bsn");
    jackdaw::scene_io::retarget_active_scene(app.world_mut(), &scene.to_string_lossy())
        .expect("retarget");
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    settle(&mut app);

    run(&mut app, "history.undo");
    assert_eq!(bytes(&mut app, map), before);
}

/// Saves the map's scene as `zone.bsn` and writes its sidecar the way files
/// were written before terrain tiles were stored: authoring, no tiles.
fn older_file(app: &mut App, dir: &tempfile::TempDir, data: &TilemapData) -> std::path::PathBuf {
    let scene = dir.path().join("assets").join("zone.bsn");
    jackdaw::scene_io::retarget_active_scene(app.world_mut(), &scene.to_string_lossy())
        .expect("retarget");
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    let map = the_map(app);
    let data_path = (app.world().get::<Tilemap>(map).expect("the map").data_path).clone();
    let sidecar = dir.path().join("assets").join(data_path);
    std::fs::write(&sidecar, jackdaw_tilemap::encode(data)).expect("write the sidecar");
    scene
}

fn unsaved(app: &mut App, map: Entity) -> bool {
    let key = key_of(app, map);
    app.world()
        .resource::<TilemapDataStore>()
        .has_unsaved_edits(&key)
}

#[test]
fn derived_tiles_never_make_the_map_unsaved_and_survive_an_unbuildable_set() {
    let (mut app, dir) = project_app();
    spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let scene = older_file(&mut app, &dir, &grass_patch());
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene);
    settle(&mut app);
    let map = the_map(&mut app);
    let tiles = stored(&mut app, map).layers[0].tiles.clone();
    assert!(
        !tiles.is_empty(),
        "the load resolves the older file's tiles"
    );
    assert_eq!(tiles, fresh_tiles(&mut app, map));
    assert!(!unsaved(&mut app, map), "a resolve is no edit");

    (app.world_mut().get_mut::<Tilemap>(map).expect("the map")).layers[0].terrain_match =
        LayerTerrainMatch::Auto;
    settle(&mut app);
    assert_eq!(
        stored(&mut app, map).layers[0].tiles,
        tiles,
        "an unbuildable set keeps the tiles"
    );
    assert!(!unsaved(&mut app, map));
    (app.world_mut().get_mut::<Tilemap>(map).expect("the map")).layers[0].terrain_match =
        LayerTerrainMatch::CornersAndSides;
    settle(&mut app);
    assert_eq!(stored(&mut app, map).layers[0].tiles, tiles);

    std::fs::write(
        dir.path()
            .join("assets")
            .join(&app.world().get::<Tilemap>(map).expect("map").data_path),
        jackdaw_tilemap::encode(&grass_strip()),
    )
    .expect("change the file");
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene);
    settle(&mut app);
    let map = the_map(&mut app);
    let authoring = stored(&mut app, map).layers[0].authoring.clone();
    assert_eq!(
        authoring,
        grass_strip().layers[0].authoring,
        "the reload reads the file"
    );
    assert!(!unsaved(&mut app, map));
}

#[test]
fn undo_after_a_save_as_never_reaches_the_map_that_reopened_the_old_file() {
    let (mut app, dir) = project_app();
    let map = spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    let assets = dir.path().join("assets");
    let scene = assets.join("scene.bsn");
    jackdaw::scene_io::retarget_active_scene(app.world_mut(), &scene.to_string_lossy())
        .expect("retarget");
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    settle(&mut app);
    let before = bytes(&mut app, map);
    paint(&mut app, map, "terrain", "1,1", "2");

    let other = assets.join("other.bsn");
    jackdaw::scene_io::retarget_active_scene(app.world_mut(), &other.to_string_lossy())
        .expect("retarget");
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    settle(&mut app);

    let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
    scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(2));
    let second = scenes.tabs.len() - 1;
    assert!(jackdaw::scenes::swap::swap_active_tab(
        app.world_mut(),
        second
    ));
    settle(&mut app);
    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene);
    settle(&mut app);
    let reopened = the_map(&mut app);
    let old_key = key_of(&mut app, reopened);
    let reopened_bytes = bytes(&mut app, reopened);

    assert!(jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 0));
    settle(&mut app);
    let map = the_map(&mut app);
    assert_ne!(key_of(&mut app, map), old_key);
    run(&mut app, "history.undo");
    assert_eq!(bytes(&mut app, map), before, "the moved map is undone");
    let store = app.world().resource::<TilemapDataStore>();
    let untouched = jackdaw_tilemap::encode(store.get(&old_key).expect("the reopened map"));
    assert_eq!(
        untouched, reopened_bytes,
        "the reopened file's map is left alone"
    );
}

#[test]
fn a_tile_layer_holding_old_authoring_takes_tiles_as_they_are() {
    let (mut app, _dir) = project_app();
    let mut data = grass_patch();
    data.layers[0].kind = LayerKind::Tile;
    let tiles = layer(TilemapLayerKind::Tile, default());
    let map = spawn_map(&mut app, vec![tiles], data);
    paint(&mut app, map, "tile", "3,3", "7");
    let layer = &stored(&mut app, map).layers[0];
    assert_eq!(layer.tiles.get(CellCoord::new(3, 3)), TileRef::new(7));
    let authoring = layer.authoring.as_ref().expect("left as it was");
    assert!(authoring.overrides.is_empty(), "no override is placed");
    assert_eq!(
        authoring.terrain,
        grass_patch()
            .layers
            .remove(0)
            .authoring
            .expect("authoring")
            .terrain
    );
    let chunk = chunk_at(&mut app, map, 0, IVec2::ZERO);
    assert_eq!(index_at(&app, chunk, (3, 3)), Some(7));
}

fn map_at(app: &mut App, key: &str) -> Entity {
    let maps: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<Tilemap>>()
        .iter(app.world())
        .collect();
    (maps.into_iter())
        .find(|&map| key_of(app, map) == key)
        .expect("a map at the key")
}

fn swap_away_and_back(app: &mut App) {
    let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
    scenes.tabs.push(jackdaw::scenes::SceneTab::new_untitled(7));
    let other = scenes.tabs.len() - 1;
    assert!(jackdaw::scenes::swap::swap_active_tab(
        app.world_mut(),
        other
    ));
    settle(app);
    assert!(jackdaw::scenes::swap::swap_active_tab(app.world_mut(), 0));
    settle(app);
}

fn save_as(app: &mut App, dir: &tempfile::TempDir, name: &str) {
    let scene = dir.path().join("assets").join(name);
    jackdaw::scene_io::retarget_active_scene(app.world_mut(), &scene.to_string_lossy())
        .expect("retarget");
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    settle(app);
}

fn sidecar(app: &mut App, dir: &tempfile::TempDir, map: Entity) -> std::path::PathBuf {
    let data_path = (app.world().get::<Tilemap>(map).expect("the map").data_path).clone();
    dir.path().join("assets").join(data_path)
}

enum Gone {
    Closed,
    Quarantined,
    Reloaded,
}

/// Two saved maps, a stroke on each, then something happens to the second
/// map's cells. The first map's stroke stays undoable.
fn earlier_history_survives(what: Gone) {
    let (mut app, dir) = project_app();
    spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    save_as(&mut app, &dir, "zone.bsn");
    let mut maps: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<Tilemap>>()
        .iter(app.world())
        .collect();
    maps.sort();
    let (first, second) = (maps[0], maps[1]);
    let (first_key, second_key) = (key_of(&mut app, first), key_of(&mut app, second));
    let first_before = bytes(&mut app, first);
    paint(&mut app, first, "terrain", "1,1", "2");
    paint(&mut app, second, "terrain", "2,2", "2");
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    settle(&mut app);

    let file = sidecar(&mut app, &dir, second);
    match what {
        Gone::Closed => {
            (app.world_mut().resource_mut::<TilemapDataStore>()).remove(&second_key);
        }
        Gone::Quarantined => {
            std::fs::write(&file, b"not a tilemap").expect("corrupt");
            swap_away_and_back(&mut app);
        }
        Gone::Reloaded => {
            std::fs::write(&file, jackdaw_tilemap::encode(&grass_strip())).expect("rewrite");
            swap_away_and_back(&mut app);
        }
    }
    app.world_mut()
        .get_resource_or_init::<OperatorWarnings>()
        .0
        .clear();
    run(&mut app, "history.undo");
    if matches!(what, Gone::Reloaded) {
        assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
        run(&mut app, "history.undo");
    } else {
        let said = warnings(&app).join("\n");
        assert!(said.contains("no longer open were dropped"), "{said:?}");
    }
    let first = map_at(&mut app, &first_key);
    assert_eq!(
        bytes(&mut app, first),
        first_before,
        "the earlier stroke is undone"
    );
    assert_eq!(entries(&app), 0);
}

#[test]
fn closing_a_map_drops_its_strokes_and_leaves_earlier_history_undoable() {
    earlier_history_survives(Gone::Closed);
}

#[test]
fn a_quarantined_map_drops_its_strokes_and_leaves_earlier_history_undoable() {
    earlier_history_survives(Gone::Quarantined);
}

#[test]
fn a_reloaded_map_keeps_its_strokes_and_earlier_history() {
    earlier_history_survives(Gone::Reloaded);
}

#[test]
fn strokes_still_apply_after_a_reload_that_follows_a_tileset_re_derive() {
    let (mut app, dir) = project_app();
    spawn_map(&mut app, vec![ground_terrain()], grass_patch());
    save_as(&mut app, &dir, "zone.bsn");
    let map = the_map(&mut app);
    let before = bytes(&mut app, map);
    let centre = stored(&mut app, map).layers[0]
        .tiles
        .get(CellCoord::new(3, 3));
    paint(&mut app, map, "terrain", "1,1", "2");
    let result = app
        .world_mut()
        .operator("tileset.tile.set_bits")
        .param("path", PropertyValue::from(TILESET))
        .param("tile", PropertyValue::from(centre.index().to_string()))
        .param("peers", PropertyValue::from("2,2,2,2,2,2,2,2,2"))
        .call()
        .expect("dispatches");
    assert_eq!(result, OperatorResult::Finished);
    settle(&mut app);
    assert!(jackdaw::scene_io::save_scene(app.world_mut()));
    settle(&mut app);
    let file = sidecar(&mut app, &dir, map);
    let saved = std::fs::read(&file).expect("saved");
    std::fs::write(&file, saved).expect("touch");
    swap_away_and_back(&mut app);

    run(&mut app, "history.undo");
    app.world_mut()
        .get_resource_or_init::<OperatorWarnings>()
        .0
        .clear();
    run(&mut app, "history.undo");
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    let map = the_map(&mut app);
    assert_eq!(bytes(&mut app, map), before, "the stroke still applies");
}

#[test]
fn a_layer_switched_from_tile_to_terrain_paints_and_undoes() {
    let (mut app, _dir) = project_app();
    let tiles = layer(TilemapLayerKind::Tile, default());
    let map = spawn_map(
        &mut app,
        vec![tiles],
        tile_layer(&[(0, 0, TileRef::new(3))]),
    );
    (app.world_mut().get_mut::<Tilemap>(map).expect("the map")).layers[0] = ground_terrain();
    settle(&mut app);
    assert_eq!(
        stored(&mut app, map).layers[0].kind,
        LayerKind::Terrain,
        "the scene's kind wins"
    );
    let before = bytes(&mut app, map);

    paint(&mut app, map, "terrain", "4,4,6,6", "1");
    let painted = stored(&mut app, map);
    assert_eq!(painted.layers[0].kind, LayerKind::Terrain);
    assert!(!painted.layers[0].tiles.get(CellCoord::new(5, 5)).is_empty());
    assert_eq!(
        painted.layers[0].tiles.get(CellCoord::new(0, 0)),
        TileRef::new(3)
    );
    run(&mut app, "history.undo");
    assert_eq!(bytes(&mut app, map), before);
    run(&mut app, "history.redo");
    assert_eq!(stored(&mut app, map), painted);
}

#[test]
fn a_call_that_changes_nothing_leaves_the_map_saved_and_unpadded() {
    let (mut app, dir) = project_app();
    let tiles = layer(TilemapLayerKind::Tile, default());
    spawn_map(&mut app, vec![tiles], TilemapData::default());
    save_as(&mut app, &dir, "zone.bsn");
    let map = the_map(&mut app);
    let count = entries(&app);
    assert!(!unsaved(&mut app, map));
    let result = set_cells(&mut app, map, "Ground", "tile", "0,0", "erase");
    assert_eq!(result, OperatorResult::Finished);
    assert_eq!(entries(&app), count);
    assert!(!unsaved(&mut app, map), "nothing changed");
    assert!(
        stored(&mut app, map).layers.is_empty(),
        "no layer was added"
    );
}

#[test]
fn an_idle_map_with_tile_and_ambiguous_layers_stops_refreshing() {
    let (mut app, _dir) = project_app();
    let tiles = layer(TilemapLayerKind::Tile, default());
    let ambiguous = TilemapLayer {
        name: "Water".to_string(),
        ..layer(TilemapLayerKind::Terrain, LayerTerrainMatch::Auto)
    };
    let mut data = grass_patch();
    data.layers
        .insert(0, tile_layer(&[(0, 0, TileRef::new(3))]).layers.remove(0));
    spawn_map(&mut app, vec![tiles, ambiguous], data);
    settle(&mut app);
    let refreshes = |app: &App| app.world().resource::<TilemapResolveStats>().refreshes;
    let before = refreshes(&app);
    for _ in 0..3 {
        settle(&mut app);
    }
    assert_eq!(refreshes(&app), before, "an idle scene runs no refresh");
}
