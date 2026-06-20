//! `brush.face.uv.reset_axes` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::BrushSelection;
use crate::brush::EditMode;
use crate::brush::topology_ops::uv_common::{can_run, for_each_selected_face};

/// Recompute the U and V axes on each selected face from the face normal.
/// Resets `uv_offset` and `uv_rotation`. Keeps `uv_scale` unchanged.
#[operator(
    id = "brush.face.uv.reset_axes",
    label = "Reset UV Axes",
    is_available = can_run,
    allows_undo = true
)]
pub(crate) fn brush_uv_reset_axes(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
) -> OperatorResult {
    for_each_selected_face(
        &edit_mode,
        &selection,
        &mut brushes,
        |face_idx, topology, face| {
            let normal = topology.face_normal(face_idx);
            let reset = jackdaw_uv::reset_axes(normal);
            face.uv_u_axis = reset.axes.u;
            face.uv_v_axis = reset.axes.v;
            face.uv_offset = reset.offset;
            face.uv_rotation = reset.rotation;
        },
    )
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvResetAxesOp>();
}
