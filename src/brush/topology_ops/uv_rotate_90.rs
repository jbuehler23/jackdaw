//! `brush.face.uv.rotate_90` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::BrushSelection;
use crate::brush::EditMode;
use crate::brush::topology_ops::uv_common::{can_run, for_each_selected_face};

/// Rotate the U and V axes 90 degrees counter-clockwise on each selected face.
#[operator(
    id = "brush.face.uv.rotate_90",
    label = "Rotate UV 90 deg",
    is_available = can_run,
    allows_undo = true
)]
pub(crate) fn brush_uv_rotate_90(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
) -> OperatorResult {
    for_each_selected_face(
        &edit_mode,
        &selection,
        &mut brushes,
        |_face_idx, _topology, face| {
            let rotated =
                jackdaw_uv::rotate_90(jackdaw_uv::UvAxes::new(face.uv_u_axis, face.uv_v_axis));
            face.uv_u_axis = rotated.u;
            face.uv_v_axis = rotated.v;
        },
    )
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvRotate90Op>();
}
