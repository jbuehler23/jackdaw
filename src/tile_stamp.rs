//! Stamps: blocks of tiles the Brush places as one, made from a rectangle of
//! palette tiles or loaded from the tileset, and saved with it.

use std::collections::HashMap;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_scene_types::Tilemap;
use jackdaw_tilemap::{CellCoord, TileRef};
use jackdaw_tileset::{StampTile, Tileset, TilesetStamp};

use crate::tile_tools::{BrushSource, TileBrush, TileEditMode, active_tileset};
use crate::tilemap_stroke::TileWrite;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<StampBrush>().add_systems(
        Update,
        drop_stale_stamp.run_if(|mode: Res<TileEditMode>, brush: Res<TileBrush>| {
            mode.is_changed() || brush.is_changed()
        }),
    );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TilemapStampSaveOp>();
}

pub(crate) fn flipped_x(tile: TileRef) -> TileRef {
    tile.with_flip_x(!tile.flip_x())
}

pub(crate) fn flipped_y(tile: TileRef) -> TileRef {
    tile.with_flip_y(!tile.flip_y())
}

/// A quarter turn clockwise: the tile is transposed, then flipped across.
pub(crate) fn rotated(tile: TileRef) -> TileRef {
    let (x, y) = (tile.flip_x(), tile.flip_y());
    (tile.with_transpose(!tile.transpose()))
        .with_flip_x(!y)
        .with_flip_y(x)
}

/// A block of tiles the Brush places as one, its top-left on the cursor
/// cell.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TileStamp {
    /// Columns and rows.
    pub size: UVec2,
    /// Each tile's column and row from the top-left, rows downward.
    pub cells: Vec<(UVec2, TileRef)>,
}

impl TileStamp {
    pub fn flip_x(&mut self) {
        let w = self.size.x.saturating_sub(1);
        for (at, tile) in &mut self.cells {
            *at = UVec2::new(w.saturating_sub(at.x), at.y);
            *tile = flipped_x(*tile);
        }
    }

    pub fn flip_y(&mut self) {
        let h = self.size.y.saturating_sub(1);
        for (at, tile) in &mut self.cells {
            *at = UVec2::new(at.x, h.saturating_sub(at.y));
            *tile = flipped_y(*tile);
        }
    }

    /// A quarter turn clockwise.
    pub fn rotate(&mut self) {
        let h = self.size.y.saturating_sub(1);
        for (at, tile) in &mut self.cells {
            *at = UVec2::new(h.saturating_sub(at.y), at.x);
            *tile = rotated(*tile);
        }
        self.size = UVec2::new(self.size.y, self.size.x);
    }

    /// The map cells and tiles of the stamp with its top-left at `anchor`;
    /// rows run down the map.
    pub fn placed(&self, anchor: CellCoord) -> impl Iterator<Item = (CellCoord, TileRef)> + '_ {
        (self.cells.iter()).map(move |&(at, tile)| {
            let step = at.as_ivec2() * IVec2::new(1, -1);
            (CellCoord(anchor.0.saturating_add(step)), tile)
        })
    }

    /// The writes placing the stamp at each of `anchors` in turn, one per
    /// tile with the cells it lands on.
    pub fn writes(&self, anchors: &[CellCoord]) -> Vec<(TileWrite, Vec<CellCoord>)> {
        let mut last: HashMap<CellCoord, TileRef> = HashMap::new();
        for &anchor in anchors {
            last.extend(self.placed(anchor));
        }
        let mut by_tile: HashMap<TileRef, Vec<CellCoord>> = HashMap::new();
        for (cell, tile) in last {
            by_tile.entry(tile).or_default().push(cell);
        }
        (by_tile.into_iter())
            .map(|(tile, mut cells)| {
                cells.sort_unstable();
                (TileWrite::Tile(tile), cells)
            })
            .collect()
    }

    /// The tiles of sheet `sheet` between tiles `a` and `b`, and those tiles
    /// as sheet and tile.
    pub(crate) fn rectangle(
        tileset: &Tileset,
        sheet: u32,
        a: u32,
        b: u32,
    ) -> Result<(Self, Vec<(u32, u32)>), String> {
        let columns = (tileset.sheets.get(sheet as usize))
            .ok_or_else(|| format!("the tileset has no sheet {sheet}"))?
            .columns;
        let block = rectangle_tiles(columns, a, b);
        let mut stamp = Self {
            size: block
                .iter()
                .fold(UVec2::ZERO, |size, (at, _)| size.max(*at + 1)),
            cells: Vec::new(),
        };
        for &(at, tile) in &block {
            let layer = (tileset.layer(sheet, tile)).and_then(|l| u16::try_from(l).ok());
            let layer = layer.ok_or_else(|| format!("sheet {sheet} has no tile {tile}"))?;
            stamp.cells.push((at, TileRef::new(layer)));
        }
        Ok((stamp, block.into_iter().map(|(_, t)| (sheet, t)).collect()))
    }

    pub(crate) fn from_saved(saved: &TilesetStamp) -> Result<Self, String> {
        let cells = (saved.cells.iter())
            .map(|c| {
                let index = u16::try_from(c.layer).map_err(|_| "a stamp tile is out of range")?;
                Ok((c.offset, TileRef::from_raw(index, c.orientation)))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            size: saved.size,
            cells,
        })
    }
}

/// The tiles of a sheet `columns` wide between tiles `a` and `b`, row by row,
/// each with its column and row from the rectangle's top-left.
pub(crate) fn rectangle_tiles(columns: u32, a: u32, b: u32) -> Vec<(UVec2, u32)> {
    let columns = columns.max(1);
    let at = |t: u32| UVec2::new(t % columns, t / columns);
    let (lo, hi) = (at(a).min(at(b)), at(a).max(at(b)));
    (lo.y..=hi.y)
        .flat_map(|row| (lo.x..=hi.x).map(move |column| UVec2::new(column, row)))
        .map(|cell| (cell - lo, cell.y * columns + cell.x))
        .collect()
}

/// The stamp the Brush places while its source is a stamp.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct StampBrush {
    pub stamp: Option<TileStamp>,
    /// The name of the saved stamp it was loaded as.
    pub name: Option<String>,
    /// The palette tiles it was made from, as sheet and tile.
    pub tiles: Vec<(u32, u32)>,
    /// The tileset its tiles come from.
    pub tileset: Option<AssetId<Tileset>>,
}

impl StampBrush {
    /// The index of the stamp in `tileset` saved under its name with its
    /// layout, if there is one.
    pub fn saved_index(&self, tileset: &Tileset) -> Option<usize> {
        let (name, stamp) = (self.name.as_ref()?, self.stamp.as_ref()?);
        (tileset.stamps.iter())
            .position(|s| &s.name == name && TileStamp::from_saved(s).ok().as_ref() == Some(stamp))
    }
}

/// Drops the stamp once the brush leaves the stamp source, or the layer in
/// tile edit mode uses another tileset.
fn drop_stale_stamp(world: &mut World) {
    let stamp = world.resource::<StampBrush>();
    if stamp.stamp.is_none() {
        return;
    }
    let from = stamp.tileset;
    let left = world.resource::<TileBrush>().source != BrushSource::Stamp;
    let moved = active_tileset(world).is_ok_and(|id| Some(id) != from);
    if left || moved {
        *world.resource_mut::<StampBrush>() = StampBrush::default();
        let mut brush = world.resource_mut::<TileBrush>();
        if brush.source == BrushSource::Stamp {
            brush.source = BrushSource::Terrain;
        }
        world.trigger(jackdaw_api_internal::operator::RefreshOperatorButtons);
    }
}

fn has_unsaved_stamp(
    mode: Res<TileEditMode>,
    brush: Res<TileBrush>,
    stamp: Res<StampBrush>,
    tilesets: Res<Assets<Tileset>>,
) -> bool {
    let saved = (stamp.tileset.and_then(|id| tilesets.get(id))).and_then(|t| stamp.saved_index(t));
    mode.map.is_some()
        && brush.source == BrushSource::Stamp
        && stamp.stamp.is_some()
        && saved.is_none()
}

/// Save the loaded stamp with the tileset.
#[operator(
    id = "tilemap.stamp.save",
    label = "Save Stamp",
    description = "Save the loaded stamp in the active layer's tileset under a name, as one \
                   undo entry.",
    allows_undo = false,
    is_available = has_unsaved_stamp,
    params(name(String, doc = "Name of the stamp; defaults to the next free Stamp N."))
)]
pub(crate) fn tilemap_stamp_save(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    match save(world, &params) {
        Ok(message) => {
            report_to_caller(world, message);
            OperatorResult::Finished
        }
        Err(err) => {
            warn_caller(world, format!("{}: {err}", TilemapStampSaveOp::ID));
            OperatorResult::Cancelled
        }
    }
}

fn save(world: &mut World, params: &OperatorParameters) -> Result<String, String> {
    if world.resource::<TileBrush>().source != BrushSource::Stamp {
        return Err("no stamp is loaded".to_string());
    }
    let mode = *world.resource::<TileEditMode>();
    let path = (mode.map.and_then(|map| world.get::<Tilemap>(map)))
        .and_then(|map| map.layers.get(mode.layer))
        .map(|layer| layer.tileset.clone())
        .ok_or("no tilemap is in tile edit mode")?;
    let brush = world.resource::<StampBrush>().clone();
    let stamp = brush.stamp.clone().ok_or("no stamp is loaded")?;
    if brush.tileset != Some(active_tileset(world)?) {
        return Err("the stamp is not from the layer's tileset".to_string());
    }
    let requested = crate::tileset_edit::text(params, "name");
    let op = TilemapStampSaveOp::ID;
    let mut name = String::new();
    let message = crate::tileset_edit::apply_edit(world, op, &path, |_, tileset| {
        if brush.saved_index(tileset).is_some() {
            return Err("the loaded stamp is saved already".to_string());
        }
        let taken = |n: &String| tileset.stamps.iter().any(|s| &s.name == n);
        name = match requested.as_str() {
            "" => (1..)
                .map(|n| format!("Stamp {n}"))
                .find(|n| !taken(n))
                .unwrap_or_default(),
            _ if taken(&requested) => {
                return Err(format!("a stamp is already named `{requested}`"));
            }
            _ => requested.clone(),
        };
        let cells = (stamp.cells.iter())
            .map(|&(offset, tile)| StampTile {
                offset,
                layer: u32::from(tile.index()),
                orientation: tile.orientation(),
            })
            .collect();
        tileset.stamps.push(TilesetStamp {
            name: name.clone(),
            size: stamp.size,
            cells,
        });
        Ok(format!("stamp `{name}` saved"))
    })?;
    world.resource_mut::<StampBrush>().name = Some(name);
    Ok(message)
}
