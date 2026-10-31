//! A tilemap as text: `tilemap.describe`, and `tilemap.read_region` and
//! `tilemap.write_region` over region text, rows plus a legend.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::{Tilemap, TilemapLayerKind};
use jackdaw_tilemap::{
    CellCoord, LayerData, Legend, LegendEntry, Links, Region, TerrainId, TileRef, emit_rows,
    parse_rows,
};
use jackdaw_tileset::Tileset;
use serde::de::{Deserialize, Deserializer, MapAccess, Visitor};

use crate::tile_tools::{shown_tile, with_layer_tileset};
use crate::tilemap::{TilemapDataStore, editable_key, store_key_of};
use crate::tilemap_ops::{MATCHES, finish};
use crate::tilemap_resolve::refresh_terrain;
use crate::tilemap_shape_ops::{cell, target, written};
use crate::tilemap_stroke::{TileWrite, TilemapStrokeBuilder, inside, packed_tile};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TilemapDescribeOp>()
        .register_operator::<TilemapReadRegionOp>()
        .register_operator::<TilemapWriteRegionOp>();
}

/// Tell the caller what a tilemap holds.
#[operator(
    id = "tilemap.describe",
    label = "Describe Tilemap",
    description = "Report a tilemap as text: its name, cell size, bounds and painted extent, \
                   and per layer its index, name, kind, tileset, match mode, terrains and \
                   painted cell count.",
    allows_undo = false,
    is_available = crate::tilemap_ops::no_modal,
    params(entity(Entity, doc = "The tilemap; defaults to the selected one.")),
)]
pub(crate) fn tilemap_describe(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let result = describe(world, &params);
    finish(world, TilemapDescribeOp::ID, result)
}

/// Read a rectangle of cells as region text.
#[operator(
    id = "tilemap.read_region",
    label = "Read Tilemap Region",
    description = "Report the rectangle between two cells, both included, as region text: a \
                   `legend:` line mapping symbols to names and a `rows:` block, top row first. \
                   Its top-left cell is the smaller x and the larger y; pass that to \
                   tilemap.write_region to write the text back.",
    allows_undo = false,
    is_available = crate::tilemap_ops::no_modal,
    params(
        entity(Entity, doc = "The tilemap; defaults to the selected one."),
        layer(String, doc = "Layer name or index; defaults to the painted layer, else 0."),
        x0(i64, doc = "Column of the first corner."),
        y0(i64, doc = "Row of the first corner."),
        x1(i64, doc = "Column of the second corner."),
        y1(i64, doc = "Row of the second corner."),
        tiles(bool, doc = "On a terrain layer, read the tiles cells show instead of their \
                           terrain."),
    )
)]
pub(crate) fn tilemap_read_region(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let result = read(world, &params);
    finish(world, TilemapReadRegionOp::ID, result)
}

/// Write region text into a tilemap.
#[operator(
    id = "tilemap.write_region",
    label = "Write Tilemap Region",
    description = "Write region text, as tilemap.read_region reports it, with its top-left \
                   cell at x0,y0, as one history entry. Legend values are terrain names, \
                   tile:NAME or tile:sheet:tile, empty or erase, and keep. `.` erases and `?` \
                   leaves a cell as it is unless the legend maps them. The whole text is \
                   checked before any cell changes.",
    allows_undo = false,
    is_available = crate::tilemap_ops::no_modal,
    params(
        entity(Entity, doc = "The tilemap; defaults to the selected one."),
        layer(String, doc = "Layer name or index; defaults to the painted layer, else 0."),
        x0(i64, doc = "Column of the top-left cell."),
        y0(i64, doc = "Row of the top-left cell."),
        text(String, doc = "A `legend: {\"g\": \"grass\"}` line, then `rows:` and the rows."),
        mode(String, doc = "connect joins every neighbour of each terrain, the default; path \
                            joins only the cells written."),
    )
)]
pub(crate) fn tilemap_write_region(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let result = write(world, &params);
    finish(world, TilemapWriteRegionOp::ID, result)
}

/// The largest region read or written in one call.
pub const MAX_REGION_CELLS: u64 = 128 * 128;

/// Symbols handed out once a name has none of its own free, in order.
const SYMBOLS: &str =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789#%&*+=~^@$!:;<>/|_()[]{}-";

fn span(lo: IVec2, hi: IVec2) -> String {
    format!("{},{} to {},{}", lo.x, lo.y, hi.x, hi.y)
}

fn describe(world: &mut World, params: &OperatorParameters) -> Result<String, String> {
    let map =
        crate::tilemap_ops::target_entity(world, params).ok_or("no tilemap given or selected")?;
    let tilemap = (world.get::<Tilemap>(map).cloned()).ok_or("the entity is not a tilemap")?;
    let name = (world.get::<Name>(map)).map_or_else(|| map.to_string(), ToString::to_string);
    let key = store_key_of(world, map).ok_or("the map's cells are not loaded")?;
    let painted: Vec<Vec<CellCoord>> = (world.resource::<TilemapDataStore>().get(&key))
        .map(|data| data.layers.iter().map(LayerData::painted_cells).collect())
        .unwrap_or_default();
    let extent = (painted.iter().flatten()).fold(None, |extent: Option<(IVec2, IVec2)>, c| {
        Some(extent.map_or((c.0, c.0), |(lo, hi)| (lo.min(c.0), hi.max(c.0))))
    });
    let bounds = (tilemap.bounds).map_or("unbounded".into(), |b| {
        format!("bounds {}", span(b.min, b.max - IVec2::ONE))
    });
    let extent = extent.map_or("painted nothing".into(), |(lo, hi)| {
        format!("painted {}", span(lo, hi))
    });
    let size = tilemap.cell_size;
    let mut out = format!(
        "tilemap {name:?}: cell {}x{}, {bounds}, {extent}\n",
        size.x, size.y
    );
    for (index, layer) in tilemap.layers.iter().enumerate() {
        let terrain = layer.kind == TilemapLayerKind::Terrain;
        let kind = if terrain { "terrain" } else { "tile" };
        let _ = write!(
            out,
            "layer {index} {:?}: {kind}, tileset {:?}",
            layer.name, layer.tileset
        );
        if terrain {
            let mode = (MATCHES.iter())
                .find(|(_, m)| *m == layer.terrain_match)
                .map_or("auto", |(name, _)| name);
            let _ = write!(out, ", match {mode}");
        }
        let count = painted.get(index).map_or(0, Vec::len);
        let _ = writeln!(out, ", {count} cells painted");
        match with_layer_tileset(world, map, index, |set, _| Ok(names(set, terrain))) {
            Ok(names) => out.push_str(&names),
            Err(err) => {
                let _ = writeln!(out, "  {err}");
            }
        }
    }
    Ok(out)
}

/// The terrain names of a terrain layer's tileset and its named tiles.
fn names(tileset: &Tileset, terrain: bool) -> String {
    let mut out = String::new();
    let mut terrains: Vec<_> = tileset.terrains.iter().collect();
    terrains.sort_by_key(|t| t.id);
    if terrain && !terrains.is_empty() {
        let listed: Vec<String> = (terrains.iter())
            .map(|t| format!("{} {:?}", t.id, t.name))
            .collect();
        let _ = writeln!(out, "  terrains: {}", listed.join(", "));
    }
    let named: Vec<String> = (tileset.properties.iter())
        .filter_map(|(&layer, props)| Some((props.get("name")?, tileset.tile_of_layer(layer)?)))
        .map(|(name, (sheet, tile))| format!("{name:?} {sheet}:{tile}"))
        .collect();
    if !named.is_empty() {
        let _ = writeln!(out, "  named tiles: {}", named.join(", "));
    }
    out
}

/// What a cell reads as.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Shown {
    Empty,
    /// A cell text cannot state, kept on writing: past the map's bounds, or
    /// terrain whose tile does not resolve when reading tiles.
    Keep,
    Terrain(TerrainId),
    /// A tile's index and orientation.
    Tile(u16, u8),
}

/// How a read sees the target layer.
#[derive(Clone, Copy)]
struct View {
    terrain_layer: bool,
    tiles: bool,
    bounds: Option<IRect>,
}

/// A terrain layer's terrain, or its hand-placed tile, when read as terrain;
/// the tile a cell shows otherwise.
fn shown(layer: Option<&LayerData>, cell: CellCoord, view: View) -> Shown {
    if !inside(view.bounds, cell) {
        return Shown::Keep;
    }
    let tile_of = |tile: TileRef| Shown::Tile(tile.index(), tile.orientation());
    if !view.terrain_layer {
        let tile = layer.map_or(TileRef::EMPTY, |l| l.tiles.get(cell));
        return if tile.is_empty() {
            Shown::Empty
        } else {
            tile_of(tile)
        };
    }
    let (tile, placed) = shown_tile(layer, cell);
    let authoring = layer.and_then(|l| l.authoring.as_ref());
    let terrain = authoring.map_or(0, |a| a.terrain.get(cell));
    if (placed || view.tiles) && !tile.is_empty() {
        tile_of(tile)
    } else if terrain == 0 {
        Shown::Empty
    } else if view.tiles {
        Shown::Keep
    } else {
        Shown::Terrain(terrain)
    }
}

/// Refuses a region of more than [`MAX_REGION_CELLS`] cells.
fn cap(cells: u64) -> Result<(), String> {
    if cells > MAX_REGION_CELLS {
        return Err(format!(
            "the region holds {cells} cells, more than {MAX_REGION_CELLS}; use a smaller one"
        ));
    }
    Ok(())
}

fn read(world: &mut World, params: &OperatorParameters) -> Result<String, String> {
    let target = target(world, params)?;
    let (a, b) = (cell(params, "x0", "y0")?, cell(params, "x1", "y1")?);
    let (width, height) = (a.0.x.abs_diff(b.0.x) + 1, a.0.y.abs_diff(b.0.y) + 1);
    cap(u64::from(width) * u64::from(height))?;
    let origin = CellCoord::new(a.0.x.min(b.0.x), a.0.y.max(b.0.y));
    let key = store_key_of(world, target.map).ok_or("the map's cells are not loaded")?;
    let tilemap = world.get::<Tilemap>(target.map);
    let view = View {
        terrain_layer: (tilemap.and_then(|t| t.layers.get(target.layer)))
            .is_some_and(|l| l.kind == TilemapLayerKind::Terrain),
        tiles: params.as_bool("tiles").unwrap_or(false),
        bounds: tilemap.and_then(|t| t.bounds),
    };
    let store = world.resource::<TilemapDataStore>();
    if view.tiles && view.terrain_layer && store.resolved_with(&key, target.layer).is_none() {
        refresh_terrain(world);
    }
    let store = world.resource::<TilemapDataStore>();
    let layer = (store.get(&key)).and_then(|data| data.layers.get(target.layer));
    let values: Vec<Shown> = (0..height)
        .flat_map(|row| (0..width).map(move |col| (col, row)))
        .map(|(col, row)| {
            let at = CellCoord::new(origin.0.x + col as i32, origin.0.y - row as i32);
            shown(layer, at, view)
        })
        .collect();
    with_layer_tileset(world, target.map, target.layer, |tileset, _| {
        let symbols = symbols(tileset, &values, view.terrain_layer && !view.tiles)?;
        let mut legend = Legend::default();
        legend
            .insert('.', LegendEntry::Empty)
            .map_err(|e| e.to_string())?;
        let mut cells = Vec::with_capacity(values.len());
        for value in &values {
            let (symbol, entry) = (symbols.get(value)).ok_or("a cell has no symbol")?;
            if legend.get(*symbol).is_none() {
                legend
                    .insert(*symbol, entry.clone())
                    .map_err(|e| e.to_string())?;
            }
            cells.push(*symbol);
        }
        let region = Region::from_cells(origin, width, height, cells)
            .ok_or("the region leaves the cell range")?;
        let quote = |text: String| serde_json::to_string(&text).unwrap_or_default();
        let legend: Vec<String> = (legend.iter())
            .map(|(symbol, entry)| {
                format!(
                    "{}: {}",
                    quote(symbol.to_string()),
                    quote(entry.to_string())
                )
            })
            .collect();
        Ok(format!(
            "legend: {{{}}}\nrows:\n{}",
            legend.join(", "),
            emit_rows(&region)
        ))
    })
}

/// A symbol and legend entry for every value in `values`: terrains first,
/// every terrain of the tileset in id order so a terrain keeps its symbol
/// from one region to the next, then tiles in index order. A terrain the
/// tileset lacks reads as keep.
fn symbols(
    tileset: &Tileset,
    values: &[Shown],
    as_terrain: bool,
) -> Result<BTreeMap<Shown, (char, LegendEntry)>, String> {
    let present: BTreeSet<Shown> = values.iter().copied().collect();
    let mut used = BTreeSet::from(['.', '?']);
    let mut pick = |name: Option<&str>| -> Result<char, String> {
        let lead = (name.and_then(|n| n.chars().next()))
            .into_iter()
            .flat_map(|c| [c.to_ascii_lowercase(), c.to_ascii_uppercase()]);
        let own = name.into_iter().flat_map(|n| n.chars().rev());
        let symbol = (lead.chain(own).chain(SYMBOLS.chars()))
            .find(|c| SYMBOLS.contains(*c) && !used.contains(c))
            .ok_or("the region holds more terrains and tiles than there are symbols")?;
        used.insert(symbol);
        Ok(symbol)
    };
    let mut out = BTreeMap::from([
        (Shown::Empty, ('.', LegendEntry::Empty)),
        (Shown::Keep, ('?', LegendEntry::Keep)),
    ]);
    let mut terrains: Vec<_> = tileset.terrains.iter().collect();
    terrains.sort_by_key(|t| t.id);
    for terrain in terrains.into_iter().filter(|_| as_terrain) {
        let symbol = pick(Some(&terrain.name))?;
        let entry = LegendEntry::Terrain(terrain.name.clone());
        out.insert(Shown::Terrain(terrain.id), (symbol, entry));
    }
    for value in present {
        match value {
            Shown::Tile(index, orientation) => {
                let (name, named) = tile_name(tileset, index, orientation);
                let symbol = pick(named.then_some(name.as_str()))?;
                out.insert(value, (symbol, LegendEntry::Tile(name)));
            }
            Shown::Terrain(_) if !out.contains_key(&value) => {
                out.insert(value, ('?', LegendEntry::Keep));
            }
            Shown::Terrain(_) | Shown::Empty | Shown::Keep => {}
        }
    }
    Ok(out)
}

/// A tile as `tile:` names it, and whether that is its own name: the name
/// the tileset gives it when no other tile shares it, else `sheet:tile`,
/// with `@orientation` when it is flipped or turned and that does not name
/// another tile whole.
fn tile_name(tileset: &Tileset, index: u16, orientation: u8) -> (String, bool) {
    let layer = u32::from(index);
    let named_as = |name: &str| {
        (tileset.properties.values())
            .filter(|p| p.get("name").is_some_and(|n| n == name))
            .count()
    };
    let own = (tileset.properties.get(&layer))
        .and_then(|p| p.get("name"))
        .filter(|name| named_as(name) == 1)
        .filter(|name| crate::tileset_edit::tile(tileset, name).is_err());
    let at = (tileset.tile_of_layer(layer)).map_or(layer.to_string(), |(s, t)| format!("{s}:{t}"));
    let base = own.cloned().unwrap_or_else(|| at.clone());
    if orientation == 0 {
        return (base, own.is_some());
    }
    let turned = format!("{base}@{orientation}");
    if own.is_some() && packed_tile(tileset, &turned).is_ok() {
        return (format!("{at}@{orientation}"), false);
    }
    (turned, own.is_some())
}

/// The tile `tile:` names: a tile name or `sheet:tile` whole, else with an
/// `@orientation` suffix.
fn named_tile(tileset: &Tileset, name: &str) -> Result<TileRef, String> {
    let whole = packed_tile(tileset, name);
    let Some((base, bits)) = name.rsplit_once('@').filter(|_| whole.is_err()) else {
        return whole;
    };
    let orientation = (bits.parse::<u8>().ok())
        .filter(|b| b & !TileRef::ORIENTATION_MASK == 0)
        .ok_or_else(|| format!("orientation {bits} is not one of 0 to 15"))?;
    let tile = packed_tile(tileset, base)?;
    Ok(TileRef::from_raw(tile.index(), orientation))
}

/// What writing one legend symbol does.
#[derive(Clone, Copy)]
enum Action {
    Keep,
    Erase,
    Terrain(TerrainId),
    Tile(TileRef),
}

fn action(
    tileset: &Tileset,
    terrain_layer: bool,
    layer: &str,
    entry: &LegendEntry,
) -> Result<Action, String> {
    Ok(match entry {
        LegendEntry::Keep => Action::Keep,
        LegendEntry::Empty => Action::Erase,
        LegendEntry::Terrain(name) if !terrain_layer => {
            return Err(format!(
                "layer `{layer}` is a tile layer; `{name}` is a terrain"
            ));
        }
        LegendEntry::Terrain(name) => {
            Action::Terrain(crate::tileset_edit::terrain(tileset, name).map_err(|err| {
                let names: Vec<&str> = tileset.terrains.iter().map(|t| t.name.as_str()).collect();
                format!("{err}; terrains are {}", names.join(", "))
            })?)
        }
        LegendEntry::Tile(name) => Action::Tile(named_tile(tileset, name)?),
    })
}

/// Legend pairs in the order written, duplicates kept for the kernel to
/// refuse.
struct Pairs(Vec<(String, String)>);

impl<'de> Deserialize<'de> for Pairs {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PairsVisitor;
        impl<'de> Visitor<'de> for PairsVisitor {
            type Value = Pairs;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an object of symbols to names")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Pairs, A::Error> {
                let mut pairs = Vec::new();
                while let Some(pair) = map.next_entry()? {
                    pairs.push(pair);
                }
                Ok(Pairs(pairs))
            }
        }
        deserializer.deserialize_map(PairsVisitor)
    }
}

/// Parsed region text: the legend, the line and column of each legend key,
/// and the rows.
struct RegionText {
    legend: Legend,
    keys: BTreeMap<char, (usize, usize)>,
    region: Region,
}

/// The line and column of legend key `key` in `lines`, from line `from`.
fn locate(lines: &[&str], from: usize, key: &str) -> Option<(usize, usize)> {
    let quoted = serde_json::to_string(key).ok()?;
    (lines.iter().enumerate().skip(from)).find_map(|(index, line)| {
        (line.match_indices(&quoted))
            .find(|(at, _)| line[at + quoted.len()..].trim_start().starts_with(':'))
            .map(|(at, _)| (index + 1, line[..at].chars().count() + 1))
    })
}

/// Region text whose top-left cell is `origin`, with `.` as empty and `?` as
/// keep unless the legend maps them. Errors name their line and column in
/// `text`.
fn parse_text(text: &str, origin: CellCoord) -> Result<RegionText, String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let rows_at = (lines.iter())
        .position(|line| line.trim_start().starts_with("rows:"))
        .ok_or("the text has no `rows:` line")?;
    if !lines[rows_at].trim()["rows:".len()..].trim().is_empty() {
        let line = rows_at + 1;
        return Err(format!("line {line}: rows start on the line after `rows:`"));
    }
    let rows = lines[rows_at + 1..].join("\n");
    cap(rows.chars().filter(|c| !c.is_whitespace()).count() as u64)?;
    let head = &lines[..rows_at];
    let (pairs, legend_line) = match head.iter().position(|line| !line.trim().is_empty()) {
        None => (Vec::new(), 0),
        Some(at) => {
            let rest = (head[at].trim_start().strip_prefix("legend:"))
                .ok_or_else(|| format!("line {}: expected `legend:` or `rows:`", at + 1))?;
            let skip = head[at].chars().count() - rest.chars().count();
            let json = std::iter::once(rest)
                .chain(head[at + 1..].iter().copied())
                .collect::<Vec<_>>()
                .join("\n");
            let Pairs(pairs) = serde_json::from_str(&json).map_err(|err| {
                let column = err.column() + if err.line() == 1 { skip } else { 0 };
                let message = err.to_string();
                let message = message
                    .rsplit_once(" at line ")
                    .map_or(&*message, |(m, _)| m);
                format!(
                    "line {}, column {column}: legend: {message}",
                    at + err.line()
                )
            })?;
            (pairs, at + 1)
        }
    };
    let entries = pairs.iter().map(|(k, v)| (k.as_str(), v.as_str()));
    let mut legend =
        Legend::parse_entries(entries).map_err(|err| format!("line {legend_line}: {err}"))?;
    let keys = (legend.iter())
        .filter_map(|(symbol, _)| {
            let at = locate(head, legend_line.saturating_sub(1), &symbol.to_string())?;
            Some((symbol, at))
        })
        .collect();
    for (symbol, entry) in [('.', LegendEntry::Empty), ('?', LegendEntry::Keep)] {
        if legend.get(symbol).is_none() {
            legend
                .insert(symbol, entry)
                .map_err(|err| err.to_string())?;
        }
    }
    let region = parse_rows(&rows, origin, &legend).map_err(|err| {
        let line = err.line + rows_at + 1;
        format!("line {line}, column {}: {}", err.column, err.kind)
    })?;
    Ok(RegionText {
        legend,
        keys,
        region,
    })
}

fn write(world: &mut World, params: &OperatorParameters) -> Result<String, String> {
    let target = target(world, params)?;
    let origin = cell(params, "x0", "y0")?;
    let text = (params.as_str("text"))
        .filter(|text| !text.trim().is_empty())
        .ok_or("text is required")?
        .to_string();
    let path = match params.as_str("mode").map_or("", str::trim) {
        "" | "connect" => false,
        "path" => true,
        other => return Err(format!("mode is connect or path, not `{other}`")),
    };
    let parsed = parse_text(&text, origin)?;
    let (actions, terrain_layer) =
        with_layer_tileset(world, target.map, target.layer, |tileset, terrain| {
            let actions = (parsed.legend.iter())
                .map(|(symbol, entry)| {
                    let action = action(tileset, terrain, &target.name, entry).map_err(|err| {
                        let at = parsed.keys.get(&symbol);
                        let at =
                            at.map_or(String::new(), |(l, c)| format!("line {l}, column {c}: "));
                        format!("{at}legend `{symbol}`: {err}")
                    })?;
                    Ok((symbol, action))
                })
                .collect::<Result<BTreeMap<char, Action>, String>>()?;
            Ok((actions, terrain))
        })?;
    let map_bounds = (world.get::<Tilemap>(target.map)).and_then(|t| t.bounds);
    let outside = (parsed.region.cells())
        .filter(|(_, symbol)| !matches!(actions.get(symbol), Some(Action::Keep)))
        .find(|(cell, _)| !inside(map_bounds, *cell));
    if let Some((cell, _)) = outside {
        let at = cell.0;
        return Err(format!(
            "cell {},{} is outside the map's bounds",
            at.x, at.y
        ));
    }
    let key = editable_key(world, target.map)?;
    let store = world.resource::<TilemapDataStore>();
    let layer = (store.get(&key)).and_then(|data| data.layers.get(target.layer));
    let writes = plan(layer, terrain_layer, &parsed.region, &actions, path);
    if writes.is_empty() {
        return Ok("no cell changed".into());
    }
    let mut stroke =
        TilemapStrokeBuilder::begin(world, target.map, target.layer, "Write tilemap region")?;
    let count = (writes.into_iter())
        .map(|(write, cells)| stroke.apply(world, &cells, write))
        .sum();
    if !stroke.end(world) {
        return Ok("no cell changed".into());
    }
    Ok(written(count, &target))
}

/// The writes that bring `layer` to `region`, skipping each cell that
/// already reads as its symbol: erases first, then hand-placed tiles a
/// terrain covers, terrains and tiles. A path write takes every cell of its
/// terrain, so the cells already holding it join the new ones.
fn plan(
    layer: Option<&LayerData>,
    terrain_layer: bool,
    region: &Region,
    actions: &BTreeMap<char, Action>,
    path: bool,
) -> Vec<(TileWrite, Vec<CellCoord>)> {
    let authoring = layer
        .and_then(|l| l.authoring.as_ref())
        .filter(|_| terrain_layer);
    let (mut erase, mut unplace) = (Vec::new(), Vec::new());
    let mut terrains: BTreeMap<TerrainId, Vec<CellCoord>> = BTreeMap::new();
    let mut tiles: BTreeMap<(u16, u8), Vec<CellCoord>> = BTreeMap::new();
    for (cell, symbol) in region.cells() {
        let terrain = authoring.map_or(0, |a| a.terrain.get(cell));
        let linked = authoring.is_some_and(|a| a.links.get(cell) != Links::EMPTY);
        let placed = authoring.and_then(|a| a.overrides.get(&cell).copied());
        let tile = layer.map_or(TileRef::EMPTY, |l| l.tiles.get(cell));
        let filled = if terrain_layer {
            terrain != 0 || linked || placed.is_some()
        } else {
            !tile.is_empty()
        };
        let into = |cells: &mut Vec<CellCoord>, changes: bool| {
            if changes {
                cells.push(cell);
            }
        };
        match actions.get(&symbol).copied().unwrap_or(Action::Keep) {
            Action::Keep => {}
            Action::Erase => into(&mut erase, filled),
            Action::Terrain(id) => {
                into(
                    terrains.entry(id).or_default(),
                    path || terrain != id || linked,
                );
                into(&mut unplace, placed.is_some());
            }
            Action::Tile(want) => {
                let key = (want.index(), want.orientation());
                let held = if terrain_layer { placed } else { Some(tile) };
                into(tiles.entry(key).or_default(), held != Some(want));
            }
        }
    }
    let mut writes = vec![
        (TileWrite::Erase, erase),
        (TileWrite::Tile(TileRef::EMPTY), unplace),
    ];
    for (id, cells) in terrains {
        let write = if path {
            TileWrite::Path(id)
        } else {
            TileWrite::Terrain(id)
        };
        writes.push((write, cells));
    }
    for ((index, bits), cells) in tiles {
        writes.push((TileWrite::Tile(TileRef::from_raw(index, bits)), cells));
    }
    writes.retain(|(_, cells)| !cells.is_empty());
    writes
}
