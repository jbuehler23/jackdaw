//! `brush.mesh.vertex_slide` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::vertex_slide::vertex_slide;
use jackdaw_geometry::halfedge::{VertKey, apply_topology_edit};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

const DEFAULT_SLIDE_T: f32 = 0.5;

/// Slide each selected vertex along its first incident edge by a fixed parameter (default 0.5).
/// Pure transform; topology unchanged. Available in Vertex mode.
#[operator(
    id = "brush.mesh.vertex_slide",
    label = "Vertex Slide",
    is_available = can_run_vertex_slide,
    allows_undo = true
)]
pub(crate) fn brush_vertex_slide(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Vertex) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.active_brush?;
    let sel_verts: Vec<usize> = selection
        .sub(brush_entity)
        .map(|s| s.vertices.clone())
        .unwrap_or_default();
    if sel_verts.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Map cache vertex indices to HalfedgeMesh VertKeys via vert_keys parallel array.
    let mut halfedge = halfedge_q.get_mut(brush_entity)?;
    let mut vert_keys: Vec<VertKey> = Vec::with_capacity(sel_verts.len());
    for &vert_idx in &sel_verts {
        if let Some(&vk) = halfedge.vert_keys.get(vert_idx) {
            vert_keys.push(vk);
        }
    }
    if vert_keys.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Slide the verts and reconcile the brush's faces, topology, and binding.
    // `into_inner` reborrows the change-detected `Mut<Brush>` as `&mut Brush` so
    // the two fields can be borrowed disjointly.
    let brush = brushes.get_mut(brush_entity)?.into_inner();
    apply_topology_edit(
        &mut brush.faces,
        &mut brush.topology,
        &mut halfedge.0,
        |mesh| {
            let _ = vertex_slide(mesh, &vert_keys, DEFAULT_SLIDE_T);
        },
    );

    OperatorResult::Finished
}

pub(crate) fn can_run_vertex_slide(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex)
        && selection
            .active_sub()
            .is_some_and(|s| !s.vertices.is_empty())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushVertexSlideOp>();
    // No keybind; operator is available via menu / command palette only for MVP.
}
