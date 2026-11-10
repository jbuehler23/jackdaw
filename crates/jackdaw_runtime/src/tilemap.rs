//! Tilemaps in a built game.
//!
//! A `Tilemap` names a `.jdtiles` sidecar relative to the document it was
//! authored in: the scene, or the prefab an inherited map comes from. The
//! sidecar holds each layer's resolved tiles and where they sit, so the game
//! draws them as they are, with the chunk builder the editor uses, and never
//! resolves terrain.
//!
//! The sidecar is read from disk when the map first draws, as terrain
//! sidecars are. A missing sidecar draws an empty map, with a warning. A map
//! draws again, whole, when its `Tilemap` changes or one of its tilesets does.
//!
//! A map that carries a [`TilemapCollision`] also gets the collision shapes of
//! its tiles, chunk by chunk, each time it draws, with or without a renderer.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bevy::prelude::*;
use bevy::sprite_render::{TilemapChunkMaterial, TilemapChunkMeshCache};
use jackdaw_scene_types::{LayerTerrainMatch, Tilemap, TilemapLayer, TilemapLayerKind};
use jackdaw_tilemap::{
    CHUNK_SIZE, ChunkCollisionBuilder, ChunkCoord, CollisionGroup, Placement, TilemapData,
};
use jackdaw_tileset::chunk::{ChunkDraw, TileLayout, TilemapChunkOf, TilemapEdgePiece};
use jackdaw_tileset::{CollisionLookup, TerrainMatch, Tileset, TilesetPlugin};

use crate::{JackdawCatalog, RuntimeOnly};

pub(crate) fn plugin(app: &mut App) {
    if !app.is_plugin_added::<TilesetPlugin>() {
        app.add_plugins(TilesetPlugin);
    }
    app.register_required_components::<TilemapChunkOf, RuntimeOnly>()
        .register_required_components::<TilemapEdgePiece, RuntimeOnly>()
        .add_systems(Update, queue_draws.after(crate::spawn_loaded_scenes));
}

/// On a map whose sidecar was not found; it draws empty.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct TilemapSidecarMissing;

/// The collision shapes of a map's tiles, by layer and chunk, for a physics
/// integration to build colliders from. The runtime fills it on every map
/// that has it each time the map draws, so an integration requires it on
/// `Tilemap` and reads it when it changes.
#[derive(Component, Clone, Debug, Default, PartialEq)]
pub struct TilemapCollision {
    /// By index into the map's layers, and chunk.
    pub chunks: HashMap<(usize, ChunkCoord), TilemapChunkCollision>,
    revisions: u64,
}

/// The collision of one chunk of one layer.
#[derive(Clone, Debug, PartialEq)]
pub struct TilemapChunkCollision {
    /// The chunk's bottom-left corner in the map's space.
    pub origin: Vec2,
    /// One tile's size in world units, which scales the groups' shapes.
    pub tile_size: Vec2,
    /// By physics layer, solid, then one-way, then trigger, in tiles from
    /// `origin`.
    pub groups: Vec<CollisionGroup>,
    /// Changes whenever the rest of the entry does, so a reader rebuilds
    /// only the chunks that changed.
    pub revision: u64,
}

impl TilemapCollision {
    /// This collision with `chunks` in place of its own, each entry that
    /// is unchanged keeping its revision.
    fn with_chunks(&self, chunks: HashMap<(usize, ChunkCoord), TilemapChunkCollision>) -> Self {
        let mut revisions = self.revisions;
        let chunks = (chunks.into_iter())
            .map(|(key, mut chunk)| {
                match self.chunks.get(&key) {
                    Some(old)
                        if (old.origin, old.tile_size, &old.groups)
                            == (chunk.origin, chunk.tile_size, &chunk.groups) =>
                    {
                        chunk.revision = old.revision;
                    }
                    _ => {
                        revisions += 1;
                        chunk.revision = revisions;
                    }
                }
                (key, chunk)
            })
            .collect();
        Self { chunks, revisions }
    }
}

/// Where a spawned map reads its sidecar, and what it last drew from.
#[derive(Component)]
struct TilemapSource {
    /// Folder of the document that names the map.
    dir: PathBuf,
    /// Where unsaved copies of its sidecars stand in for the files, under Play.
    copies: Option<PathBuf>,
    /// The data path `data` was read from.
    read: Option<String>,
    data: TilemapData,
    tilesets: Vec<AssetId<Tileset>>,
    /// Some layer's tileset was not loaded.
    missing: bool,
}

/// Give every freshly spawned map the folder its sidecar resolves against.
pub(crate) fn attach_sidecars(world: &mut World, dirs: &[crate::SidecarDir]) {
    for crate::SidecarDir {
        entity,
        dir,
        copies,
    } in dirs
    {
        if (world.get::<Tilemap>(*entity)).is_some_and(|map| !map.data_path.is_empty()) {
            world.entity_mut(*entity).insert(TilemapSource {
                dir: dir.clone(),
                copies: copies.clone(),
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
    /// What its chunks draw from, or why they draw nothing.
    draw: Result<ChunkDraw, String>,
    shapes: CollisionLookup,
    collision: ChunkCollisionBuilder,
}

/// The tilesets of one batch by path, each looked up, cloned and given its
/// image once.
type Tilesets = HashMap<String, (Option<AssetId<Tileset>>, Result<Prepared, String>)>;

fn draw_batch(world: &mut World, maps: &[Entity]) {
    let renders = world.contains_resource::<TilemapChunkMeshCache>()
        && world.contains_resource::<Assets<TilemapChunkMaterial>>();
    let only_drawn = |map: &Entity| world.get::<TilemapCollision>(*map).is_none();
    if !renders && maps.iter().any(only_drawn) {
        warn!("this app has no tilemap chunk renderer; tilemaps draw nothing");
    }
    let mut tilesets = Tilesets::new();
    for &map in maps {
        draw_map(world, map, &mut tilesets, renders);
    }
}

fn prepare(
    world: &World,
    path: &str,
    renders: bool,
) -> (Option<AssetId<Tileset>>, Result<Prepared, String>) {
    let id = (world.resource::<JackdawCatalog>().get(path))
        .and_then(|handle| handle.id().try_typed::<Tileset>().ok());
    let Some(tileset) = id.and_then(|id| world.resource::<Assets<Tileset>>().get(id)) else {
        return (id, Err(format!("{path} is not a loaded tileset")));
    };
    let draw = if !renders {
        Err("this app has no tilemap chunk renderer".to_string())
    } else if tileset.image.is_empty() {
        Err("its tileset has no image".to_string())
    } else {
        let image = world.resource::<AssetServer>().load(&tileset.image);
        Ok(ChunkDraw::new(tileset, image))
    };
    let prepared = Prepared {
        tileset: tileset.clone(),
        draw,
        shapes: tileset.collision_lookup(),
        collision: ChunkCollisionBuilder::new(),
    };
    (id, Ok(prepared))
}

/// Draw `map` again from its sidecar, reading the file first when the data
/// path changed, and build its collision if it carries a
/// [`TilemapCollision`].
fn draw_map(world: &mut World, map: Entity, tilesets: &mut Tilesets, renders: bool) {
    let Some(tilemap) = world.get::<Tilemap>(map).cloned() else {
        return;
    };
    let collides = world.get::<TilemapCollision>(map).is_some();
    if !renders && !collides {
        return;
    }
    let name = (world.get::<Name>(map)).map_or_else(|| map.to_string(), ToString::to_string);
    let Some(mut source) = world.entity_mut(map).take::<TilemapSource>() else {
        return;
    };
    if source.read.as_deref() != Some(tilemap.data_path.as_str()) {
        source.data = read_sidecar(
            world,
            map,
            &source.dir,
            source.copies.as_deref(),
            &tilemap.data_path,
            &name,
        );
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
    let mut chunk_collision = HashMap::new();
    for (index, (layer, cells)) in tilemap.layers.iter().zip(&data.layers).enumerate() {
        let (id, prepared) = tilesets
            .entry(layer.tileset.clone())
            .or_insert_with(|| prepare(world, &layer.tileset, renders));
        let assets = world.resource::<Assets<Tileset>>();
        missing |= id.is_none_or(|id| !assets.contains(id));
        ids.extend(*id);
        let Prepared {
            tileset,
            draw,
            shapes,
            collision,
        } = match prepared {
            Ok(prepared) => prepared,
            Err(why) => {
                if !cells.tiles.is_empty() {
                    warn!("Tilemap layer {:?} draws nothing: {why}", layer.name);
                }
                continue;
            }
        };
        let layout = TileLayout {
            placement: placement(layer, tileset).unwrap_or(cells.placement),
            bounds: jackdaw_tileset::map_bounds(tilemap.bounds),
        };
        let tile_size = tileset.display_size(tilemap.cell_size);
        if collides {
            let shapes: &CollisionLookup = shapes;
            for (coord, chunk_cells) in cells.tiles.chunks() {
                let groups = collision.build(
                    coord,
                    chunk_cells,
                    layout.placement,
                    layout.bounds,
                    |tile| shapes.get(u32::from(tile)),
                );
                if groups.is_empty() {
                    continue;
                }
                let origin = (coord.0 * CHUNK_SIZE).as_vec2() + layout.placement.offset();
                let chunk = TilemapChunkCollision {
                    origin: origin * tile_size.as_vec2(),
                    tile_size: tile_size.as_vec2(),
                    groups,
                    revision: 0,
                };
                chunk_collision.insert((index, coord), chunk);
            }
        }
        if !renders {
            continue;
        }
        let draw = match draw {
            Ok(draw) => draw,
            Err(why) => {
                if !cells.tiles.is_empty() {
                    warn!("Tilemap layer {:?} draws nothing: {why}", layer.name);
                }
                continue;
            }
        };
        if layout.crops() && draw.strip.is_none() {
            draw.strip = jackdaw_tileset::chunk::strip_for(world, &draw.image);
        }
        for (coord, chunk_cells) in cells.tiles.chunks() {
            let bundle = draw.bundle(coord, chunk_cells, tile_size, index, layout);
            let pieces = draw.edge_pieces(coord, chunk_cells, tile_size, layout);
            let of = TilemapChunkOf {
                layer: index,
                chunk: coord,
            };
            let chunk = world
                .spawn((of, bundle, ChildOf(map), jackdaw_scene_types::SceneDerived))
                .id();
            for piece in pieces {
                world.spawn((piece, ChildOf(chunk), jackdaw_scene_types::SceneDerived));
            }
        }
    }
    source.tilesets = ids;
    source.missing = missing;
    world.entity_mut(map).insert(source);
    if let Some(mut current) = world.get_mut::<TilemapCollision>(map) {
        let next = current.with_chunks(chunk_collision);
        current.set_if_neq(next);
    }
}

/// Where a layer's tiles sit by its tileset: a terrain layer's set match
/// mode decides, so a sidecar written before the placement was stored draws
/// like a fresh one. `None` for a tile layer or a mode the tileset lacks.
fn placement(layer: &TilemapLayer, tileset: &Tileset) -> Option<Placement> {
    if layer.kind != TilemapLayerKind::Terrain {
        return None;
    }
    let mode = match layer.terrain_match {
        LayerTerrainMatch::Auto => None,
        LayerTerrainMatch::Corners => Some(TerrainMatch::Corners),
        LayerTerrainMatch::Sides => Some(TerrainMatch::Sides),
        LayerTerrainMatch::CornersAndSides => Some(TerrainMatch::CornersAndSides),
    };
    tileset.placement(mode)
}

fn read_sidecar(
    world: &mut World,
    map: Entity,
    dir: &Path,
    copies: Option<&Path>,
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
    let copy = copies.and_then(|copies| jackdaw_tilemap::resolve_path(copies, data_path).ok());
    match crate::play_overlay::read_sidecar(copy.as_deref(), &path) {
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
