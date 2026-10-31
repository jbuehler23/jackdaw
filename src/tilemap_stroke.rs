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
    Links, TerrainAuthoring, TerrainId, TerrainSet, TileRef, TilemapData, apply_connect_stroke,
    apply_path_stroke,
};
use jackdaw_tileset::Tileset;

use crate::commands::EditorCommand;
use crate::tilemap::{TilemapDataStore, data_kind, editable_key, resolve_into};
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

fn put_cells(world: &mut World, map: u64, label: &str, cells: &MapCells) {
    let mut store = world.resource_mut::<TilemapDataStore>();
    let Some(key) = store.key_of_map(map).map(str::to_owned) else {
        warn!("{label}: {GONE}");
        return;
    };
    let resolved: BTreeSet<usize> = (cells.layers.iter().enumerate())
        .filter(|(layer, (_, authored))| *authored && store.resolved_with(&key, *layer).is_some())
        .map(|(layer, _)| layer)
        .collect();
    if let Some(mut edit) = store.get_mut(&key) {
        for (layer, cells) in cells.apply(&mut edit, &resolved) {
            edit.mark_written(layer, cells);
        }
    }
}

impl EditorCommand for TilemapStroke {
    fn execute(&mut self, world: &mut World) {
        put_cells(world, self.map, &self.label, &self.after);
    }

    fn undo(&mut self, world: &mut World) {
        put_cells(world, self.map, &self.label, &self.before);
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

/// What a stroke writes to the cells it touches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileWrite {
    /// A tile on a tile layer, or a hand-placed tile over a terrain layer's
    /// terrain; [`TileRef::EMPTY`] erases.
    Tile(TileRef),
    /// Terrain on a terrain layer, joining every neighbour of that terrain;
    /// 0 erases.
    Terrain(TerrainId),
    /// Terrain on a terrain layer joining only the other cells of the same
    /// write, so two parallel paths stay apart.
    Path(TerrainId),
    /// Clears a cell whole: a terrain layer's terrain and hand-placed tile,
    /// or a tile layer's tile.
    Erase,
}

/// A stroke in progress on one layer of one map: cells are written live as
/// [`Self::apply`] takes them, and [`Self::end`] pushes the whole stroke as one
/// [`TilemapStroke`]. Every tool that paints cells, by hand or by parameters,
/// goes through it.
pub struct TilemapStrokeBuilder {
    map: u64,
    key: String,
    tilemap: Tilemap,
    layer: usize,
    label: String,
    /// The chunks captured so far: the 3 x 3 ring around each touched cell's
    /// chunk, so the resolve ring of every written cell is held.
    chunks: BTreeSet<(usize, ChunkCoord)>,
    before: Option<MapCells>,
}

impl TilemapStrokeBuilder {
    /// Opens a stroke on layer `layer` of the map `entity`, or says why the
    /// map cannot take one.
    pub fn begin(
        world: &mut World,
        entity: Entity,
        layer: usize,
        label: impl Into<String>,
    ) -> Result<Self, String> {
        let tilemap =
            (world.get::<Tilemap>(entity).cloned()).ok_or("the entity is not a tilemap")?;
        let key = editable_key(world, entity)?;
        let target =
            (tilemap.layers.get(layer)).ok_or_else(|| format!("the map has no layer {layer}"))?;
        if target.kind == TilemapLayerKind::Terrain {
            layer_set(world, target)
                .ok_or("the layer's tileset is not loaded")?
                .map_err(|err| format!("layer `{}`: {err}", target.name))?;
            if world
                .resource::<TilemapDataStore>()
                .resolved_with(&key, layer)
                .is_none()
            {
                refresh_terrain(world);
            }
        }
        let map = (world.resource::<TilemapDataStore>().map_id(&key))
            .ok_or("the map's cells are not loaded")?;
        let held = (world.resource::<TilemapDataStore>().get(&key)).map_or(0, |d| d.layers.len());
        if held != 0 && held != tilemap.layers.len() {
            let why = format!(
                "the map lists {} layers but its cells hold {held}; edit layers through the \
                 layer operators",
                tilemap.layers.len()
            );
            if (world.resource_mut::<TilemapDataStore>().mismatch_warned).insert(map) {
                warn!("Tilemap {entity}: {why}");
            }
            return Err(why);
        }
        Ok(Self {
            map,
            key,
            tilemap,
            layer,
            label: label.into(),
            chunks: BTreeSet::new(),
            before: None,
        })
    }

    /// Writes `write` to the cells inside the map's bounds, resolving a
    /// terrain layer's neighbourhood, and returns whether any cell changed.
    pub fn apply(&mut self, world: &mut World, cells: &[CellCoord], write: TileWrite) -> bool {
        let cells = in_bounds(self.tilemap.bounds, cells);
        let terrain_layer = self.tilemap.layers[self.layer].kind == TilemapLayerKind::Terrain;
        let store = world.resource::<TilemapDataStore>();
        let Some(data) = store.get(&self.key) else {
            return false;
        };
        if !changes(data.layers.get(self.layer), terrain_layer, &cells, &write) {
            return false;
        }
        let fresh: BTreeSet<(usize, ChunkCoord)> = (ring_chunks(&cells).into_iter())
            .map(|chunk| (self.layer, chunk))
            .filter(|at| !self.chunks.contains(at))
            .collect();
        let captured = MapCells::capture(data, &fresh);
        match &mut self.before {
            Some(before) => before.chunks.extend(captured.chunks),
            None => self.before = Some(captured),
        }
        self.chunks.extend(fresh);
        let mut store = world.resource_mut::<TilemapDataStore>();
        let Some(mut edit) = store.get_mut(&self.key) else {
            return false;
        };
        pad_layers(&mut edit, &self.tilemap);
        write_layer(&mut edit.layers[self.layer], terrain_layer, &cells, write);
        if terrain_layer {
            edit.mark_written(self.layer, cells);
        }
        true
    }

    /// Closes the stroke, pushing one history entry when it changed any
    /// cell, and returns whether it did.
    pub fn end(self, world: &mut World) -> bool {
        let Some(mut before) = self.before else {
            return false;
        };
        let store = world.resource::<TilemapDataStore>();
        let Some(data) = store.get(&self.key) else {
            return false;
        };
        let mut after = MapCells::capture(data, &self.chunks);
        MapCells::keep_changed(&mut before, &mut after);
        if after.chunks.is_empty() {
            return false;
        }
        let stroke = TilemapStroke {
            map: self.map,
            label: self.label,
            before,
            after,
        };
        world
            .resource_mut::<CommandHistory>()
            .push_executed(Box::new(stroke));
        true
    }

    /// Puts back every cell the stroke wrote, leaving no history entry.
    pub fn cancel(self, world: &mut World) {
        if let Some(before) = self.before {
            put_cells(world, self.map, &self.label, &before);
        }
    }
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
    editable_key(world, entity)?;
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
            TileWrite::Terrain(0)
        } else {
            TileWrite::Tile(TileRef::EMPTY)
        }
    } else if terrain_kind {
        TileWrite::Terrain(crate::tileset_edit::terrain(tileset, &value)?)
    } else {
        TileWrite::Tile(packed_tile(tileset, &value)?)
    };
    let mut stroke = TilemapStrokeBuilder::begin(
        world,
        entity,
        index,
        format!("Set {} tilemap cells", cells.len()),
    )?;
    if !stroke.apply(world, &cells, write) {
        return Ok("no cell changed".into());
    }
    stroke.end(world);
    Ok(format!(
        "{} cells set on layer `{}`",
        cells.len(),
        layer.name
    ))
}

/// A tile named as `sheet:tile` or a packed layer, as the tile it packs to.
pub(crate) fn packed_tile(tileset: &Tileset, value: &str) -> Result<TileRef, String> {
    let (sheet, at) = crate::tileset_edit::tile(tileset, value)?;
    let packed = (tileset.layer(sheet, at)).and_then(|l| u16::try_from(l).ok());
    Ok(TileRef::new(packed.ok_or_else(|| {
        format!("tile `{value}` is out of range")
    })?))
}

/// Whether writing `write` to `cells` of `layer` changes any of them.
fn changes(
    layer: Option<&LayerData>,
    terrain_layer: bool,
    cells: &[CellCoord],
    write: &TileWrite,
) -> bool {
    let authoring = layer.and_then(|layer| layer.authoring.as_ref());
    let terrain = |cell| authoring.map_or(0, |a| a.terrain.get(cell));
    let links = |cell| authoring.map_or(Links::EMPTY, |a| a.links.get(cell));
    if let TileWrite::Path(id) = *write {
        let (mut after_terrain, mut after_links) = (ChunkedLayer::new(), ChunkedLayer::new());
        for &cell in cells {
            after_terrain.set(cell, terrain(cell));
            after_links.set(cell, links(cell));
        }
        paint_path(&mut after_terrain, &mut after_links, cells, id);
        return (cells.iter())
            .any(|&c| after_terrain.get(c) != terrain(c) || after_links.get(c) != links(c));
    }
    cells.iter().any(|&cell| match *write {
        TileWrite::Terrain(id) => terrain(cell) != id || links(cell) != Links::EMPTY,
        TileWrite::Tile(tile) if terrain_layer => {
            let placed = authoring.and_then(|a| a.overrides.get(&cell).copied());
            placed != (!tile.is_empty()).then_some(tile)
        }
        TileWrite::Tile(tile) => layer.map_or(TileRef::EMPTY, |l| l.tiles.get(cell)) != tile,
        TileWrite::Erase if terrain_layer => {
            terrain(cell) != 0
                || links(cell) != Links::EMPTY
                || authoring.is_some_and(|a| a.overrides.contains_key(&cell))
        }
        TileWrite::Erase => layer.is_some_and(|l| !l.tiles.get(cell).is_empty()),
        TileWrite::Path(_) => false,
    })
}

/// The cells inside `bounds`, or all of them on an unbounded map.
pub fn in_bounds(bounds: Option<IRect>, cells: &[CellCoord]) -> Vec<CellCoord> {
    (cells.iter().copied())
        .filter(|c| bounds.is_none_or(|b| c.0.cmpge(b.min).all() && c.0.cmplt(b.max).all()))
        .collect()
}

/// The chunks of `cells` and the ring around each: every chunk a write to
/// them and its resolve reads or writes.
fn ring_chunks(cells: &[CellCoord]) -> BTreeSet<ChunkCoord> {
    let touched: BTreeSet<ChunkCoord> = cells.iter().map(|cell| cell.chunk()).collect();
    (touched.iter())
        .flat_map(|chunk| {
            let at = chunk.0;
            (-1..=1).flat_map(move |y| (-1..=1).map(move |x| ChunkCoord(at + IVec2::new(x, y))))
        })
        .collect()
}

/// Adds the layers the map lists that the cells do not hold yet.
fn pad_layers(data: &mut TilemapData, tilemap: &Tilemap) {
    for layer in tilemap.layers.iter().skip(data.layers.len()) {
        data.layers.push(LayerData::empty(data_kind(layer.kind)));
    }
}

/// Paints `id` over `cells` as Path strokes that join each cell to its side
/// neighbours among `cells` and no others: the kernel's Path stroke along
/// every horizontal run of the cells, then along every vertical run. A
/// cell's first run gives it the stroke's links, and its second keeps them,
/// its terrain then being `id`.
fn paint_path(
    terrain: &mut ChunkedLayer<TerrainId>,
    links: &mut ChunkedLayer<Links>,
    cells: &[CellCoord],
    id: TerrainId,
) {
    let mut sorted = cells.to_vec();
    let keys: [fn(&CellCoord) -> (i32, i32); 2] = [|c| (c.0.y, c.0.x), |c| (c.0.x, c.0.y)];
    for key in keys {
        sorted.sort_unstable_by_key(key);
        sorted.dedup();
        let mut run: Vec<CellCoord> = Vec::new();
        for &cell in &sorted {
            let (line, at) = key(&cell);
            if run
                .last()
                .is_some_and(|last| key(last).0 != line || key(last).1.checked_add(1) != Some(at))
            {
                let _ = apply_path_stroke(terrain, links, &run, id);
                run.clear();
            }
            run.push(cell);
        }
        let _ = apply_path_stroke(terrain, links, &run, id);
    }
}

/// Writes `write` to `cells` of `layer`: terrain, or tiles as hand-placed
/// overrides, on a terrain layer, whose tiles are left to the resolve; tiles
/// as they are on a tile layer. Returns the cells whose tile changed.
fn write_layer(
    layer: &mut LayerData,
    terrain_layer: bool,
    cells: &[CellCoord],
    write: TileWrite,
) -> Vec<(CellCoord, TileRef)> {
    if !terrain_layer {
        let tile = match write {
            TileWrite::Tile(tile) => tile,
            TileWrite::Erase => TileRef::EMPTY,
            TileWrite::Terrain(_) | TileWrite::Path(_) => return Vec::new(),
        };
        let mut changed: Vec<_> = (cells.iter())
            .filter(|&&cell| layer.tiles.set(cell, tile) != tile)
            .map(|&cell| (cell, tile))
            .collect();
        changed.sort_unstable_by_key(|(cell, _)| *cell);
        return changed;
    }
    layer.kind = LayerKind::Terrain;
    let a = layer
        .authoring
        .get_or_insert_with(TerrainAuthoring::default);
    match write {
        TileWrite::Path(id) => paint_path(&mut a.terrain, &mut a.links, cells, id),
        TileWrite::Terrain(id) => {
            apply_connect_stroke(&mut a.terrain, &mut a.links, cells, id);
        }
        TileWrite::Erase => {
            apply_connect_stroke(&mut a.terrain, &mut a.links, cells, 0);
            for cell in cells {
                a.overrides.remove(cell);
            }
        }
        TileWrite::Tile(tile) => {
            for &cell in cells {
                if tile.is_empty() {
                    a.overrides.remove(&cell);
                } else {
                    a.overrides.insert(cell, tile);
                }
            }
        }
    }
    Vec::new()
}

/// Writes `write` to `cells` of `layer` and returns each cell whose tile
/// changed, with its new tile: a terrain layer resolves the written cells and
/// their ring with `set`, as the edit a stroke writes through does on drop,
/// and keeps its tiles without one. The preview runs it on a copy.
pub fn paint_layer(
    layer: &mut LayerData,
    terrain_layer: bool,
    set: Option<&TerrainSet>,
    cells: &[CellCoord],
    write: TileWrite,
) -> Vec<(CellCoord, TileRef)> {
    let changed = write_layer(layer, terrain_layer, cells, write);
    match set {
        Some(set) if terrain_layer => resolve_into(layer, set, cells.iter().copied()).1,
        _ => changed,
    }
}

/// The tiles writing `write` to `cells` of `layer` would change, as
/// [`paint_layer`] gives them, leaving `layer` as it is.
pub fn preview_write(
    layer: Option<&LayerData>,
    terrain_layer: bool,
    set: Option<&TerrainSet>,
    cells: &[CellCoord],
    write: TileWrite,
) -> Vec<(CellCoord, TileRef)> {
    let chunks = ring_chunks(cells);
    let mut copy = LayerData {
        kind: layer.map_or(LayerKind::Tile, |l| l.kind),
        tiles: ChunkedLayer::new(),
        authoring: None,
    };
    if let Some(layer) = layer {
        let from = layer.authoring.as_ref();
        let mut to = from.map(|_| TerrainAuthoring::default());
        for &chunk in &chunks {
            copy_chunk(&layer.tiles, &mut copy.tiles, chunk);
            if let (Some(from), Some(to)) = (from, to.as_mut()) {
                copy_chunk(&from.terrain, &mut to.terrain, chunk);
                copy_chunk(&from.links, &mut to.links, chunk);
            }
        }
        if let (Some(from), Some(to)) = (from, to.as_mut()) {
            to.overrides = (from.overrides.iter())
                .filter(|(cell, _)| chunks.contains(&cell.chunk()))
                .map(|(cell, tile)| (*cell, *tile))
                .collect();
        }
        copy.authoring = to;
    }
    paint_layer(&mut copy, terrain_layer, set, cells, write)
}

fn copy_chunk<T: CellValue>(from: &ChunkedLayer<T>, to: &mut ChunkedLayer<T>, coord: ChunkCoord) {
    for (local, &value) in (from.chunk(coord).into_iter().flatten()).enumerate() {
        if !value.is_empty() {
            to.set(coord.cell(local), value);
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
