//! `brush.face.uv.world_aligned` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::BrushSelection;
use crate::brush::EditMode;
use crate::brush::topology_ops::uv_common::{can_run, for_each_selected_face};

/// Snap U and V axes to the closest world-axis pair for the face's normal.
/// Useful for grid-aligned brushwork where adjacent brushes with the same
/// texture tile continuously across edges.
#[operator(
    id = "brush.face.uv.world_aligned",
    label = "World-Align UVs",
    is_available = can_run,
    allows_undo = true
)]
pub(crate) fn brush_uv_world_aligned(
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
            let axes = jackdaw_uv::world_aligned(normal);
            face.uv_u_axis = axes.u;
            face.uv_v_axis = axes.v;
        },
    )
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvWorldAlignedOp>();
}
