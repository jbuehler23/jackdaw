//! `brush.face.uv.reset_axes` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushSelection, EditMode};

/// Recompute the U and V axes on each selected face from the face normal.
/// Resets `uv_offset` and `uv_rotation`. Keeps `uv_scale` unchanged.
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
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Face) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.active_brush?;
    let sel_faces: Vec<usize> = selection
        .sub(brush_entity)
        .map(|s| s.faces.clone())
        .unwrap_or_default();
    if sel_faces.is_empty() {
        return OperatorResult::Cancelled;
    }
    let mut brush = brushes.get_mut(brush_entity)?;

    for &face_idx in &sel_faces {
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

    OperatorResult::Finished
}

pub(crate) fn can_run_uv_face_op(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face)
        && selection.active_sub().is_some_and(|s| !s.faces.is_empty())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvResetAxesOp>();
}
