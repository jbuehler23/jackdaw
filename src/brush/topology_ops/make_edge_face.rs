//! `brush.mesh.make_edge_face` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::VertKey;
use jackdaw_geometry::halfedge::ops::contextual_create::{ContextualResult, contextual_create};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;

/// Captured selection target for the F-key `contextual_create` result. Held
/// across the flatten/re-lift roundtrip so the post-commit selection update
/// can map back to a topology index (or vertex index pair).
enum ChainTarget {
    /// Topology vertex index pair (a < b) for the newly created edge.
    Edge((usize, usize)),
    /// `material_idx` of the newly created face; resolved to a topology face
    /// index via `count(faces with material_idx < this)` (mirrors inset logic).
    Face(u32),
}

/// Fill the current vertex selection with a new edge or face. Two verts -> edge.
/// Three or more -> face whose ring is the selected verts in selection order.
/// Available in Vertex mode.
#[operator(
    id = "brush.mesh.make_edge_face",
    label = "Make Edge / Face",
    is_available = can_run_make_edge_face,
    allows_undo = true
)]
pub(crate) fn brush_make_edge_face(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Vertex) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.entity?;
    if selection.vertices.len() < 2 {
        return OperatorResult::Cancelled;
    }

    // Snapshot before mutation for undo.
    let brush_before = brushes.get(brush_entity).cloned()?;

    // Map cache vertex indices to HalfedgeMesh VertKeys via vert_keys parallel array.
    let mut halfedge = halfedge_q.get_mut(brush_entity)?;
    let mut vert_keys: Vec<VertKey> = Vec::with_capacity(selection.vertices.len());
    for &vert_idx in &selection.vertices {
        if let Some(&vk) = halfedge.vert_keys.get(vert_idx) {
            vert_keys.push(vk);
        }
    }
    if vert_keys.len() < 2 {
        return OperatorResult::Cancelled;
    }

    // Run contextual_create on the selected vertices.
    let create_result = contextual_create(&mut halfedge.mesh, &vert_keys);

    // Capture either the new edge's (v0, v1) topology index pair or the new
    // face's `material_idx` so we can resolve the post-flatten selection
    // targets. Topology vertex order matches HalfedgeMesh slotmap iteration order
    // (see `flatten_to_topology`); `contextual_create` never removes verts.
    let chain_target: Option<ChainTarget> = match &create_result {
        Ok(ContextualResult::Edge(ek)) => {
            if let Some(edge) = halfedge.mesh.edges.get(*ek) {
                let mut a_idx: Option<usize> = None;
                let mut b_idx: Option<usize> = None;
                for (i, (k, _)) in halfedge.mesh.verts.iter().enumerate() {
                    if k == edge.v[0] {
                        a_idx = Some(i);
                    }
                    if k == edge.v[1] {
                        b_idx = Some(i);
                    }
                }
                if let (Some(a), Some(b)) = (a_idx, b_idx) {
                    let pair = if a < b { (a, b) } else { (b, a) };
                    Some(ChainTarget::Edge(pair))
                } else {
                    None
                }
            } else {
                None
            }
        }
        Ok(ContextualResult::Face(fk)) => halfedge
            .mesh
            .faces
            .get(*fk)
            .map(|f| ChainTarget::Face(f.material_idx)),
        Err(_) => None,
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
    let mut brush = brushes.get_mut(brush_entity)?;

    // Make_edge_face may add new faces. Extend brush.faces with copies of the last
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
    let new_mesh = jackdaw_geometry::halfedge::HalfedgeMesh::lift_from_topology(&brush.topology);
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
    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old: brush_before,
        new: brush.clone(),
        label: "Make Edge / Face".to_string(),
    }));

    // Chain selection: write the new edge or face into `BrushSelection` so the
    // user can immediately act on it (e.g. toggle to Edge / Face mode and drag).
    match chain_target {
        Some(ChainTarget::Edge((a, b))) => {
            let vert_count = brush.topology.vertices.len();
            if a < vert_count && b < vert_count {
                selection.edges = vec![(a, b)];
            }
        }
        Some(ChainTarget::Face(mtx)) => {
            let face_idx = halfedge
                .mesh
                .faces
                .values()
                .filter(|f| f.material_idx < mtx)
                .count();
            if face_idx < brush.faces.len() {
                selection.faces = vec![face_idx];
            }
        }
        None => {}
    }

    OperatorResult::Finished
}

pub(crate) fn can_run_make_edge_face(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex) && selection.vertices.len() >= 2
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushMakeEdgeFaceOp>();
    // No keybind; operator is available via menu / command palette only for MVP.
}
