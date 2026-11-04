//! The tile tools' group of the 2D toolbar, shown in place of the object
//! tools while tile edit mode is on.

use bevy::prelude::*;
use jackdaw_api::prelude::Operator;
use jackdaw_editor_core::toolbar_2d::{Toolbar2dGroup, Toolbar2dItems, Toolbar2dSlot};
use jackdaw_feathers::icons::Icon;
use jackdaw_tilemap::TileRef;

use crate::tile_palette::{
    BRUSH_TURNS, TilemapBrushFlipXOp, TilemapBrushFlipYOp, TilemapBrushRotateOp,
};
use crate::tile_select::{
    TilemapSelectOp, TilemapSelectionCopyOp, TilemapSelectionCutOp, TilemapSelectionEraseOp,
    TilemapStampPasteOp,
};
use crate::tile_tools::{
    BrushSource, TileBrush, TileEditMode, TileTool, TilemapBrushOp, TilemapBucketOp,
    TilemapEditModeExitOp, TilemapEditModeOp, TilemapEraserOp, TilemapLineOp, TilemapPickerOp,
    TilemapRectangleOp,
};

/// The id of the tile tools' group.
pub const TILE_TOOLS_GROUP: &str = "tiles";

/// The tile tools' group, as the 2D toolbar marks its node.
pub const TILE_TOOLBAR_GROUP: Toolbar2dGroup = Toolbar2dGroup::Mode(TILE_TOOLS_GROUP);

const TILE_TOOLS: Toolbar2dItems = &[
    Some((TilemapEditModeExitOp::ID, Icon::Check)),
    None,
    Some((TilemapBrushOp::ID, Icon::Paintbrush)),
    Some((TilemapEraserOp::ID, Icon::Eraser)),
    Some((TilemapRectangleOp::ID, Icon::Square)),
    Some((TilemapLineOp::ID, Icon::Slash)),
    Some((TilemapBucketOp::ID, Icon::PaintBucket)),
    Some((TilemapPickerOp::ID, Icon::Pipette)),
    Some((TilemapSelectOp::ID, Icon::SquareDashed)),
    None,
    Some(BRUSH_TURNS[0]),
    Some(BRUSH_TURNS[1]),
    Some(BRUSH_TURNS[2]),
    None,
    Some((TilemapSelectionCopyOp::ID, Icon::Copy)),
    Some((TilemapSelectionCutOp::ID, Icon::Scissors)),
    Some((TilemapStampPasteOp::ID, Icon::ClipboardPaste)),
    Some((TilemapSelectionEraseOp::ID, Icon::Trash2)),
];

pub(crate) const SLOT: Toolbar2dSlot = Toolbar2dSlot {
    id: TILE_TOOLS_GROUP,
    entry: Some((TilemapEditModeOp::ID, Icon::Grid2x2)),
    items: TILE_TOOLS,
    active: editing,
    lit: tile_button_lit,
};

fn editing(world: &World) -> bool {
    world
        .get_resource::<TileEditMode>()
        .is_some_and(|mode| mode.map.is_some())
}

/// Whether the tile button calling `id` shows as on, or `None` for a button
/// that is not one of the tile tools.
fn tile_button_lit(world: &World, id: &str) -> Option<bool> {
    let mode = world.get_resource::<TileEditMode>()?;
    let brush = world.get_resource::<TileBrush>()?;
    let tool = match id {
        TilemapBrushOp::ID => TileTool::Brush,
        TilemapEraserOp::ID => TileTool::Eraser,
        TilemapRectangleOp::ID => TileTool::Rectangle,
        TilemapLineOp::ID => TileTool::Line,
        TilemapBucketOp::ID => TileTool::Bucket,
        TilemapPickerOp::ID => TileTool::Picker,
        TilemapSelectOp::ID => TileTool::Select,
        _ => {
            let [flip_x, flip_y, rotate] = if brush.source == BrushSource::Stamp {
                [false; 3]
            } else {
                turns_lit(brush.tile)
            };
            return match id {
                TilemapBrushFlipXOp::ID => Some(flip_x),
                TilemapBrushFlipYOp::ID => Some(flip_y),
                TilemapBrushRotateOp::ID => Some(rotate),
                _ => None,
            };
        }
    };
    Some(mode.tool == tool)
}

/// Which of Flip X, Flip Y and Rotate show as on for a tile's orientation.
/// A pure flip lights its flip; any quarter or half turn lights Rotate; a
/// quarter turn that is also mirrored lights Rotate and Flip X.
fn turns_lit(tile: TileRef) -> [bool; 3] {
    let (flip_x, flip_y) = (tile.flip_x(), tile.flip_y());
    if !tile.transpose() {
        if flip_x == flip_y {
            [false, false, flip_x]
        } else {
            [flip_x, flip_y, false]
        }
    } else {
        [flip_x == flip_y, false, true]
    }
}
