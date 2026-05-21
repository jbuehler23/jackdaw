//! `brush.mesh.subdivide` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::subdivide::subdivide;
use jackdaw_geometry::halfedge::{EdgeKey, HalfedgeMesh, VertKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Split each selected edge at its midpoint and re-tessellate touched faces.
/// Operates on the current edge selection. No modal interaction. Available
/// in Edge mode with at least one edge selected.
#[operator(
    id = "brush.mesh.subdivide",
    label = "Subdivide",
    is_available = can_run_subdivide,
    allows_undo = true
)]
pub(crate) fn brush_subdivide(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
        return OperatorResult::Cancelled;
    }
    let Some(brush_entity) = selection.entity else {
        return OperatorResult::Cancelled;
    };
    if selection.edges.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Snapshot before mutation for undo.

    // Map each selected cache-edge (a, b) to a HalfedgeMesh EdgeKey via vert_keys.
    let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
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

    // Run the HalfedgeMesh op.
    let Ok(subdivide_result) = subdivide(&mut halfedge.mesh, &mesh_edges) else {
        return OperatorResult::Cancelled;
    };

    // Capture the topology vertex index pair for each new cross-cut edge so we
    // can write them into `BrushSelection.edges` after the flatten/re-lift
    // roundtrip. Topology vertex order matches HalfedgeMesh slotmap iteration order
    // (see `flatten_to_topology`); subdivide never removes verts, so the slot
    // positions are stable here.
    let new_edge_pairs: Vec<(usize, usize)> = {
        let mut vk_to_topo: std::collections::HashMap<VertKey, usize> =
            std::collections::HashMap::with_capacity(halfedge.mesh.verts.len());
        for (i, (k, _)) in halfedge.mesh.verts.iter().enumerate() {
            vk_to_topo.insert(k, i);
        }
        let mut out: Vec<(usize, usize)> = Vec::with_capacity(subdivide_result.new_edges.len());
        for ek in &subdivide_result.new_edges {
            let Some(edge) = halfedge.mesh.edges.get(*ek) else {
                continue;
            };
            let Some(&a) = vk_to_topo.get(&edge.v[0]) else {
                continue;
            };
            let Some(&b) = vk_to_topo.get(&edge.v[1]) else {
                continue;
            };
            let pair = if a < b { (a, b) } else { (b, a) };
            if !out.contains(&pair) {
                out.push(pair);
            }
        }
        out
    };

    // Re-cache all face normals.
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

    // Flatten HalfedgeMesh -> topology, sync Brush.faces[i].plane + Brush.topology.
    let new_topology = halfedge.mesh.flatten_to_topology();
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    // Subdivide may add new faces. Extend brush.faces with copies of the last
    // existing face data as a default; material_idx from the parent face is
    // inherited during flatten.
    let new_face_count = new_topology.polygons.len();
    while brush.faces.len() < new_face_count {
        let template = brush.faces.last().cloned().unwrap_or_default();
        brush.faces.push(template);
    }

    // Update plane data per face from new topology.
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

    // Re-lift HalfedgeMesh from new topology so vert_keys / face_keys are consistent.
    let new_mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let new_vert_keys: Vec<_> = new_mesh.verts.keys().collect();
    let mut new_face_keys = vec![Default::default(); new_mesh.faces.len()];
    for (k, f) in new_mesh.faces.iter() {
        let slot = f.material_idx as usize;
        if slot < new_face_keys.len() {
            new_face_keys[slot] = k;
        }
    }
    halfedge.mesh = new_mesh;
    halfedge.vert_keys = new_vert_keys;
    halfedge.face_keys = new_face_keys;

    // Push undo entry.
    // Chain selection: write the new cross-cut edges into `BrushSelection.edges`
    // so a follow-up gesture (loop cut, edge slide, subdivide again) operates
    // on the freshly created geometry.
    let vert_count = brush.topology.vertices.len();
    let inbounds: Vec<(usize, usize)> = new_edge_pairs
        .into_iter()
        .filter(|(a, b)| *a < vert_count && *b < vert_count)
        .collect();
    if !inbounds.is_empty() {
        selection.edges = inbounds;
    }

    OperatorResult::Finished
}

fn find_edge_between(mesh: &HalfedgeMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    mesh.edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

pub(crate) fn can_run_subdivide(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge) && !selection.edges.is_empty()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushSubdivideOp>();
    // No keybind; operator is available via menu / command palette only.
}
