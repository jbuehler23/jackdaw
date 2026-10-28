use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{report_to_caller, warn_caller};
use jackdaw_tilemap::{LayoutTemplate, RgbaImage, reset_to_auto};
use jackdaw_tileset::{Tileset, TilesetObject};

use crate::asset_index::assets_dir;
use crate::tileset_edit::{
    open_tileset, queue_edit, setup_tiles, sheet, terrain, terrain_or_empty, text, tile_list,
    with_setup,
};
use crate::tileset_import::decode_sheet;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TilesetApplyTemplateOp>();
    ctx.register_operator::<TilesetTileResetToAutoOp>();
    ctx.register_operator::<TilesetPaintPropertyOp>();
    ctx.register_operator::<TilesetObjectAddOp>();
    ctx.register_operator::<TilesetObjectRemoveOp>();
    ctx.register_operator::<TilesetDescribeOp>();
}

/// A sheet, defaulting to the first one under terrain setup.
fn sheet_or_first(tileset: &Tileset, text: &str) -> Result<u32, String> {
    if text.is_empty() {
        return (tileset.setup_sheets.first().copied())
            .ok_or_else(|| "no sheet is under terrain setup".into());
    }
    sheet(tileset, text)
}

fn pair(text: &str, what: &str) -> Result<[f32; 2], String> {
    let parts: Vec<f32> = (text.split(',').map(|p| p.trim().parse::<f32>()))
        .collect::<Result<_, _>>()
        .map_err(|_| format!("{what} is two numbers like 2,3, not `{text}`"))?;
    <[f32; 2]>::try_from(parts).map_err(|_| format!("{what} is two numbers like 2,3, not `{text}`"))
}

fn cell(text: &str, what: &str) -> Result<UVec2, String> {
    let [x, y] = pair(text, what)?;
    if x < 0.0 || y < 0.0 || x.fract() != 0.0 || y.fract() != 0.0 {
        return Err(format!("{what} is two whole numbers, not `{text}`"));
    }
    Ok(UVec2::new(x as u32, y as u32))
}

fn layout_template(text: &str) -> Result<LayoutTemplate, String> {
    match text {
        "corner16" => Ok(LayoutTemplate::Corner16),
        "side16" => Ok(LayoutTemplate::Side16),
        "minimal3x3" => Ok(LayoutTemplate::Minimal3x3),
        "corner2x2" => Ok(LayoutTemplate::Corner2x2),
        "blob47" => Ok(LayoutTemplate::Blob47),
        other => Err(format!(
            "template is corner16, side16, minimal3x3, corner2x2 or blob47, not `{other}`"
        )),
    }
}

/// Lay a layout template's bits onto a block of tiles.
#[operator(
    id = "tileset.apply_template",
    label = "Apply Tileset Template",
    description = "Set the bits of a block of tiles to a common layout of an inner terrain over \
                   an outer one, adding their terrain set when missing. corner16 and side16 are \
                   4x4, minimal3x3 3x3, corner2x2 2x2 and blob47 rows of 8.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        inner(String, doc = "Inner terrain id or name."),
        outer(String, doc = "Outer terrain id or name, or empty."),
        template(String, doc = "corner16, side16, minimal3x3, corner2x2 or blob47."),
        origin(
            String,
            doc = "Column and row of the block's top-left tile, as col,row."
        ),
        sheet(
            String,
            doc = "Sheet index or source path; defaults to the first setup sheet."
        ),
    )
)]
pub fn tileset_apply_template(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (inner, outer) = (text(&params, "inner"), text(&params, "outer"));
    let (template, origin, on) = (
        text(&params, "template"),
        text(&params, "origin"),
        text(&params, "sheet"),
    );
    queue_edit(
        &mut commands,
        "tileset.apply_template",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let (inner, outer) = (
                    terrain(tileset, &inner)?,
                    terrain_or_empty(tileset, &outer)?,
                );
                let template = layout_template(&template)?;
                let origin = cell(&origin, "origin")?;
                let on = sheet_or_first(tileset, &on)?;
                let index = tileset.setup_sheet(on).map_err(|e| e.to_string())?;
                let source = &tileset.sheets[on as usize];
                let grid = UVec2::new(source.columns, source.rows);
                let tiles = (setup.apply_template(template, inner, outer, index, grid, origin))
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "{template:?} laid on {} tiles of sheet {on}",
                    tiles.len()
                ))
            })
        },
    )
}

/// Hand tiles back to auto setup.
#[operator(
    id = "tileset.tile.reset_to_auto",
    label = "Reset Tiles To Auto",
    description = "Hand tiles back to auto setup: their bits and shapes are proposed afresh \
                   from the source sheets.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        tiles(
            String,
            doc = "Comma-separated tiles, each a packed layer or sheet:tile."
        ),
    )
)]
pub fn tileset_tile_reset_to_auto(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let list = text(&params, "tiles");
    queue_edit(
        &mut commands,
        "tileset.tile.reset_to_auto",
        text(&params, "path"),
        move |world, tileset| {
            let tiles = setup_tiles(tileset, &list)?;
            let assets = assets_dir(world).ok_or("no project is open")?;
            let setup = tileset.setup().map_err(|e| e.to_string())?;
            // Every setup sheet is decoded: a re-run drops the values of any
            // sheet missing from its inputs, and terrains are found across all.
            let sheets = (setup.sheets.iter())
                .map(|name| decode_sheet(&assets, name.clone()))
                .collect::<Result<Vec<_>, _>>()?;
            let inputs = (sheets.iter())
                .map(|sheet| Ok((sheet.name.as_str(), sheet.image()?)))
                .collect::<Result<Vec<(&str, RgbaImage)>, String>>()?;
            let reset = reset_to_auto(&setup, &inputs, &tiles).map_err(|e| e.to_string())?;
            tileset.set_setup(&reset).map_err(|e| e.to_string())?;
            Ok(format!("{} tiles handed back to auto setup", tiles.len()))
        },
    )
}

/// Set or clear a named value on tiles.
#[operator(
    id = "tileset.paint_property",
    label = "Paint Tile Property",
    description = "Set a named value on tiles, or clear it with an empty value.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        tiles(
            String,
            doc = "Comma-separated tiles, each a packed layer or sheet:tile."
        ),
        key(String, doc = "Property name."),
        value(String, doc = "Property value; empty removes the property."),
    )
)]
pub fn tileset_paint_property(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (list, key, value) = (
        text(&params, "tiles"),
        text(&params, "key"),
        params.as_str("value").unwrap_or_default().to_string(),
    );
    queue_edit(
        &mut commands,
        "tileset.paint_property",
        text(&params, "path"),
        move |_, tileset| {
            if key.is_empty() {
                return Err("no key given".into());
            }
            let layers: Vec<u32> = (tile_list(tileset, &list)?.into_iter())
                .filter_map(|(sheet, tile)| tileset.layer(sheet, tile))
                .collect();
            let changed = tileset.paint_property(&layers, &key, &value);
            Ok(format!("{key} changed on {changed} tiles"))
        },
    )
}

/// Mark a block of tiles as one object.
#[operator(
    id = "tileset.object.add",
    label = "Add Tileset Object",
    description = "Mark a block of tiles as one object placed as a sprite, in place of any \
                   object sharing a tile with it.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        sheet(
            String,
            doc = "Sheet index or source path; defaults to the first setup sheet."
        ),
        origin(
            String,
            doc = "Column and row of the block's top-left tile, as col,row."
        ),
        size(String, doc = "Width and height in tiles, as w,h."),
        pivot(
            String,
            default = "0.5,0",
            doc = "Placement point as a fraction of the block \
                                                from its bottom-left, y up, as x,y."
        ),
    )
)]
pub fn tileset_object_add(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (on, origin, size, pivot) = (
        text(&params, "sheet"),
        text(&params, "origin"),
        text(&params, "size"),
        text(&params, "pivot"),
    );
    queue_edit(
        &mut commands,
        "tileset.object.add",
        text(&params, "path"),
        move |_, tileset| {
            let on = sheet_or_first(tileset, &on)?;
            let (origin, size) = (cell(&origin, "origin")?, cell(&size, "size")?);
            let pivot = if pivot.is_empty() {
                TilesetObject::BOTTOM_CENTRE
            } else {
                Vec2::from(pair(&pivot, "pivot")?)
            };
            if !(pivot.is_finite() && pivot.cmpge(Vec2::ZERO).all() && pivot.cmple(Vec2::ONE).all())
            {
                return Err(format!("pivot is two fractions from 0 to 1, not `{pivot}`"));
            }
            with_setup(tileset, |tileset, setup| {
                let index = tileset.setup_sheet(on).map_err(|e| e.to_string())?;
                let source = &tileset.sheets[on as usize];
                let grid = UVec2::new(source.columns, source.rows);
                setup
                    .put_block(index, grid, origin, size)
                    .map_err(|e| e.to_string())?;
                Ok(String::new())
            })?;
            let object = (tileset.objects.iter_mut())
                .find(|o| o.sheet == on && o.origin == origin && o.size == size)
                .ok_or("the object was not kept")?;
            object.pivot = pivot;
            Ok(format!(
                "object of {}x{} tiles at {},{} on sheet {on}",
                size.x, size.y, origin.x, origin.y
            ))
        },
    )
}

/// Unmark an object.
#[operator(
    id = "tileset.object.remove",
    label = "Remove Tileset Object",
    description = "Remove the object covering a tile; its tiles stay in the tileset.",
    allows_undo = false,
    params(
        path(
            String,
            doc = "Tileset file under the project's assets; defaults to the open tileset."
        ),
        sheet(
            String,
            doc = "Sheet index or source path; defaults to the first setup sheet."
        ),
        origin(String, doc = "Column and row of any tile of the object, as col,row."),
    )
)]
pub fn tileset_object_remove(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let (on, at) = (text(&params, "sheet"), text(&params, "origin"));
    queue_edit(
        &mut commands,
        "tileset.object.remove",
        text(&params, "path"),
        move |_, tileset| {
            with_setup(tileset, |tileset, setup| {
                let on = sheet_or_first(tileset, &on)?;
                let index = tileset.setup_sheet(on).map_err(|e| e.to_string())?;
                let removed = (setup.remove_stamp_at(index, cell(&at, "origin")?))
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "object at {},{} on sheet {on} removed",
                    removed.origin.x, removed.origin.y
                ))
            })
        },
    )
}

/// Tell the caller what a tileset holds.
#[operator(
    id = "tileset.describe",
    label = "Describe Tileset",
    description = "Report a tileset as text: terrains, terrain sets with their coverage and \
                   missing shapes, flagged tiles, objects, properties and a grid of each sheet.",
    allows_undo = false,
    params(path(
        String,
        doc = "Tileset file under the project's assets; defaults to the open tileset."
    ))
)]
pub fn tileset_describe(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let path = text(&params, "path");
    commands.queue(move |world: &mut World| {
        let described = open_tileset(world, &path).and_then(|(path, tileset)| {
            let text = tileset.describe().map_err(|e| e.to_string())?;
            Ok(format!("tileset \"{}\"\n{text}", path.display()))
        });
        match described {
            Ok(text) => report_to_caller(world, text),
            Err(err) => warn_caller(world, format!("tileset.describe: {err}")),
        }
    });
    OperatorResult::Finished
}
