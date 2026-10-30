//! Draws each tilemap's layers as Bevy tilemap chunks: children of the map,
//! derived from its cells in the store. An edit draws again only the chunks
//! it touched; a change to the map or its tilesets draws the whole map.

use std::collections::{BTreeSet, HashMap, HashSet};

use bevy::prelude::*;
use bevy::sprite_render::{TileData, TileOrientation, TilemapChunk, TilemapChunkTileData};
use jackdaw_scene_types::{Tilemap, TilemapLayerKind};
use jackdaw_tilemap::ChunkCoord;
use jackdaw_tileset::Tileset;
use jackdaw_tileset::chunk::ChunkDraw;
pub use jackdaw_tileset::chunk::TilemapChunkOf;

use crate::tilemap::{Redraw, TilemapDataStore, store_keys};
use crate::tilemap_resolve::cached_set;

pub fn plugin(app: &mut App) {
    // Derived from the map's cells, so never saved and not listed in the
    // outliner.
    app.register_required_components::<TilemapChunkOf, crate::EditorHidden>()
        .register_required_components::<TilemapChunkOf, crate::NonSerializable>();
    app.init_resource::<TilemapDrawWarnings>()
        .init_resource::<TilemapDrawStats>()
        .add_observer(clear_removed_map)
        .add_systems(
            Update,
            queue_tilemap_draws
                .after(crate::tilemap::ensure_tilemap_data_path)
                .after(crate::tilemap_resolve::queue_terrain_refresh)
                .run_if(in_state(crate::AppState::Editor)),
        );
}

/// What a map's chunks were last drawn from.
#[derive(Component, Default)]
struct TilemapDrawn {
    key: Option<String>,
    tilesets: Vec<AssetId<Tileset>>,
    /// Some layer's tileset could not be found.
    missing: bool,
    chunks: HashMap<(usize, ChunkCoord), Entity>,
}

/// Warnings shown, by map (its store key, or its entity without one), layer
/// and text, so a map drawn again does not repeat them. A layer that draws
/// again forgets its own.
#[derive(Resource, Default)]
pub struct TilemapDrawWarnings(HashSet<(String, usize, String)>);

impl TilemapDrawWarnings {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What the last batch of tilemap draws did.
#[derive(Resource, Default, Debug)]
pub struct TilemapDrawStats {
    /// Chunks whose tile data was written.
    pub chunks_written: usize,
}

fn queue_tilemap_draws(
    changed: Query<Entity, Changed<Tilemap>>,
    drawn: Query<(Entity, &TilemapDrawn)>,
    store: Res<TilemapDataStore>,
    mut events: MessageReader<AssetEvent<Tileset>>,
    mut commands: Commands,
) {
    let (mut touched, mut gone) = (HashSet::new(), HashSet::new());
    for event in events.read() {
        match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id } => touched.insert(*id),
            AssetEvent::Removed { id } | AssetEvent::Unused { id } => gone.insert(*id),
        };
    }
    let mut whole: Vec<Entity> = changed.iter().collect();
    let mut edited = Vec::new();
    for (entity, drawn) in &drawn {
        if whole.contains(&entity) {
            continue;
        }
        let tileset_moved = (!touched.is_empty() && drawn.missing)
            || (drawn.tilesets.iter()).any(|id| touched.contains(id) || gone.contains(id));
        if tileset_moved {
            whole.push(entity);
        } else if store.is_changed()
            && (drawn.key.as_deref()).is_some_and(|key| store.needs_redraw(key))
        {
            edited.push(entity);
        }
    }
    if !whole.is_empty() || !edited.is_empty() {
        commands.queue(move |world: &mut World| draw_batch(world, &whole, &edited, &gone));
    }
}

fn clear_removed_map(
    remove: On<Remove<Tilemap>>,
    drawn: Query<&TilemapDrawn>,
    mut commands: Commands,
) {
    let Ok(drawn) = drawn.get(remove.entity) else {
        return;
    };
    for &chunk in drawn.chunks.values() {
        commands.entity(chunk).try_despawn();
    }
    commands.entity(remove.entity).try_remove::<TilemapDrawn>();
}

/// A layer's tileset as one batch of draws uses it.
struct Prepared {
    tileset: Tileset,
    draw: ChunkDraw,
}

/// The tilesets of one batch, each loaded once.
#[derive(Default)]
struct Tilesets(HashMap<String, Result<(AssetId<Tileset>, Option<Prepared>), String>>);

impl Tilesets {
    fn get(
        &mut self,
        world: &mut World,
        path: &str,
    ) -> &mut Result<(AssetId<Tileset>, Option<Prepared>), String> {
        self.0.entry(path.to_string()).or_insert_with(|| {
            let (id, tileset) = crate::tileset_dock::load(world, path)?;
            let prepared = tileset.map(|tileset| {
                let image = world.resource::<AssetServer>().load(&tileset.image);
                let draw = ChunkDraw::new(&tileset, image);
                Prepared { tileset, draw }
            });
            Ok((id, prepared))
        })
    }
}

fn draw_batch(
    world: &mut World,
    whole: &[Entity],
    edited: &[Entity],
    gone: &HashSet<AssetId<Tileset>>,
) -> Result {
    world.insert_resource(TilemapDrawStats::default());
    let keys = store_keys(world);
    let mut redraws: HashMap<String, Option<Redraw>> = HashMap::new();
    for map in whole.iter().chain(edited) {
        if let Some(key) = keys.get(map) {
            let mut store = world.resource_mut::<TilemapDataStore>();
            redraws
                .entry(key.clone())
                .or_insert_with(|| store.take_redraw(key));
        }
    }
    let mut tilesets = Tilesets::default();
    for &map in whole {
        draw_map(world, map, keys.get(&map), None, &mut tilesets, gone)?;
    }
    for &map in edited {
        let key = keys.get(&map);
        let chunks = match key.and_then(|key| redraws.get(key)) {
            Some(Some(Redraw::Chunks(chunks))) => Some(chunks),
            Some(Some(Redraw::Whole)) => None,
            Some(None) | None => continue,
        };
        draw_map(world, map, key, chunks, &mut tilesets, gone)?;
    }
    Ok(())
}

type Wanted = (TilemapChunk, TilemapChunkTileData, Transform);

/// Brings `map`'s chunk children in line with its cells, all of them or only
/// the `edited` chunks: spawns the chunks that gained cells, despawns the
/// ones left empty, re-uploads changed tile data and re-inserts a chunk whose
/// tileset image or tile size changed.
fn draw_map(
    world: &mut World,
    map: Entity,
    key: Option<&String>,
    edited: Option<&BTreeSet<(usize, ChunkCoord)>>,
    tilesets: &mut Tilesets,
    gone: &HashSet<AssetId<Tileset>>,
) -> Result {
    let Some(tilemap) = world.get::<Tilemap>(map).cloned() else {
        return Ok(());
    };
    if world.get::<TilemapDrawn>(map).is_none() {
        world.entity_mut(map).insert(TilemapDrawn::default());
    }
    let Some(mut drawn) = world.get_mut::<TilemapDrawn>(map) else {
        return Ok(());
    };
    let mut chunks = std::mem::take(&mut drawn.chunks);
    let owner = key.cloned().unwrap_or_else(|| format!("{map}"));
    let mut warnings = Vec::new();
    let mut drew = Vec::new();
    let (mut ids, mut missing) = (Vec::new(), false);
    let mut wanted: HashMap<(usize, ChunkCoord), Wanted> = HashMap::new();
    let mut touched: HashSet<(usize, ChunkCoord)> = HashSet::new();

    for (index, layer) in tilemap.layers.iter().enumerate() {
        let prepared = match tilesets.get(world, &layer.tileset) {
            Ok((id, prepared)) => {
                ids.push(*id);
                match prepared {
                    Some(prepared) => prepared,
                    None => {
                        if gone.contains(id) {
                            warnings.push((
                                index,
                                format!("its tileset {} was unloaded", layer.tileset),
                            ));
                        }
                        continue;
                    }
                }
            }
            Err(why) => {
                missing = true;
                warnings.push((index, why.clone()));
                continue;
            }
        };
        if prepared.tileset.image.is_empty() {
            warnings.push((index, "its tileset has no image".to_string()));
            continue;
        }
        if layer.kind == TilemapLayerKind::Terrain
            && let Some(&id) = ids.last()
            && let Some(Err(why)) = cached_set(world, id, layer.terrain_match)
        {
            warnings.push((index, why));
            continue;
        }
        let store = world.resource::<TilemapDataStore>();
        let Some(cells) = (key.and_then(|key| store.get(key))).and_then(|d| d.layers.get(index))
        else {
            drew.push(index);
            continue;
        };
        let scope: Option<Vec<ChunkCoord>> = edited.map(|edited| {
            (edited.iter())
                .filter(|(at, _)| *at == index)
                .map(|(_, chunk)| *chunk)
                .collect()
        });
        let tiles = &cells.tiles;
        let coords = scope.unwrap_or_else(|| tiles.chunks().map(|(c, _)| c).collect());
        drew.push(index);
        let tile_size = prepared.tileset.display_size(tilemap.cell_size);
        for coord in coords {
            touched.insert((index, coord));
            let Some(cells) = tiles.chunk(coord) else {
                continue;
            };
            let bundle = (prepared.draw).bundle(coord, cells, tile_size, index);
            wanted.insert((index, coord), bundle);
        }
    }

    world.resource_mut::<TilemapDrawStats>().chunks_written += wanted.len();
    chunks.retain(|slot, entity| {
        if edited.is_some() && !touched.contains(slot) {
            return true;
        }
        let alive = world.get_entity(*entity).is_ok();
        match wanted.remove(slot) {
            Some(next) if alive => {
                update_chunk(world, *entity, next);
                true
            }
            Some(next) => {
                wanted.insert(*slot, next);
                false
            }
            None => {
                if alive {
                    world.entity_mut(*entity).despawn();
                }
                false
            }
        }
    });
    for ((layer, chunk), (tilemap_chunk, data, transform)) in wanted {
        let of = TilemapChunkOf { layer, chunk };
        let bundle = (of, tilemap_chunk, data, transform);
        chunks.insert((layer, chunk), world.spawn((bundle, ChildOf(map))).id());
    }
    if let Some(mut drawn) = world.get_mut::<TilemapDrawn>(map) {
        drawn.chunks = chunks;
        drawn.key = key.cloned();
        if edited.is_none() {
            drawn.tilesets = ids;
            drawn.missing = missing;
        }
    }

    let mut shown = world.resource_mut::<TilemapDrawWarnings>();
    if edited.is_none() {
        shown
            .0
            .retain(|(at, layer, _)| *at != owner || !drew.contains(layer));
    }
    for (index, why) in warnings {
        let name = &tilemap.layers[index].name;
        if shown.0.insert((owner.clone(), index, why.clone())) {
            warn!("Tilemap layer {name:?} draws nothing: {why}");
        }
    }
    Ok(())
}

fn update_chunk(world: &mut World, entity: Entity, (chunk, data, transform): Wanted) {
    let mut chunk_entity = world.entity_mut(entity);
    let same_chunk = (chunk_entity.get::<TilemapChunk>()).is_some_and(|old| {
        old.tileset == chunk.tileset && old.tile_display_size == chunk.tile_display_size
    });
    let same_data = (chunk_entity.get::<TilemapChunkTileData>())
        .is_some_and(|old| old.iter().map(tile_key).eq(data.iter().map(tile_key)));
    if !same_chunk {
        chunk_entity.insert((chunk, data));
    } else if !same_data {
        chunk_entity.insert(data);
    }
    if chunk_entity.get::<Transform>() != Some(&transform) {
        chunk_entity.insert(transform);
    }
}

fn tile_key(tile: &Option<TileData>) -> Option<(u16, bool, TileOrientation, Color)> {
    tile.map(|t| (t.tileset_index, t.visible, t.orientation, t.color))
}
