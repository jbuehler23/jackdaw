//! Tilemaps in a built game.
//!
//! A `Tilemap` names a `.jdtiles` sidecar relative to the document it was
//! authored in: the scene, or the prefab an inherited map comes from. The
//! sidecar holds each layer's resolved tiles, so the game draws them as they
//! are, with the chunk builder the editor uses, and never resolves terrain.
//!
//! The sidecar is read from disk when the map first draws, as terrain
//! sidecars are. A missing sidecar draws an empty map, with a warning. A map
//! draws again, whole, when its `Tilemap` changes or one of its tilesets does.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::sprite_render::{TilemapChunkMaterial, TilemapChunkMeshCache};
use jackdaw_scene_types::Tilemap;
use jackdaw_tilemap::TilemapData;
use jackdaw_tileset::chunk::{ChunkDraw, TilemapChunkOf};
use jackdaw_tileset::{Tileset, TilesetPlugin};

use crate::{JackdawCatalog, RuntimeOnly};

pub(crate) fn plugin(app: &mut App) {
    if !app.is_plugin_added::<TilesetPlugin>() {
        app.add_plugins(TilesetPlugin);
    }
    app.register_required_components::<TilemapChunkOf, RuntimeOnly>()
        .add_systems(Update, queue_draws.after(crate::spawn_loaded_scenes));
}

/// On a map whose sidecar was not found; it draws empty.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct TilemapSidecarMissing;

/// Where a spawned map reads its sidecar, and what it last drew from.
#[derive(Component)]
struct TilemapSource {
    /// Folder of the document that names the map.
    dir: PathBuf,
    /// The data path `data` was read from.
    read: Option<String>,
    data: TilemapData,
    tilesets: Vec<AssetId<Tileset>>,
    /// Some layer's tileset was not loaded.
    missing: bool,
}

/// Give every freshly spawned map the folder its sidecar resolves against.
pub(crate) fn attach_sidecars(world: &mut World, dirs: &[(Entity, PathBuf)]) {
    for (entity, dir) in dirs {
        if (world.get::<Tilemap>(*entity)).is_some_and(|map| !map.data_path.is_empty()) {
            world.entity_mut(*entity).insert(TilemapSource {
                dir: dir.clone(),
                read: None,
                data: TilemapData::default(),
                tilesets: Vec::new(),
                missing: false,
            });
        }
    }
}

fn queue_draws(
    changed: Query<Entity, (Changed<Tilemap>, With<TilemapSource>)>,
    sources: Query<(Entity, &TilemapSource)>,
    mut events: MessageReader<AssetEvent<Tileset>>,
    mut commands: Commands,
) {
    let touched: HashSet<AssetId<Tileset>> = (events.read())
        .filter_map(|event| match event {
            AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id }
            | AssetEvent::Removed { id } => Some(*id),
            _ => None,
        })
        .collect();
    let mut maps: Vec<Entity> = changed.iter().collect();
    if !touched.is_empty() {
        for (entity, source) in &sources {
            let moved = source.missing || source.tilesets.iter().any(|id| touched.contains(id));
            if moved && !maps.contains(&entity) {
                maps.push(entity);
            }
        }
    }
    if !maps.is_empty() {
        commands.queue(move |world: &mut World| draw_batch(world, &maps));
    }
}

/// A layer's tileset as one batch of draws uses it.
struct Prepared {
    tileset: Tileset,
    draw: ChunkDraw,
}

/// The tilesets of one batch by path, each looked up, cloned and given its
/// image once.
type Tilesets = HashMap<String, (Option<AssetId<Tileset>>, Result<Prepared, String>)>;

fn draw_batch(world: &mut World, maps: &[Entity]) {
    if !world.contains_resource::<TilemapChunkMeshCache>()
        || !world.contains_resource::<Assets<TilemapChunkMaterial>>()
    {
        warn!("this app has no tilemap chunk renderer; tilemaps draw nothing");
        return;
    }
    let mut tilesets = Tilesets::new();
    for &map in maps {
        draw_map(world, map, &mut tilesets);
    }
}

fn prepare(world: &World, path: &str) -> (Option<AssetId<Tileset>>, Result<Prepared, String>) {
    let id = (world.resource::<JackdawCatalog>().get(path))
        .and_then(|handle| handle.id().try_typed::<Tileset>().ok());
    let Some(tileset) = id.and_then(|id| world.resource::<Assets<Tileset>>().get(id)) else {
        return (id, Err(format!("{path} is not a loaded tileset")));
    };
    if tileset.image.is_empty() {
        return (id, Err("its tileset has no image".to_string()));
    }
    let image = world.resource::<AssetServer>().load(&tileset.image);
    let draw = ChunkDraw::new(tileset, image);
    let tileset = tileset.clone();
    (id, Ok(Prepared { tileset, draw }))
}

/// Draw `map` again from its sidecar, reading the file first when the data
/// path changed.
fn draw_map(world: &mut World, map: Entity, tilesets: &mut Tilesets) {
    let Some(tilemap) = world.get::<Tilemap>(map).cloned() else {
        return;
    };
    let name = (world.get::<Name>(map)).map_or_else(|| map.to_string(), ToString::to_string);
    let Some(mut source) = world.entity_mut(map).take::<TilemapSource>() else {
        return;
    };
    if source.read.as_deref() != Some(tilemap.data_path.as_str()) {
        source.data = read_sidecar(world, map, &source.dir, &tilemap.data_path, &name);
        source.read = Some(tilemap.data_path.clone());
    }
    let old: Vec<Entity> = (world.get::<Children>(map).into_iter().flatten())
        .copied()
        .filter(|&child| world.get::<TilemapChunkOf>(child).is_some())
        .collect();
    for chunk in old {
        world.entity_mut(chunk).despawn();
    }
    let data = &source.data;
    if !data.layers.is_empty() && data.layers.len() != tilemap.layers.len() {
        warn!(
            "Tilemap {name}: its sidecar holds {} layers and the map names {}; drawing the \
             layers that line up",
            data.layers.len(),
            tilemap.layers.len()
        );
    }
    let (mut ids, mut missing) = (Vec::new(), false);
    for (index, (layer, cells)) in tilemap.layers.iter().zip(&data.layers).enumerate() {
        let (id, prepared) = tilesets
            .entry(layer.tileset.clone())
            .or_insert_with(|| prepare(world, &layer.tileset));
        let assets = world.resource::<Assets<Tileset>>();
        missing |= id.is_none_or(|id| !assets.contains(id));
        ids.extend(*id);
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(why) => {
                if !cells.tiles.is_empty() {
                    warn!("Tilemap layer {:?} draws nothing: {why}", layer.name);
                }
                continue;
            }
        };
        let tile_size = prepared.tileset.display_size(tilemap.cell_size);
        for (coord, chunk_cells) in cells.tiles.chunks() {
            let bundle = prepared.draw.bundle(coord, chunk_cells, tile_size, index);
            let of = TilemapChunkOf {
                layer: index,
                chunk: coord,
            };
            world.spawn((of, bundle, ChildOf(map)));
        }
    }
    source.tilesets = ids;
    source.missing = missing;
    world.entity_mut(map).insert(source);
}

fn read_sidecar(
    world: &mut World,
    map: Entity,
    dir: &Path,
    data_path: &str,
    name: &str,
) -> TilemapData {
    let path = match jackdaw_tilemap::resolve_path(dir, data_path) {
        Ok(path) => path,
        Err(err) => {
            warn!("Tilemap {name} draws nothing: its data path {data_path:?} is unusable: {err}");
            return TilemapData::default();
        }
    };
    world.entity_mut(map).remove::<TilemapSidecarMissing>();
    match std::fs::read(&path) {
        Ok(bytes) => jackdaw_tilemap::decode(&bytes).unwrap_or_else(|err| {
            warn!(
                "Tilemap {name} draws nothing: its sidecar {} is unreadable ({err})",
                path.display()
            );
            TilemapData::default()
        }),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            warn!(
                "Tilemap sidecar {} is missing; loading an empty map",
                path.display()
            );
            world.entity_mut(map).insert(TilemapSidecarMissing);
            TilemapData::default()
        }
        Err(err) => {
            warn!(
                "Tilemap {name} draws nothing: its sidecar {} cannot be read ({err})",
                path.display()
            );
            TilemapData::default()
        }
    }
}
