//! `brush.mesh.bridge_edge_loops` operator.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::bridge_edge_loops::bridge_edge_loops;
use jackdaw_geometry::halfedge::{EdgeKey, HalfedgeMesh, VertKey, apply_topology_edit};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Connect two selected edge loops with a quad strip. The selection must
/// contain edges forming exactly two distinct connected loops with the same
/// vertex count. Available in Edge mode.
#[operator(
    id = "brush.mesh.bridge_edge_loops",
    label = "Bridge Edge Loops",
    is_available = can_run_bridge,
    allows_undo = true
)]
pub(crate) fn brush_bridge_edge_loops(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
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
    if sel_edges.len() < 2 {
        return OperatorResult::Cancelled;
    }

    let mut halfedge = halfedge_q.get_mut(brush_entity)?;

    // Map cache edge pairs (a, b) -> HalfedgeMesh EdgeKeys via vert_keys.
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
    if mesh_edges.len() < 2 {
        return OperatorResult::Cancelled;
    }

    // Partition into connected components (BFS over edge adjacency through verts).
    let components = partition_edges_by_connectivity(&halfedge.mesh, &mesh_edges);
    if components.len() != 2 {
        return OperatorResult::Cancelled;
    }
    let edges_a = &components[0];
    let edges_b = &components[1];

    // Bridge the loops and reconcile, snapshotting the material_idx of each
    // newly created face so the post-commit selection can resolve its topology
    // face index. `flatten_to_topology` stable-sorts by `material_idx`, and
    // `create_face_from_verts` assigns `max_existing + 1` each call (so every
    // bridge face has a distinct, monotonically increasing material_idx with no
    // ties). The post-flatten topology index for a face with material_idx M is
    // therefore `count(faces with material_idx < M)`. `into_inner` reborrows the
    // change-detected `Mut<Brush>` as `&mut Brush` so the two fields can be
    // borrowed disjointly.
    let brush = brushes.get_mut(brush_entity)?.into_inner();
    let source = brush.faces.last().cloned().unwrap_or_default();
    let original_face_count = brush.faces.len();
    let new_face_material_idxs: Vec<u32> = apply_topology_edit(
        &mut brush.faces,
        &mut brush.topology,
        &mut halfedge.0,
        |mesh| {
            bridge_edge_loops(mesh, edges_a, edges_b).map(|result| {
                result
                    .new_faces
                    .iter()
                    .filter_map(|fk| mesh.faces.get(*fk).map(|f| f.material_idx))
                    .collect()
            })
        },
    )?;

    // Any face the bridge created inherits the previous last face's appearance.
    for new_face in original_face_count..brush.faces.len() {
        brush.faces[new_face].copy_appearance_from(&source);
        brush.faces[new_face].ensure_uv_axes();
    }

    // Resolve the post-flatten topology face index for each new bridge face by
    // counting faces with strictly smaller material_idx in the post-op mesh
    // (mirrors the inset chaining logic; see commentary there).
    let face_count = brush.faces.len();
    let new_face_indices: Vec<usize> = new_face_material_idxs
        .into_iter()
        .map(|mtx| {
            halfedge
                .mesh
                .faces
                .values()
                .filter(|f| f.material_idx < mtx)
                .count()
        })
        .filter(|&i| i < face_count)
        .collect();
    if !new_face_indices.is_empty() {
        selection.sub_mut(brush_entity).faces = new_face_indices;
    }
    OperatorResult::Finished
}

fn find_edge_between(mesh: &HalfedgeMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    mesh.edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

/// Partition the given edges into connected components based on shared vertices.
fn partition_edges_by_connectivity(mesh: &HalfedgeMesh, edges: &[EdgeKey]) -> Vec<Vec<EdgeKey>> {
    let edge_set: HashSet<EdgeKey> = edges.iter().copied().collect();
    let mut vert_to_edges: HashMap<VertKey, Vec<EdgeKey>> = HashMap::new();
    for &e in edges {
        let edge = &mesh.edges[e];
        vert_to_edges.entry(edge.v[0]).or_default().push(e);
        vert_to_edges.entry(edge.v[1]).or_default().push(e);
    }
    let mut visited: HashSet<EdgeKey> = HashSet::new();
    let mut components: Vec<Vec<EdgeKey>> = Vec::new();
    for &start_edge in edges {
        if visited.contains(&start_edge) {
            continue;
        }
        // BFS from this edge.
        let mut stack: Vec<EdgeKey> = vec![start_edge];
        let mut component: Vec<EdgeKey> = Vec::new();
        while let Some(e) = stack.pop() {
            if !visited.insert(e) {
                continue;
            }
            if !edge_set.contains(&e) {
                continue;
            }
            component.push(e);
            let edge = &mesh.edges[e];
            for &v in &edge.v {
                if let Some(neigh) = vert_to_edges.get(&v) {
                    for &ne in neigh {
                        if !visited.contains(&ne) {
                            stack.push(ne);
                        }
                    }
                }
            }
        }
        components.push(component);
    }
    components
}

pub(crate) fn can_run_bridge(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge)
        && selection.active_sub().is_some_and(|s| s.edges.len() >= 2)
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushBridgeEdgeLoopsOp>();
}
