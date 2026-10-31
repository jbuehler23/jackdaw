//! Parametric twins of the tile tools: rectangle, line, flood fill and clear
//! from named cells.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{PropertyValue, Tilemap};
use jackdaw_tilemap::CellCoord;

use crate::tile_tools::{
    Paint, TileBrush, TileEditMode, TileTool, brush_write, fill_from, paint_cells, paint_shape,
    read_brush, with_layer_tileset,
};
use crate::tilemap::{TilemapDataStore, editable_key};
use crate::tilemap_ops::finish;
use crate::tilemap_stroke::{TileWrite, in_bounds, layer_named};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TilemapFillRectOp>()
        .register_operator::<TilemapDrawLineOp>()
        .register_operator::<TilemapFillOp>()
        .register_operator::<TilemapClearOp>();
}

/// Fill a rectangle of cells.
#[operator(
    id = "tilemap.fill_rect",
    label = "Fill Tilemap Rectangle",
    description = "Fill the rectangle between two cells, both included, with a terrain or a \
                   tile, or erase it, as one history entry.",
    allows_undo = false,
    is_available = crate::tilemap_ops::no_modal,
    params(
        entity(Entity, doc = "The tilemap; defaults to the selected one."),
        layer(String, doc = "Layer name or index; defaults to the painted layer, else 0."),
        x0(i64, doc = "Column of the first corner."),
        y0(i64, doc = "Row of the first corner."),
        x1(i64, doc = "Column of the second corner."),
        y1(i64, doc = "Row of the second corner."),
        terrain(String, doc = "Terrain name or id to paint on a terrain layer."),
        tile(String, doc = "Tile as sheet:tile, packed layer or tile name to place."),
        erase(bool, doc = "Erase the cells instead."),
        mode(String, doc = "connect joins every neighbour of the terrain, the default; path \
                            joins only the cells written."),
    )
)]
pub(crate) fn tilemap_fill_rect(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let result = shape(world, &params, TileTool::Rectangle);
    finish(world, TilemapFillRectOp::ID, result)
}

/// Draw a line of cells.
#[operator(
    id = "tilemap.draw_line",
    label = "Draw Tilemap Line",
    description = "Draw a line of cells between two cells with a terrain or a tile, or erase \
                   it, as one history entry.",
    allows_undo = false,
    is_available = crate::tilemap_ops::no_modal,
    params(
        entity(Entity, doc = "The tilemap; defaults to the selected one."),
        layer(String, doc = "Layer name or index; defaults to the painted layer, else 0."),
        x0(i64, doc = "Column of the first end."),
        y0(i64, doc = "Row of the first end."),
        x1(i64, doc = "Column of the second end."),
        y1(i64, doc = "Row of the second end."),
        terrain(String, doc = "Terrain name or id to paint on a terrain layer."),
        tile(String, doc = "Tile as sheet:tile, packed layer or tile name to place."),
        erase(bool, doc = "Erase the cells instead."),
        mode(String, doc = "connect joins every neighbour of the terrain, the default; path \
                            joins only the cells written."),
    )
)]
pub(crate) fn tilemap_draw_line(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let result = shape(world, &params, TileTool::Line);
    finish(world, TilemapDrawLineOp::ID, result)
}

/// Flood fill from a cell.
#[operator(
    id = "tilemap.fill",
    label = "Fill Tilemap Region",
    description = "Fill the cells matching a seed cell with a terrain or a tile, or erase them, \
                   as one history entry: matching by terrain on a terrain layer, by the tile \
                   shown when placing a tile or on a tile layer.",
    allows_undo = false,
    is_available = crate::tilemap_ops::no_modal,
    params(
        entity(Entity, doc = "The tilemap; defaults to the selected one."),
        layer(String, doc = "Layer name or index; defaults to the painted layer, else 0."),
        x(i64, doc = "Column of the seed cell."),
        y(i64, doc = "Row of the seed cell."),
        terrain(String, doc = "Terrain name or id to paint on a terrain layer."),
        tile(String, doc = "Tile as sheet:tile, packed layer or tile name to place."),
        erase(bool, doc = "Erase the cells instead."),
        contiguous(bool, default = true, doc = "Fill only matching cells joined to the seed; \
                                                 off fills every matching cell on the layer."),
    )
)]
pub(crate) fn tilemap_fill(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let result = fill(world, &params);
    finish(world, TilemapFillOp::ID, result)
}

/// Clear a layer.
#[operator(
    id = "tilemap.clear",
    label = "Clear Tilemap Layer",
    description = "Erase every cell of a layer, or of the rectangle x0,y0,x1,y1 when given, \
                   terrain and hand-placed tiles alike, as one history entry.",
    allows_undo = false,
    is_available = crate::tilemap_ops::no_modal,
    params(
        entity(Entity, doc = "The tilemap; defaults to the selected one."),
        layer(String, doc = "Layer name or index; defaults to the painted layer, else 0."),
        x0(i64, doc = "Column of the first corner of the rectangle to clear."),
        y0(i64, doc = "Row of the first corner."),
        x1(i64, doc = "Column of the second corner."),
        y1(i64, doc = "Row of the second corner."),
    )
)]
pub(crate) fn tilemap_clear(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let result = clear(world, &params);
    finish(world, TilemapClearOp::ID, result)
}

/// The map and layer a call names: the `entity` param or the selected map,
/// and the `layer` param, else the layer tile edit mode paints on that map,
/// else 0.
struct Target {
    map: Entity,
    layer: usize,
    name: String,
}

fn target(world: &mut World, params: &OperatorParameters) -> Result<Target, String> {
    let map =
        crate::tilemap_ops::target_entity(world, params).ok_or("no tilemap given or selected")?;
    let tilemap = world
        .get::<Tilemap>(map)
        .ok_or("the entity is not a tilemap")?;
    let mode = world.resource::<TileEditMode>();
    let painting = if mode.map == Some(map) { mode.layer } else { 0 };
    let layer = match params.get("layer") {
        None => painting,
        Some(PropertyValue::String(named)) if named.trim().is_empty() => painting,
        Some(PropertyValue::String(named)) => layer_named(tilemap, named.trim())?,
        Some(PropertyValue::Int(index)) => usize::try_from(*index)
            .ok()
            .filter(|i| *i < tilemap.layers.len())
            .ok_or_else(|| format!("the map has no layer {index}"))?,
        Some(other) => return Err(format!("layer is a name or an index, not {other:?}")),
    };
    let name = (tilemap.layers.get(layer))
        .ok_or_else(|| format!("the map has no layer {layer}"))?
        .name
        .clone();
    Ok(Target { map, layer, name })
}

/// What the `terrain`, `tile` or `erase` param writes with `tool`, read as the
/// brush of that tool reads it.
fn write(
    world: &mut World,
    target: &Target,
    params: &OperatorParameters,
    tool: TileTool,
) -> Result<TileWrite, String> {
    let given = |name: &str| params.as_str(name).is_some_and(|t| !t.trim().is_empty());
    let erase = params.as_bool("erase").unwrap_or(false);
    let (terrain, tile) = (given("terrain"), given("tile"));
    if usize::from(terrain) + usize::from(tile) + usize::from(erase) != 1 {
        return Err("give one of terrain, tile or erase".into());
    }
    if erase {
        return Ok(TileWrite::Erase);
    }
    with_layer_tileset(world, target.map, target.layer, |tileset, terrain_layer| {
        if terrain && !terrain_layer {
            return Err(format!(
                "layer `{}` is a tile layer; give a tile",
                target.name
            ));
        }
        let layer = Some((tileset, terrain_layer));
        let brush = read_brush(params, TileBrush::default(), layer)?;
        brush_write(tileset, terrain_layer, &brush, tool)
    })
}

/// The cell coordinate param `name`: `None` when left out, refused when it is
/// not an integer or past the cell range.
fn coord(params: &OperatorParameters, name: &str) -> Result<Option<i32>, String> {
    if params.get(name).is_none() {
        return Ok(None);
    }
    let value = (params.as_int(name)).ok_or_else(|| format!("{name} is not an integer"))?;
    (i32::try_from(value).map(Some)).map_err(|_| format!("{name} is out of range"))
}

fn cell(params: &OperatorParameters, x: &str, y: &str) -> Result<CellCoord, String> {
    let get = |name: &str| coord(params, name)?.ok_or_else(|| format!("{name} is required"));
    Ok(CellCoord::new(get(x)?, get(y)?))
}

fn shape(world: &mut World, params: &OperatorParameters, tool: TileTool) -> Result<String, String> {
    let target = target(world, params)?;
    let (from, to) = (cell(params, "x0", "y0")?, cell(params, "x1", "y1")?);
    let paint = Paint::Write(write(world, &target, params, tool)?);
    let count = paint_shape(world, (target.map, target.layer), tool, &paint, from, to)?;
    Ok(written(count, &target))
}

fn fill(world: &mut World, params: &OperatorParameters) -> Result<String, String> {
    let target = target(world, params)?;
    let start = cell(params, "x", "y")?;
    let write = write(world, &target, params, TileTool::Bucket)?;
    let bounds = (world.get::<Tilemap>(target.map)).and_then(|t| t.bounds);
    if in_bounds(bounds, &[start]).is_empty() {
        let at = start.0;
        return Err(format!(
            "cell {},{} is outside the map's bounds",
            at.x, at.y
        ));
    }
    let contiguous = params.as_bool("contiguous").unwrap_or(true);
    let count = fill_from(world, target.map, target.layer, start, write, contiguous)?;
    Ok(written(count, &target))
}

fn clear(world: &mut World, params: &OperatorParameters) -> Result<String, String> {
    let target = target(world, params)?;
    let corners = ["x0", "y0", "x1", "y1"].map(|name| coord(params, name));
    let corners: Vec<i32> = (corners.into_iter().collect::<Result<Vec<_>, _>>()?)
        .into_iter()
        .flatten()
        .collect();
    let erase = Paint::Write(TileWrite::Erase);
    let at = (target.map, target.layer);
    let count = match corners[..] {
        [x0, y0, x1, y1] => {
            let (from, to) = (CellCoord::new(x0, y0), CellCoord::new(x1, y1));
            paint_shape(world, at, TileTool::Rectangle, &erase, from, to)?
        }
        [] => {
            let cells = painted(world, &target)?;
            paint_cells(world, at.0, at.1, "Clear layer", &erase, &cells)?
        }
        _ => return Err("give x0, y0, x1 and y1, or none".into()),
    };
    Ok(written(count, &target))
}

/// Every cell of the target layer holding a tile, terrain or a hand-placed
/// tile.
fn painted(world: &mut World, target: &Target) -> Result<Vec<CellCoord>, String> {
    let key = editable_key(world, target.map)?;
    let store = world.resource::<TilemapDataStore>();
    let Some(layer) = (store.get(&key)).and_then(|data| data.layers.get(target.layer)) else {
        return Ok(Vec::new());
    };
    let mut cells: Vec<CellCoord> = layer.tiles.cells().map(|(c, _)| c).collect();
    if let Some(a) = &layer.authoring {
        cells.extend(a.terrain.cells().map(|(c, _)| c));
        cells.extend(a.links.cells().map(|(c, _)| c));
        cells.extend(a.overrides.keys().copied());
    }
    cells.sort_unstable();
    cells.dedup();
    Ok(cells)
}

fn written(count: usize, target: &Target) -> String {
    match count {
        0 => "no cell changed".to_string(),
        n => format!("{n} cells written on layer `{}`", target.name),
    }
}
