//! `brush.mesh.dissolve_edges` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::dissolve_edges::dissolve_edges;
use jackdaw_geometry::halfedge::{EdgeKey, HalfedgeMesh, VertKey, apply_topology_edit};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Remove the selected edges and merge each pair of adjacent faces into one.
/// Boundary or non-manifold edges are skipped silently. Available in Edge mode.
#[operator(
    id = "brush.mesh.dissolve_edges",
    label = "Dissolve Edges",
    is_available = can_run_dissolve_edges,
    allows_undo = true
)]
pub(crate) fn brush_dissolve_edges(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.active_brush?;
    let sel_edges: Vec<(usize, usize)> = selection
        .sub(brush_entity)
        .map(|s| s.edges.clone())
        .unwrap_or_default();
    if sel_edges.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Map each selected cache-edge (a, b) to a HalfedgeMesh EdgeKey via vert_keys.
    let mut halfedge = halfedge_q.get_mut(brush_entity)?;
    let mut mesh_edges: Vec<EdgeKey> = Vec::with_capacity(sel_edges.len());
    for &(a, b) in &sel_edges {
        let Some(&va) = halfedge.vert_keys.get(a) else {
            continue;
        };
        let Some(&vb) = halfedge.vert_keys.get(b) else {
            continue;
        };
        if let Some(ek) = find_edge_between(&halfedge.mesh, va, vb) {
            mesh_edges.push(ek);
        }
    }
    if mesh_edges.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Apply the dissolve and reconcile the brush's faces, topology, and binding.
    // `into_inner` reborrows the change-detected `Mut<Brush>` as `&mut Brush` so
    // the two fields can be borrowed disjointly.
    let brush = brushes.get_mut(brush_entity)?.into_inner();
    apply_topology_edit(
        &mut brush.faces,
        &mut brush.topology,
        &mut halfedge.0,
        |mesh| dissolve_edges(mesh, &mesh_edges),
    )?;

    OperatorResult::Finished
}

fn find_edge_between(mesh: &HalfedgeMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    mesh.edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

pub(crate) fn can_run_dissolve_edges(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge)
        && selection.active_sub().is_some_and(|s| !s.edges.is_empty())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushDissolveEdgesOp>();
    // No keybind; operator is available via menu / command palette only for MVP.
}
