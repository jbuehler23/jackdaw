//! `brush.select.ring` operator. Expands the edge selection by walking the
//! perpendicular-edge ring around each selected edge through quad faces.

use std::collections::HashSet;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::select::ring_walk::ring_walk;
use jackdaw_geometry::halfedge::{EdgeKey, HalfedgeMesh, VertKey};

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Expand the edge selection by walking the perpendicular-edge ring around each
/// selected edge through quad faces. Stops at non-quad faces or boundary
/// edges. Available in Edge mode with at least one edge selected.
#[operator(
    id = "brush.select.ring",
    label = "Ring Select",
    is_available = can_run_select_ring,
    allows_undo = false
)]
pub(crate) fn brush_select_ring(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    halfedge_q: Query<&BrushHalfedge>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.entity?;
    if selection.edges.is_empty() {
        return OperatorResult::Cancelled;
    }
    let halfedge = halfedge_q.get(brush_entity)?;

    // Map each selected cache edge (a, b) to its HalfedgeMesh EdgeKey.
    let mut mesh_edges: Vec<EdgeKey> = Vec::with_capacity(selection.edges.len());
    for &(a, b) in &selection.edges {
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

    // For each selected HalfedgeMesh edge, walk its ring. Union all results.
    let mut walked: HashSet<EdgeKey> = HashSet::new();
    for ek in mesh_edges {
        for k in ring_walk(&halfedge.mesh, ek) {
            walked.insert(k);
        }
    }
    if walked.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Build a VertKey -> cache index lookup (inverse of vert_keys).
    let mut key_to_idx: std::collections::HashMap<VertKey, usize> =
        std::collections::HashMap::with_capacity(halfedge.vert_keys.len());
    for (i, &k) in halfedge.vert_keys.iter().enumerate() {
        key_to_idx.insert(k, i);
    }

    // Convert walked HalfedgeMesh edges back to (usize, usize) cache pairs.
    let mut new_edges: Vec<(usize, usize)> = Vec::with_capacity(walked.len());
    for ek in walked {
        let edge = &halfedge.mesh.edges[ek];
        let Some(&a) = key_to_idx.get(&edge.v[0]) else {
            continue;
        };
        let Some(&b) = key_to_idx.get(&edge.v[1]) else {
            continue;
        };
        let pair = if a < b { (a, b) } else { (b, a) };
        if !new_edges.contains(&pair) {
            new_edges.push(pair);
        }
    }

    selection.edges = new_edges;
    OperatorResult::Finished
}

fn find_edge_between(mesh: &HalfedgeMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    mesh.edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

pub(crate) fn can_run_select_ring(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge) && !selection.edges.is_empty()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushSelectRingOp>();
}
