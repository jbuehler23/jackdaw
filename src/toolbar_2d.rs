//! The 2D viewport toolbar's tool groups: the object tools, and the tile tools
//! that take their place while tile edit mode is on.

use bevy::feathers::controls::ButtonVariant;
use bevy::prelude::*;
use jackdaw_api::prelude::Operator;
use jackdaw_api_internal::operator::RefreshOperatorButtons;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::icons::Icon;
use jackdaw_tilemap::TileRef;

use crate::layout::{toolbar_op_button, toolbar_separator};
use crate::tile_palette::{TilemapBrushFlipXOp, TilemapBrushFlipYOp, TilemapBrushRotateOp};
use crate::tile_select::{
    TilemapSelectOp, TilemapSelectionCopyOp, TilemapSelectionCutOp, TilemapSelectionEraseOp,
    TilemapStampPasteOp,
};
use crate::tile_tools::{
    BrushSource, TileBrush, TileEditMode, TileTool, TilemapBrushOp, TilemapBucketOp,
    TilemapEditModeExitOp, TilemapEditModeOp, TilemapEraserOp, TilemapLineOp, TilemapPickerOp,
    TilemapRectangleOp,
};
use crate::tool_ops::{ToolRotateOp, ToolScaleOp, ToolSelectOp, ToolTranslateOp};
use crate::toolbar_overflow::ToolbarTools;

/// The name of the 2D toolbar's display menu.
pub const CANVAS_VIEW_MENU: &str = "Canvas view";

/// Which tool group of the 2D toolbar a node is.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toolbar2dGroup {
    Object,
    Tiles,
}

/// A group's buttons in order; `None` is a separator.
type Items = &'static [Option<(&'static str, Icon)>];

const OBJECT_TOOLS: Items = &[
    Some((ToolSelectOp::ID, Icon::MousePointer)),
    Some((ToolTranslateOp::ID, Icon::Move)),
    Some((ToolRotateOp::ID, Icon::RotateCw)),
    Some((ToolScaleOp::ID, Icon::Scale3d)),
    None,
    Some((TilemapEditModeOp::ID, Icon::Grid2x2)),
];

const TILE_TOOLS: Items = &[
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
    Some((TilemapBrushFlipXOp::ID, Icon::FlipHorizontal2)),
    Some((TilemapBrushFlipYOp::ID, Icon::FlipVertical2)),
    Some((TilemapBrushRotateOp::ID, Icon::RotateCwSquare)),
    None,
    Some((TilemapSelectionCopyOp::ID, Icon::Copy)),
    Some((TilemapSelectionCutOp::ID, Icon::Scissors)),
    Some((TilemapStampPasteOp::ID, Icon::ClipboardPaste)),
    Some((TilemapSelectionEraseOp::ID, Icon::Trash2)),
];

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(sync_2d_toolbar);
}

/// Put the object and tile tool groups at the start of `toolbar`, each with
/// the overflow menu the 3D toolbar uses.
pub(crate) fn spawn_tool_groups(world: &mut World, toolbar: Entity) -> Result {
    let mut groups = Vec::new();
    for (which, items) in [
        (Toolbar2dGroup::Object, OBJECT_TOOLS),
        (Toolbar2dGroup::Tiles, TILE_TOOLS),
    ] {
        let group = world
            .spawn((
                crate::EditorEntity,
                which,
                ToolbarTools,
                Node {
                    display: if which == Toolbar2dGroup::Object {
                        Display::Flex
                    } else {
                        Display::None
                    },
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(jackdaw_feathers::tokens::TOOLBAR_GAP),
                    flex_shrink: 1.0,
                    min_width: px(0.0),
                    overflow: Overflow::clip_x(),
                    ..default()
                },
            ))
            .id();
        for item in items {
            let mut child = match item {
                Some((id, icon)) => world.spawn_scene(toolbar_op_button(id, *icon))?,
                None => world.spawn_scene(toolbar_separator())?,
            };
            child.insert((crate::EditorEntity, ChildOf(group)));
        }
        groups.push(group);
    }
    world.entity_mut(toolbar).insert_children(0, &groups);
    crate::toolbar_overflow::spawn_toolbar_overflow(world, toolbar);
    Ok(())
}

/// Whether the tile button calling `id` shows as on, or `None` for a button
/// that is not one of the tile tools.
fn tile_button_active(id: &str, mode: &TileEditMode, brush: &TileBrush) -> Option<bool> {
    let tool = match id {
        TilemapBrushOp::ID => TileTool::Brush,
        TilemapEraserOp::ID => TileTool::Eraser,
        TilemapRectangleOp::ID => TileTool::Rectangle,
        TilemapLineOp::ID => TileTool::Line,
        TilemapBucketOp::ID => TileTool::Bucket,
        TilemapPickerOp::ID => TileTool::Picker,
        TilemapSelectOp::ID => TileTool::Select,
        _ => {
            let tile = (brush.source != BrushSource::Stamp).then_some(brush.tile);
            return match id {
                TilemapBrushFlipXOp::ID => Some(tile.is_some_and(TileRef::flip_x)),
                TilemapBrushFlipYOp::ID => Some(tile.is_some_and(TileRef::flip_y)),
                TilemapBrushRotateOp::ID => Some(tile.is_some_and(TileRef::transpose)),
                _ => None,
            };
        }
    };
    Some(mode.tool == tool)
}

/// Show the tile group while tile edit mode is on and the object group
/// otherwise, and light the active tile tool and the brush's flips.
fn sync_2d_toolbar(
    _: On<RefreshOperatorButtons>,
    mode: Option<Res<TileEditMode>>,
    brush: Option<Res<TileBrush>>,
    mut groups: Query<(&Toolbar2dGroup, &mut Node)>,
    mut buttons: Query<(&ButtonOperatorCall, &mut ButtonVariant)>,
) {
    let (Some(mode), Some(brush)) = (mode, brush) else {
        return;
    };
    let editing = mode.map.is_some();
    for (group, mut node) in &mut groups {
        let display = if (*group == Toolbar2dGroup::Tiles) == editing {
            Display::Flex
        } else {
            Display::None
        };
        if node.display != display {
            node.display = display;
        }
    }
    for (call, mut variant) in &mut buttons {
        let Some(active) = tile_button_active(call.id.as_ref(), &mode, &brush) else {
            continue;
        };
        let target = if active {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Plain
        };
        if *variant != target {
            *variant = target;
        }
    }
}
