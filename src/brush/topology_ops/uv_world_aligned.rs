//! `brush.face.uv.world_aligned` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;

/// Snap U and V axes to the closest world-axis pair for the face's normal.
/// Useful for Hammer / Quake-style brushwork where adjacent brushes with the
/// same texture tile continuously across edges.
#[operator(
    id = "brush.face.uv.world_aligned",
    label = "World-Align UVs",
    is_available = can_run_uv_world_aligned,
    allows_undo = true
)]
pub(crate) fn brush_uv_world_aligned(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Face) {
        return OperatorResult::Cancelled;
    }
    let Some(brush_entity) = selection.entity else {
        return OperatorResult::Cancelled;
    };
    if selection.faces.is_empty() {
        return OperatorResult::Cancelled;
    }
    let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
        return OperatorResult::Cancelled;
    };
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    for &face_idx in &selection.faces {
        if face_idx >= brush.faces.len() {
            continue;
        }
        let n = brush.topology.face_normal(face_idx);
        let abs = n.abs();
        let (u, v) = if abs.x >= abs.y && abs.x >= abs.z {
            // Normal mostly along X: U = +/-Y, V = Z
            if n.x >= 0.0 {
                (Vec3::Y, Vec3::Z)
            } else {
                (Vec3::NEG_Y, Vec3::Z)
            }
        } else if abs.y >= abs.x && abs.y >= abs.z {
            // Normal mostly along Y: U = +/-X (negated for consistent winding), V = Z
            if n.y >= 0.0 {
                (Vec3::NEG_X, Vec3::Z)
            } else {
                (Vec3::X, Vec3::Z)
            }
        } else {
            // Normal mostly along Z: U = X, V = +/-Y
            if n.z >= 0.0 {
                (Vec3::X, Vec3::Y)
            } else {
                (Vec3::X, Vec3::NEG_Y)
            }
        };
        let face = &mut brush.faces[face_idx];
        face.uv_u_axis = u;
        face.uv_v_axis = v;
    }

    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old: brush_before,
        new: brush.clone(),
        label: "World-Align UVs".to_string(),
    }));
    OperatorResult::Finished
}

pub(crate) fn can_run_uv_world_aligned(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !selection.faces.is_empty()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvWorldAlignedOp>();
}
