//! Draws each tilemap's layers as Bevy tilemap chunks: children of the map,
//! derived from its cells in the store. An edit draws again only the chunks
//! it touched; a change to the map or its tilesets draws the whole map.

use std::collections::{BTreeSet, HashMap, HashSet};

use bevy::prelude::*;
use bevy::sprite_render::{
    AlphaMode2d, TileData, TileOrientation, TilemapChunk, TilemapChunkTileData,
};
use jackdaw_scene_types::{Tilemap, TilemapLayerKind};
use jackdaw_tilemap::{CHUNK_SIZE, ChunkCoord, TileRef};
use jackdaw_tileset::Tileset;

use crate::tilemap::{Redraw, TilemapDataStore, store_keys};
use crate::tilemap_resolve::cached_set;

pub fn plugin(app: &mut App) {
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

/// One layer's 32 x 32 block of the tilemap it is a child of, as drawn.
/// Derived from the map's cells, so it is never saved and not listed in the
/// outliner.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
#[require(crate::EditorHidden, crate::NonSerializable)]
pub struct TilemapChunkOf {
    /// Index into [`Tilemap::layers`].
    pub layer: usize,
    pub chunk: ChunkCoord,
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

/// The tile a cell's [`TileRef`] draws, or `None` for an empty cell.
///
/// Both orient the tile image the same way: transpose across its top-left
/// diagonal, then mirror, so the bits map one to one.
pub fn tile_data(tile: TileRef) -> Option<TileData> {
    (!tile.is_empty()).then(|| TileData {
        tileset_index: tile.index(),
        visible: tile.visible(),
        orientation: TileOrientation::from_bools(tile.flip_x(), tile.flip_y(), tile.transpose()),
        ..default()
    })
}

/// Where the chunk at `coord` of layer `layer` sits under its map: the chunk
/// mesh is centred on its entity, and cell (0,0) has its min corner at the
/// map's origin. Layers stack one whole z unit apart, so y-sorted entities,
/// which move less than half a unit, never cross from one layer to the next.
pub fn chunk_transform(coord: ChunkCoord, tile_size: UVec2, layer: usize) -> Transform {
    let centre = (coord.0 * CHUNK_SIZE).as_vec2() + CHUNK_SIZE as f32 / 2.0;
    Transform::from_translation((centre * tile_size.as_vec2()).extend(layer as f32))
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
    image: Handle<Image>,
    /// The packed layers the tileset has a tile in.
    known: HashSet<u16>,
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
                let known = (tileset.sheets.iter())
                    .flat_map(|sheet| {
                        sheet.first_layer..sheet.first_layer + sheet.columns * sheet.rows
                    })
                    .filter_map(|layer| u16::try_from(layer).ok())
                    .collect();
                let image = world.resource::<AssetServer>().load(&tileset.image);
                Prepared {
                    tileset,
                    image,
                    known,
                }
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

type Wanted = (TilemapChunk, Vec<Option<TileData>>, Transform);

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
        let tile_size = match prepared.tileset.tile_size.min_element() {
            0 => tilemap.cell_size,
            _ => prepared.tileset.tile_size,
        };
        let chunk = TilemapChunk {
            chunk_size: UVec2::splat(CHUNK_SIZE as u32),
            tile_display_size: tile_size,
            tileset: prepared.image.clone(),
            alpha_mode: AlphaMode2d::Blend,
        };
        for coord in coords {
            touched.insert((index, coord));
            let Some(cells) = tiles.chunk(coord) else {
                continue;
            };
            let data = (cells.iter())
                .map(|&tile| tile_data(tile).filter(|_| prepared.known.contains(&tile.index())))
                .collect();
            let transform = chunk_transform(coord, tile_size, index);
            wanted.insert((index, coord), (chunk.clone(), data, transform));
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
        let bundle = (of, tilemap_chunk, TilemapChunkTileData(data), transform);
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
        chunk_entity.insert((chunk, TilemapChunkTileData(data)));
    } else if !same_data {
        chunk_entity.insert(TilemapChunkTileData(data));
    }
    if chunk_entity.get::<Transform>() != Some(&transform) {
        chunk_entity.insert(transform);
    }
}

fn tile_key(tile: &Option<TileData>) -> Option<(u16, bool, TileOrientation, Color)> {
    tile.map(|t| (t.tileset_index, t.visible, t.orientation, t.color))
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: usize = 3;
    type Pixels = [[u8; N]; N];

    fn source() -> Pixels {
        [[0, 1, 2], [3, 4, 5], [6, 7, 8]]
    }

    /// The tile as the kernel defines its bits, rows top down: transpose,
    /// then flip x, then flip y.
    fn kernel_oriented(tile: TileRef) -> Pixels {
        let image = source();
        let mut out = [[0; N]; N];
        for (y, row) in image.iter().enumerate() {
            for (x, &pixel) in row.iter().enumerate() {
                let (mut tx, mut ty) = if tile.transpose() { (y, x) } else { (x, y) };
                if tile.flip_x() {
                    tx = N - 1 - tx;
                }
                if tile.flip_y() {
                    ty = N - 1 - ty;
                }
                out[ty][tx] = pixel;
            }
        }
        out
    }

    /// The tile as the chunk shader samples it, in uv (y down): mirror h,
    /// then v, then swap the coordinates.
    fn shader_oriented(orientation: TileOrientation) -> Pixels {
        let image = source();
        let mut out = [[0; N]; N];
        for (y, row) in out.iter_mut().enumerate() {
            for (x, pixel) in row.iter_mut().enumerate() {
                let (mut u, mut v) = (x, y);
                if orientation.mirror_h() {
                    u = N - 1 - u;
                }
                if orientation.mirror_v() {
                    v = N - 1 - v;
                }
                if orientation.mirror_d() {
                    (u, v) = (v, u);
                }
                *pixel = image[v][u];
            }
        }
        out
    }

    #[test]
    fn every_orientation_draws_the_tile_the_kernel_means() {
        let mut seen = HashSet::new();
        for bits in 0..8u8 {
            let tile = TileRef::from_raw(5, bits);
            let data = tile_data(tile).expect("a filled cell draws");
            assert_eq!(data.tileset_index, 5);
            assert!(data.visible);
            assert_eq!(
                shader_oriented(data.orientation),
                kernel_oriented(tile),
                "bits {bits:03b} as {:?}",
                data.orientation
            );
            seen.insert(data.orientation);
        }
        assert_eq!(seen.len(), 8, "the eight orientations stay distinct");
    }

    #[test]
    fn hidden_and_empty_cells() {
        let hidden = tile_data(TileRef::new(2).with_visible(false)).expect("hidden still draws");
        assert!(!hidden.visible);
        assert!(tile_data(TileRef::EMPTY).is_none());
    }

    #[test]
    fn cell_zero_sits_at_the_map_origin() {
        let size = UVec2::new(16, 8);
        let at = chunk_transform(ChunkCoord(IVec2::new(-1, 2)), size, 3);
        let half = Vec2::new(16.0 * 16.0, 16.0 * 8.0);
        let min = at.translation.truncate() - half;
        assert_eq!(min, Vec2::new(-32.0 * 16.0, 64.0 * 8.0));
        assert_eq!(at.translation.z, 3.0);
    }
}
