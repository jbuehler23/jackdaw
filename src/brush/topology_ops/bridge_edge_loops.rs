//! `brush.mesh.bridge_edge_loops` operator.

use std::collections::{HashMap, HashSet};

use bevy_ecs::prelude::*;
use bevy_math::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::bridge_edge_loops::bridge_edge_loops;
use jackdaw_geometry::halfedge::{EdgeKey, HalfedgeMesh, VertKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;

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
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
        return OperatorResult::Cancelled;
    }
    let Some(brush_entity) = selection.entity else {
        return OperatorResult::Cancelled;
    };
    if selection.edges.len() < 2 {
        return OperatorResult::Cancelled;
    }

    let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
        return OperatorResult::Cancelled;
    };
    let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    // Map cache edge pairs (a, b) -> HalfedgeMesh EdgeKeys via vert_keys.
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

    let Ok(result) = bridge_edge_loops(&mut halfedge.mesh, edges_a, edges_b) else {
        return OperatorResult::Cancelled;
    };

    // Snapshot the material_idx of each newly created face so we can resolve
    // post-flatten topology face indices. `flatten_to_topology` stable-sorts by
    // `material_idx`, and `create_face_from_verts` assigns `max_existing + 1`
    // each call (so every bridge face has a distinct, monotonically increasing
    // material_idx with no ties). The post-flatten topology index for a face
    // with material_idx M is therefore `count(faces with material_idx < M)`.
    let new_face_material_idxs: Vec<u32> = result
        .new_faces
        .iter()
        .filter_map(|fk| halfedge.mesh.faces.get(*fk).map(|f| f.material_idx))
        .collect();

    // Re-cache normals (mirror inset/extrude pattern).
    let face_keys_all: Vec<_> = halfedge.mesh.faces.keys().collect();
    for fk in face_keys_all {
        let face = &halfedge.mesh.faces[fk];
        let mut ring_positions = Vec::with_capacity(face.loop_count as usize);
        let mut cur = face.loop_first;
        for _ in 0..face.loop_count {
            let lp = &halfedge.mesh.loops[cur];
            ring_positions.push(halfedge.mesh.verts[lp.vert].co);
            cur = lp.next;
        }
        let new_normal = jackdaw_geometry::newell_normal(&ring_positions);
        halfedge.mesh.faces[fk].normal_cache = new_normal;
    }

    // Flatten + sync planes + grow brush.faces.
    let new_topology = halfedge.mesh.flatten_to_topology();
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let new_face_count = new_topology.polygons.len();
    while brush.faces.len() < new_face_count {
        let template = brush.faces.last().cloned().unwrap_or_default();
        brush.faces.push(template);
    }
    let positions: Vec<Vec3> = new_topology.vertices.iter().map(|v| v.position).collect();
    for (face_idx, face_data) in brush.faces.iter_mut().enumerate() {
        if face_idx < new_topology.polygons.len() {
            let normal = new_topology.face_normal_with(&positions, face_idx);
            let v0_idx = new_topology.loops[new_topology.polygons[face_idx].loop_start as usize]
                .vert as usize;
            let distance = positions[v0_idx].dot(normal);
            face_data.plane.normal = normal;
            face_data.plane.distance = distance;
        }
    }
    brush.topology = new_topology;

    // Re-lift HalfedgeMesh.
    let new_mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let new_vert_keys: Vec<_> = new_mesh.verts.keys().collect();
    let mut new_face_keys = vec![Default::default(); new_mesh.faces.len()];
    for (k, f) in new_mesh.faces.iter() {
        if (f.material_idx as usize) < new_face_keys.len() {
            new_face_keys[f.material_idx as usize] = k;
        }
    }
    halfedge.mesh = new_mesh;
    halfedge.vert_keys = new_vert_keys;
    halfedge.face_keys = new_face_keys;

    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old: brush_before,
        new: brush.clone(),
        label: "Bridge Edge Loops".to_string(),
    }));

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
        selection.faces = new_face_indices;
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
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge) && selection.edges.len() >= 2
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushBridgeEdgeLoopsOp>();
}
