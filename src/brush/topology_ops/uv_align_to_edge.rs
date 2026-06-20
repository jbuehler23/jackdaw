//! `brush.face.uv.align_to_edge` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::BrushSelection;
use crate::brush::EditMode;
use crate::brush::topology_ops::uv_common::{can_run, for_each_selected_face};

/// Rotate UV axes so the U direction aligns with a selected edge of the face.
/// Useful for getting a texture's grain to follow a particular feature edge.
/// Prefers a selected edge that belongs to this face; falls back to the face's
/// first ring edge if none is selected.
#[operator(
    id = "brush.face.uv.align_to_edge",
    label = "Align UV to Edge",
    is_available = can_run,
    allows_undo = true
)]
pub(crate) fn brush_uv_align_to_edge(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
) -> OperatorResult {
    let selected_edges: Vec<(usize, usize)> = selection
        .active_sub()
        .map(|s| s.edges.clone())
        .unwrap_or_default();

    for_each_selected_face(
        &edit_mode,
        &selection,
        &mut brushes,
        |face_idx, topology, face| {
            let normal = topology.face_normal(face_idx);
            let ring: Vec<u32> = topology.face_ring(face_idx).collect();
            if ring.len() < 2 {
                return;
            }

            // Find a selected edge whose both endpoints are in this face's ring.
            let mut target_edge: Option<(usize, usize)> = None;
            for &(a, b) in &selected_edges {
                if ring.contains(&(a as u32)) && ring.contains(&(b as u32)) {
                    target_edge = Some((a, b));
                    break;
                }
            }
            let (a_idx, b_idx) =
                target_edge.unwrap_or_else(|| (ring[0] as usize, ring[1] as usize));

            let a_pos = topology.vertices[a_idx].position;
            let b_pos = topology.vertices[b_idx].position;

            if let Some(axes) = jackdaw_uv::align_to_edge(normal, a_pos, b_pos) {
                face.uv_u_axis = axes.u;
                face.uv_v_axis = axes.v;
            }
        },
    )
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvAlignToEdgeOp>();
}
