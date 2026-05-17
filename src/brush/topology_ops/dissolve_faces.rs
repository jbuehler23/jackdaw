//! `brush.mesh.dissolve_faces` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::dissolve_faces::dissolve_faces;
use jackdaw_geometry::halfedge::{HalfedgeMesh, FaceKey};
use jackdaw_jsn::Brush;

use crate::brush::{BrushHalfedge, BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;

/// Remove the selected faces, leaving holes. Boundary edges become wire edges.
/// Available in Face mode.
#[operator(
    id = "brush.mesh.dissolve_faces",
    label = "Dissolve Faces",
    is_available = can_run_dissolve_faces,
    allows_undo = true
)]
pub(crate) fn brush_dissolve_faces(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Face) {
        return OperatorResult::Cancelled;
    }
    let Some(brush_entity) = selection.entity else {
        return OperatorResult::Cancelled;
    };
    if selection.faces.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Snapshot before mutation for undo.
    let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
        return OperatorResult::Cancelled;
    };

    // Map cache face indices to HalfedgeMesh FaceKeys via face_keys parallel array.
    let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let mut mesh_faces: Vec<FaceKey> = Vec::with_capacity(selection.faces.len());
    for &face_idx in &selection.faces {
        if let Some(&fk) = halfedge.face_keys.get(face_idx) {
            mesh_faces.push(fk);
        }
    }
    if mesh_faces.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Run dissolve_faces on the selected faces.
    let Ok(_) = dissolve_faces(&mut halfedge.mesh, &mesh_faces) else {
        return OperatorResult::Cancelled;
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

    // Dissolve_faces removes faces. Truncate brush.faces if the topology
    // now has fewer faces than before.
    if brush.faces.len() > new_topology.polygons.len() {
        brush.faces.truncate(new_topology.polygons.len());
    }

    // Ensure we have enough face data entries for any newly created faces.
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
    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old: brush_before,
        new: brush.clone(),
        label: "Dissolve Faces".to_string(),
    }));

    OperatorResult::Finished
}

pub(crate) fn can_run_dissolve_faces(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !selection.faces.is_empty()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushDissolveFacesOp>();
    // No keybind; operator is available via menu / command palette only for MVP.
}
