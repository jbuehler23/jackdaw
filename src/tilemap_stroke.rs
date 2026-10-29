//! Tilemap strokes: one history entry per stroke, holding the before and
//! after of every 32 x 32 chunk it touched on any layer, and the
//! `tilemap.set_cells` operator that makes one.

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::{Tilemap, TilemapLayerKind};
use jackdaw_tilemap::{
    CHUNK_AREA, CHUNK_SIZE, CellCoord, CellValue, ChunkCoord, ChunkedLayer, LayerData, LayerKind,
    Links, TerrainAuthoring, TerrainId, TileRef, TilemapData,
};
use jackdaw_tileset::Tileset;

use crate::commands::EditorCommand;
use crate::tilemap::{TilemapDataStore, editable_key};
use crate::tilemap_resolve::{layer_set, refresh_terrain};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TilemapSetCellsOp>();
}

/// One layer's cells in one chunk.
#[derive(Clone, PartialEq)]
struct ChunkCells {
    tiles: Option<Box<[TileRef; CHUNK_AREA]>>,
    terrain: Option<Box<[TerrainId; CHUNK_AREA]>>,
    links: Option<Box<[Links; CHUNK_AREA]>>,
    overrides: Vec<(CellCoord, TileRef)>,
}

/// The part of a map a stroke touched.
#[derive(Clone)]
struct MapCells {
    /// Every layer's kind and whether it holds authoring.
    layers: Vec<(LayerKind, bool)>,
    chunks: BTreeMap<(usize, ChunkCoord), ChunkCells>,
}

impl MapCells {
    fn capture(data: &TilemapData, chunks: &BTreeSet<(usize, ChunkCoord)>) -> Self {
        let chunks = (chunks.iter())
            .map(|&(index, coord)| {
                let layer = data.layers.get(index);
                let authoring = layer.and_then(|l| l.authoring.as_ref());
                let min = coord.min_cell().0;
                let overrides = (authoring.into_iter()).flat_map(|a| {
                    (min.y..min.y + CHUNK_SIZE).flat_map(move |y| {
                        let row = CellCoord::new(min.x, y)..CellCoord::new(min.x + CHUNK_SIZE, y);
                        a.overrides.range(row).map(|(cell, tile)| (*cell, *tile))
                    })
                });
                let cells = ChunkCells {
                    tiles: (layer.and_then(|l| l.tiles.chunk(coord))).map(|c| Box::new(*c)),
                    terrain: (authoring.and_then(|a| a.terrain.chunk(coord))).map(|c| Box::new(*c)),
                    links: (authoring.and_then(|a| a.links.chunk(coord))).map(|c| Box::new(*c)),
                    overrides: overrides.collect(),
                };
                ((index, coord), cells)
            })
            .collect();
        Self {
            layers: (data.layers.iter())
                .map(|l| (l.kind, l.authoring.is_some()))
                .collect(),
            chunks,
        }
    }

    /// Drops the chunks `before` and `after` hold alike.
    fn keep_changed(before: &mut Self, after: &mut Self) {
        before
            .chunks
            .retain(|at, cells| after.chunks.get(at) != Some(cells));
        let kept: BTreeSet<_> = before.chunks.keys().copied().collect();
        after.chunks.retain(|at, _| kept.contains(at));
    }

    /// Writes the cells back, returning the authoring cells written by layer.
    /// The tiles of the `resolved` layers are left to the edit, which
    /// resolves them from the restored authoring with the set the layer
    /// resolves with now. Layers keep their kind, which the scene gives.
    fn apply(
        &self,
        data: &mut TilemapData,
        resolved: &BTreeSet<usize>,
    ) -> BTreeMap<usize, Vec<CellCoord>> {
        let mut written: BTreeMap<usize, Vec<CellCoord>> = BTreeMap::new();
        while let Some(&(kind, _)) = self.layers.get(data.layers.len()) {
            data.layers.push(LayerData {
                kind,
                tiles: ChunkedLayer::new(),
                authoring: None,
            });
        }
        for (&(index, coord), cells) in &self.chunks {
            let Some(layer) = data.layers.get_mut(index) else {
                continue;
            };
            if !resolved.contains(&index) {
                put(&mut layer.tiles, coord, cells.tiles.as_deref());
            }
            let authored =
                cells.terrain.is_some() || cells.links.is_some() || !cells.overrides.is_empty();
            if authored && layer.authoring.is_none() {
                layer.authoring = Some(TerrainAuthoring::default());
            }
            if let Some(authoring) = &mut layer.authoring {
                let cells_written = written.entry(index).or_default();
                cells_written.extend(put(&mut authoring.terrain, coord, cells.terrain.as_deref()));
                cells_written.extend(put(&mut authoring.links, coord, cells.links.as_deref()));
                authoring.overrides.retain(|cell, _| cell.chunk() != coord);
                authoring.overrides.extend(cells.overrides.iter().copied());
            }
        }
        data.layers.truncate(self.layers.len());
        for (layer, &(_, authored)) in data.layers.iter_mut().zip(&self.layers) {
            if !authored {
                layer.authoring = None;
            } else if layer.authoring.is_none() {
                layer.authoring = Some(TerrainAuthoring::default());
            }
        }
        written
    }

    fn heap_bytes(&self) -> usize {
        self.chunks.len() * CHUNK_AREA * (size_of::<TileRef>() + 2)
    }
}

/// Writes a chunk's cells, setting only the cells that differ, and returns
/// those cells.
fn put<T: CellValue>(
    layer: &mut ChunkedLayer<T>,
    coord: ChunkCoord,
    cells: Option<&[T; CHUNK_AREA]>,
) -> Vec<CellCoord> {
    if layer.chunk(coord) == cells {
        return Vec::new();
    }
    (0..CHUNK_AREA)
        .map(|local| coord.cell(local))
        .filter(|&cell| {
            let value = cells.map_or(T::EMPTY, |c| c[cell.local_index()]);
            layer.set(cell, value) != value
        })
        .collect()
}

/// One stroke on one map: undo and redo write the touched chunks back
/// through the store's edit path, which resolves their terrain and redraws
/// only those chunks. It names the map by its store id, so it reaches the map
/// across a tab swap and a save to a new file, and never a map that took the
/// map's old key.
const GONE: &str = "tile edits to a map that is no longer open were dropped";

pub struct TilemapStroke {
    map: u64,
    label: String,
    before: MapCells,
    after: MapCells,
}

impl TilemapStroke {
    fn put(&self, world: &mut World, cells: &MapCells) {
        let mut store = world.resource_mut::<TilemapDataStore>();
        let Some(key) = store.key_of_map(self.map).map(str::to_owned) else {
            warn!("{}: {GONE}", self.label);
            return;
        };
        let resolved: BTreeSet<usize> = (cells.layers.iter().enumerate())
            .filter(|(layer, (_, authored))| {
                *authored && store.resolved_with(&key, *layer).is_some()
            })
            .map(|(layer, _)| layer)
            .collect();
        if let Some(mut edit) = store.get_mut(&key) {
            for (layer, cells) in cells.apply(&mut edit, &resolved) {
                edit.mark_written(layer, cells);
            }
        }
    }
}

impl EditorCommand for TilemapStroke {
    fn execute(&mut self, world: &mut World) {
        self.put(world, &self.after);
    }

    fn undo(&mut self, world: &mut World) {
        self.put(world, &self.before);
    }

    fn description(&self) -> &str {
        &self.label
    }

    fn heap_bytes(&self) -> usize {
        self.before.heap_bytes() + self.after.heap_bytes()
    }

    fn gone(&self, world: &World) -> Option<String> {
        let store = world.get_resource::<TilemapDataStore>()?;
        store
            .key_of_map(self.map)
            .is_none()
            .then(|| GONE.to_string())
    }
}

/// Cells as `x,y` pairs split by `;`, or one rect `x0,y0,x1,y1` with both
/// corners included. Spaces, tabs and newlines around numbers are ignored.
fn parse_cells(text: &str) -> Result<Vec<CellCoord>, String> {
    const MAX: i64 = 1 << 20;
    let number =
        |n: &str| (n.trim().parse::<i32>()).map_err(|_| format!("`{n}` is not a cell coordinate"));
    let groups: Vec<Vec<i32>> = (text.split([';', '\n']))
        .filter(|g| !g.trim().is_empty())
        .map(|g| g.split(',').map(number).collect())
        .collect::<Result<_, _>>()?;
    if let [rect] = groups.as_slice()
        && let [x0, y0, x1, y1] = rect[..]
    {
        let (min, max) = (
            IVec2::new(x0.min(x1), y0.min(y1)),
            IVec2::new(x0.max(x1), y0.max(y1)),
        );
        let area =
            (i64::from(max.x) - i64::from(min.x) + 1) * (i64::from(max.y) - i64::from(min.y) + 1);
        if area > MAX {
            return Err(format!("the rect holds {area} cells, more than {MAX}"));
        }
        return Ok((min.y..=max.y)
            .flat_map(|y| (min.x..=max.x).map(move |x| CellCoord::new(x, y)))
            .collect());
    }
    let cells: Vec<CellCoord> = (groups.iter())
        .map(|g| match g[..] {
            [x, y] => Ok(CellCoord::new(x, y)),
            _ => Err(format!(
                "cells are x,y pairs split by `;` or one rect x0,y0,x1,y1, not `{text}`"
            )),
        })
        .collect::<Result<_, _>>()?;
    if cells.is_empty() {
        return Err("no cells given".into());
    }
    Ok(cells)
}

/// What a `tilemap.set_cells` call writes.
enum Write {
    Tile(TileRef),
    Terrain(TerrainId),
}

/// Paint, place or erase tilemap cells.
#[operator(
    id = "tilemap.set_cells",
    label = "Set Tilemap Cells",
    description = "Paint terrain, place tiles or erase cells on one layer of a tilemap, as one \
                   history entry.",
    allows_undo = false,
    params(
        entity(Entity, doc = "The tilemap."),
        layer(String, doc = "Layer name or index."),
        cells(
            String,
            doc = "Cells as x,y pairs split by `;`, or one rect x0,y0,x1,y1 with both corners \
                   included."
        ),
        kind(
            String,
            default = "terrain",
            doc = "terrain paints terrain on a terrain layer; tile places tiles on a tile layer, \
                   or hand-placed tiles over a terrain layer's terrain."
        ),
        value(
            String,
            doc = "A terrain id or name for terrain, a tile as sheet:tile or packed layer for \
                   tile, or erase."
        ),
    )
)]
pub(crate) fn tilemap_set_cells(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    match set_cells(world, &params) {
        Ok(message) => {
            report_to_caller(world, message);
            OperatorResult::Finished
        }
        Err(err) => {
            warn_caller(world, format!("{}: {err}", TilemapSetCellsOp::ID));
            OperatorResult::Cancelled
        }
    }
}

fn set_cells(world: &mut World, params: &OperatorParameters) -> Result<String, String> {
    let text = |name: &str| params.as_str(name).unwrap_or_default().trim().to_string();
    let entity = params
        .as_entity("entity")
        .ok_or("no tilemap entity given")?;
    let tilemap = (world.get::<Tilemap>(entity).cloned()).ok_or("the entity is not a tilemap")?;
    let key = editable_key(world, entity)?;
    let named = text("layer");
    let index = (tilemap.layers.iter().position(|l| l.name == named))
        .or_else(|| named.parse().ok().filter(|i| *i < tilemap.layers.len()))
        .ok_or_else(|| format!("the map has no layer `{named}`"))?;
    let layer = &tilemap.layers[index];
    let terrain_layer = layer.kind == TilemapLayerKind::Terrain;
    let terrain_kind = match text("kind").as_str() {
        "" | "terrain" => true,
        "tile" => false,
        other => return Err(format!("kind is tile or terrain, not `{other}`")),
    };
    if terrain_kind && !terrain_layer {
        return Err(format!(
            "layer `{}` is a tile layer; paint it with kind tile",
            layer.name
        ));
    }
    let cells = parse_cells(&text("cells"))?;
    if let Some(bounds) = tilemap.bounds
        && let Some(out) =
            (cells.iter()).find(|c| !(c.0.cmpge(bounds.min).all() && c.0.cmplt(bounds.max).all()))
    {
        return Err(format!(
            "cell {},{} is outside the map's bounds",
            out.0.x, out.0.y
        ));
    }
    let id = crate::tileset_dock::tileset_id(world, &layer.tileset)?;
    let tileset =
        (world.resource::<Assets<Tileset>>().get(id)).ok_or("the layer's tileset is not loaded")?;
    let value = text("value");
    let write = if value == "erase" {
        if terrain_kind {
            Write::Terrain(0)
        } else {
            Write::Tile(TileRef::EMPTY)
        }
    } else if terrain_kind {
        Write::Terrain(crate::tileset_edit::terrain(tileset, &value)?)
    } else {
        let (sheet, at) = crate::tileset_edit::tile(tileset, &value)?;
        let packed = (tileset.layer(sheet, at)).and_then(|l| u16::try_from(l).ok());
        Write::Tile(TileRef::new(
            packed.ok_or_else(|| format!("tile `{value}` is out of range"))?,
        ))
    };
    if terrain_layer {
        layer_set(world, layer)
            .ok_or("the layer's tileset is not loaded")?
            .map_err(|err| format!("layer `{}`: {err}", layer.name))?;
    }
    let store = world.resource::<TilemapDataStore>();
    let map = store.map_id(&key).ok_or("the map's cells are not loaded")?;
    let current = store.get(&key).and_then(|data| data.layers.get(index));
    if !changes(current, terrain_layer, &cells, &write) {
        return Ok("no cell changed".into());
    }
    if terrain_layer && store.resolved_with(&key, index).is_none() {
        refresh_terrain(world);
    }

    let around: BTreeSet<ChunkCoord> = cells.iter().map(|cell| cell.chunk()).collect();
    let chunks: BTreeSet<(usize, ChunkCoord)> = (around.iter())
        .flat_map(|chunk| {
            (-1..=1)
                .flat_map(move |y| (-1..=1).map(move |x| ChunkCoord(chunk.0 + IVec2::new(x, y))))
        })
        .map(|chunk| (index, chunk))
        .collect();
    let empty = TilemapData::default();
    let store = world.resource::<TilemapDataStore>();
    let mut before = MapCells::capture(store.get(&key).unwrap_or(&empty), &chunks);
    {
        let mut store = world.resource_mut::<TilemapDataStore>();
        let mut edit = store
            .get_mut(&key)
            .ok_or("the map's cells are not loaded")?;
        write_cells(&mut edit, &tilemap, index, &cells, &write);
        if terrain_layer {
            edit.mark_written(index, cells.iter().copied());
        }
    }
    let store = world.resource::<TilemapDataStore>();
    let mut after = MapCells::capture(store.get(&key).unwrap_or(&empty), &chunks);
    MapCells::keep_changed(&mut before, &mut after);
    let stroke = TilemapStroke {
        map,
        label: format!("Set {} tilemap cells", cells.len()),
        before,
        after,
    };
    world
        .resource_mut::<CommandHistory>()
        .push_executed(Box::new(stroke));
    Ok(format!(
        "{} cells set on layer `{}`",
        cells.len(),
        layer.name
    ))
}

/// Whether writing `write` to `cells` of `layer` changes any of them.
fn changes(
    layer: Option<&LayerData>,
    terrain_layer: bool,
    cells: &[CellCoord],
    write: &Write,
) -> bool {
    let authoring = layer.and_then(|layer| layer.authoring.as_ref());
    cells.iter().any(|&cell| match *write {
        Write::Terrain(id) => authoring.map_or(0, |a| a.terrain.get(cell)) != id,
        Write::Tile(tile) if terrain_layer => {
            let placed = authoring.and_then(|a| a.overrides.get(&cell).copied());
            placed != (!tile.is_empty()).then_some(tile)
        }
        Write::Tile(tile) => layer.map_or(TileRef::EMPTY, |l| l.tiles.get(cell)) != tile,
    })
}

/// Writes `write` to `cells` of layer `index`, adding the layers the map
/// lists that the cells do not hold yet. A terrain layer, so kept by the
/// scene, takes terrain, or tiles as overrides; a tile layer takes tiles as
/// they are.
fn write_cells(
    data: &mut TilemapData,
    tilemap: &Tilemap,
    index: usize,
    cells: &[CellCoord],
    write: &Write,
) {
    for layer in tilemap.layers.iter().skip(data.layers.len()) {
        let terrain = layer.kind == TilemapLayerKind::Terrain;
        data.layers.push(LayerData {
            kind: if terrain {
                LayerKind::Terrain
            } else {
                LayerKind::Tile
            },
            tiles: ChunkedLayer::new(),
            authoring: terrain.then(TerrainAuthoring::default),
        });
    }
    let target = &mut data.layers[index];
    if tilemap.layers[index].kind != TilemapLayerKind::Terrain {
        if let Write::Tile(tile) = *write {
            for &cell in cells {
                target.tiles.set(cell, tile);
            }
        }
        return;
    }
    target.kind = LayerKind::Terrain;
    let authoring = target
        .authoring
        .get_or_insert_with(TerrainAuthoring::default);
    for &cell in cells {
        match *write {
            Write::Terrain(id) => {
                authoring.terrain.set(cell, id);
            }
            Write::Tile(tile) if tile.is_empty() => {
                authoring.overrides.remove(&cell);
            }
            Write::Tile(tile) => {
                authoring.overrides.insert(cell, tile);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile_map() -> TilemapData {
        TilemapData {
            layers: vec![LayerData {
                kind: LayerKind::Tile,
                tiles: ChunkedLayer::new(),
                authoring: None,
            }],
        }
    }

    fn around(chunks: &[(i32, i32)]) -> BTreeSet<(usize, ChunkCoord)> {
        (chunks.iter())
            .map(|&(x, y)| (0, ChunkCoord::new(x, y)))
            .collect()
    }

    #[test]
    fn a_stroke_over_a_chunk_border_keeps_both_chunks_and_restores_them() {
        let mut before = tile_map();
        before.layers[0]
            .tiles
            .set(CellCoord::new(40, 0), TileRef::new(5));
        let mut next = before.clone();
        for x in [31, 32] {
            next.layers[0]
                .tiles
                .set(CellCoord::new(x, 0), TileRef::new(7));
        }
        let chunks = around(&[(-1, 0), (0, 0), (1, 0), (2, 0)]);
        let (mut old, mut new) = (
            MapCells::capture(&before, &chunks),
            MapCells::capture(&next, &chunks),
        );
        MapCells::keep_changed(&mut old, &mut new);
        let kept: Vec<_> = new.chunks.keys().copied().collect();
        assert_eq!(
            kept,
            around(&[(0, 0), (1, 0)]).into_iter().collect::<Vec<_>>()
        );

        let mut data = before.clone();
        new.apply(&mut data, &BTreeSet::new());
        assert_eq!(
            jackdaw_tilemap::encode(&data),
            jackdaw_tilemap::encode(&next)
        );
        old.apply(&mut data, &BTreeSet::new());
        assert_eq!(
            jackdaw_tilemap::encode(&data),
            jackdaw_tilemap::encode(&before)
        );
    }

    #[test]
    fn a_chunk_holds_only_its_own_overrides() {
        let mut data = tile_map();
        let authoring = data.layers[0].authoring.insert(TerrainAuthoring::default());
        for (x, y) in [(0, 0), (31, 31), (32, 0), (-1, 5), (5, 32)] {
            authoring
                .overrides
                .insert(CellCoord::new(x, y), TileRef::new(1));
        }
        let cells = MapCells::capture(&data, &around(&[(0, 0)]));
        let held: Vec<CellCoord> = cells
            .chunks
            .values()
            .next()
            .expect("one chunk")
            .overrides
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(held, [CellCoord::new(0, 0), CellCoord::new(31, 31)]);
    }

    #[test]
    fn padded_layers_go_again_on_undo() {
        let before = TilemapData::default();
        let mut next = tile_map();
        next.layers[0]
            .tiles
            .set(CellCoord::new(1, 1), TileRef::new(2));
        let chunks = around(&[(0, 0)]);
        let old = MapCells::capture(&before, &chunks);
        let mut data = next.clone();
        old.apply(&mut data, &BTreeSet::new());
        assert_eq!(data, before);
    }

    #[test]
    fn cells_parse_as_pairs_or_one_rect() {
        let pairs = parse_cells("1, 2; 3,\t-4;\n5 ,6").expect("pairs");
        assert_eq!(
            pairs,
            [
                CellCoord::new(1, 2),
                CellCoord::new(3, -4),
                CellCoord::new(5, 6)
            ]
        );
        let rect = parse_cells(" 2, 1,\t0, 0 ").expect("rect");
        assert_eq!(rect.len(), 6);
        assert!(parse_cells("1,2,3").is_err());
        assert!(parse_cells("1,2 3,4").is_err());
        assert!(parse_cells("").is_err());
        assert!(parse_cells("0,0,100000,100000").is_err());
    }
}
