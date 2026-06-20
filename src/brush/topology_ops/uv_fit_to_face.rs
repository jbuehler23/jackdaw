//! `brush.face.uv.fit_to_face` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::BrushSelection;
use crate::brush::EditMode;
use crate::brush::topology_ops::uv_common::{can_run, for_each_selected_face};

/// Set `uv_scale` so the face spans 0..1 in UV space. The texture covers the
/// face exactly once.
#[operator(
    id = "brush.face.uv.fit_to_face",
    label = "Fit UV to Face",
    is_available = can_run,
    allows_undo = true
)]
pub(crate) fn brush_uv_fit_to_face(
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
            let ring: Vec<Vec3> = topology
                .face_ring(face_idx)
                .map(|i| topology.vertices[i as usize].position)
                .collect();
            let axes = jackdaw_uv::UvAxes::new(face.uv_u_axis, face.uv_v_axis);
            if let Some(fit) = jackdaw_uv::fit_to_face(&ring, axes) {
                face.uv_scale = fit.scale;
                face.uv_offset = fit.offset;
            }
        },
    )
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvFitToFaceOp>();
}
