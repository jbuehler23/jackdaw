//! `brush.face.uv.reset_axes` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;

/// Recompute the U and V axes on each selected face from the face normal.
/// Resets uv_offset and uv_rotation. Keeps uv_scale unchanged.
#[operator(
    id = "brush.face.uv.reset_axes",
    label = "Reset UV Axes",
    is_available = can_run_uv_face_op,
    allows_undo = true
)]
pub(crate) fn brush_uv_reset_axes(
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
        let normal = brush.topology.face_normal(face_idx);
        let (u, v) = jackdaw_geometry::compute_face_tangent_axes(normal);
        let face = &mut brush.faces[face_idx];
        face.uv_u_axis = u;
        face.uv_v_axis = v;
        face.uv_offset = Vec2::ZERO;
        face.uv_rotation = 0.0;
    }

    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old: brush_before,
        new: brush.clone(),
        label: "Reset UV Axes".to_string(),
    }));
    OperatorResult::Finished
}

pub(crate) fn can_run_uv_face_op(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !selection.faces.is_empty()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvResetAxesOp>();
}
