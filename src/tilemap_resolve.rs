//! Keeps each terrain layer's stored tiles resolved from its terrain, links
//! and overrides. An edit resolves the cells it touched as it is dropped (see
//! [`TilemapEdit`](crate::tilemap::TilemapEdit)); a layer whose terrain set
//! changed (a tileset edit or its undo, a new match mode, a load) is resolved
//! whole here, as derived data that never makes the map unsaved.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bevy::prelude::*;
use jackdaw_scene_types::{LayerTerrainMatch, Tilemap, TilemapLayer};
use jackdaw_tilemap::{
    ChunkedLayer, LayerKind, TerrainAuthoring, TerrainSet, TileRef, resolve_region,
};
use jackdaw_tileset::Tileset;

use crate::tilemap::{TilemapDataStore, layer_mode, store_keys};

pub fn plugin(app: &mut App) {
    app.init_resource::<TilemapResolveStats>()
        .init_resource::<TerrainSetCache>()
        .add_systems(
            Update,
            queue_terrain_refresh
                .after(crate::tilemap::ensure_tilemap_data_path)
                .run_if(in_state(crate::AppState::Editor)),
        );
}

#[derive(Resource, Default, Debug)]
pub struct TilemapResolveStats {
    /// Terrain sets built from a tileset since the editor started.
    pub sets_built: usize,
    /// Runs of the refresh that checks every live map's terrain layers.
    pub refreshes: usize,
}

/// The terrain set of each tileset and match mode, or why it cannot be
/// built; built again only once the tileset changes.
#[derive(Resource, Default)]
pub struct TerrainSetCache(
    HashMap<(AssetId<Tileset>, LayerTerrainMatch), Result<Arc<TerrainSet>, String>>,
);

pub(crate) fn queue_terrain_refresh(
    changed: Query<(), Changed<Tilemap>>,
    store: Res<TilemapDataStore>,
    mut cache: ResMut<TerrainSetCache>,
    mut events: MessageReader<AssetEvent<Tileset>>,
    mut commands: Commands,
) {
    let mut tilesets = false;
    for event in events.read() {
        let (AssetEvent::Added { id }
        | AssetEvent::Modified { id }
        | AssetEvent::LoadedWithDependencies { id }
        | AssetEvent::Removed { id }
        | AssetEvent::Unused { id }) = event;
        cache.0.retain(|(at, _), _| at != id);
        tilesets = true;
    }
    if tilesets || store.is_changed() || !changed.is_empty() {
        commands.queue(refresh_terrain);
    }
}

/// The terrain set a terrain layer of tileset `id` resolves with, from the
/// cache, or `None` while the tileset loads.
pub(crate) fn cached_set(
    world: &mut World,
    id: AssetId<Tileset>,
    terrain_match: LayerTerrainMatch,
) -> Option<Result<Arc<TerrainSet>, String>> {
    if let Some(hit) = (world.resource::<TerrainSetCache>().0).get(&(id, terrain_match)) {
        return Some(hit.clone());
    }
    let tileset = world.resource::<Assets<Tileset>>().get(id)?;
    let set = (tileset.terrain_set(layer_mode(terrain_match)))
        .map(Arc::new)
        .map_err(|err| err.to_string());
    world.resource_mut::<TilemapResolveStats>().sets_built += 1;
    (world.resource_mut::<TerrainSetCache>().0).insert((id, terrain_match), set.clone());
    Some(set)
}

/// The terrain set `layer` resolves with, or `None` while its tileset is
/// missing or loading.
pub(crate) fn layer_set(
    world: &mut World,
    layer: &TilemapLayer,
) -> Option<Result<Arc<TerrainSet>, String>> {
    let id = crate::tileset_dock::tileset_id(world, &layer.tileset).ok()?;
    cached_set(world, id, layer.terrain_match)
}

/// Gives every terrain layer of the live maps the set it resolves with, and
/// resolves whole each layer whose tiles were resolved with another set or
/// none. A layer whose set cannot be built keeps its tiles until it can; a
/// tile layer resolves nothing.
pub(crate) fn refresh_terrain(world: &mut World) {
    world.resource_mut::<TilemapResolveStats>().refreshes += 1;
    let mut keys: Vec<(Entity, String)> = store_keys(world).into_iter().collect();
    keys.sort();
    let mut seen = HashSet::new();
    for (entity, key) in keys {
        if !seen.insert(key.clone()) {
            continue;
        }
        let Some(tilemap) = world.get::<Tilemap>(entity).cloned() else {
            continue;
        };
        for (index, layer) in tilemap.layers.iter().enumerate() {
            refresh_layer(world, &key, index, layer);
        }
    }
}

/// Brings one data layer in line with its scene layer: its kind, and for a
/// terrain layer the set it resolves with and the tiles that set gives. The
/// store is borrowed mutably only for a change.
fn refresh_layer(world: &mut World, key: &str, index: usize, layer: &TilemapLayer) {
    let kind = crate::tilemap::data_kind(layer.kind);
    let terrain = kind == LayerKind::Terrain;
    if world
        .resource::<TilemapDataStore>()
        .kind_differs(key, index, kind)
    {
        (world.resource_mut::<TilemapDataStore>()).sync_layer_kind(key, index, kind);
    }
    let set = if terrain {
        layer_set(world, layer)
    } else {
        Some(Err(String::new()))
    };
    let Some(set) = set else {
        return;
    };
    let store = world.resource::<TilemapDataStore>();
    let current = store.resolved_with(key, index);
    let Ok(set) = set else {
        if current.is_some() {
            (world.resource_mut::<TilemapDataStore>()).forget_resolved(key, index);
        }
        return;
    };
    if current.is_some_and(|at| Arc::ptr_eq(at, &set)) {
        return;
    }
    let same = current.is_some_and(|at| **at == *set);
    let authoring = (store.get(key))
        .and_then(|data| data.layers.get(index))
        .and_then(|cells| cells.authoring.as_ref());
    let fresh = authoring
        .filter(|_| !same)
        .map(|authoring| full_resolve(&set, authoring));
    let mut store = world.resource_mut::<TilemapDataStore>();
    if let Some((fresh, count)) = fresh {
        store.resolved_cells += count;
        store.put_derived_tiles(key, index, &fresh);
    }
    store.note_resolved(key, index, set);
}

/// A terrain layer's tiles resolved from nothing: every painted and
/// overridden cell and the ring around them. Returns the tiles and how many
/// cells were resolved.
pub fn full_resolve(
    set: &TerrainSet,
    authoring: &TerrainAuthoring,
) -> (ChunkedLayer<TileRef>, usize) {
    let painted = (authoring.terrain.cells().map(|(cell, _)| cell))
        .chain(authoring.overrides.keys().copied());
    let (terrain, links, overrides) = (&authoring.terrain, &authoring.links, &authoring.overrides);
    let resolved = resolve_region(set, terrain, links, overrides, painted);
    let mut tiles = ChunkedLayer::new();
    for &(cell, tile) in &resolved {
        tiles.set(cell, tile);
    }
    tiles.take_dirty();
    (tiles, resolved.len())
}
