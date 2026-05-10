//! `brush.mesh.make_edge_face` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::bmesh::VertKey;
use jackdaw_geometry::bmesh::ops::contextual_create::contextual_create;
use jackdaw_jsn::Brush;

use crate::brush::{BrushBMesh, BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;

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
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushBMesh>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Vertex) {
        return OperatorResult::Cancelled;
    }
    let Some(brush_entity) = selection.entity else {
        return OperatorResult::Cancelled;
    };
    if selection.vertices.len() < 2 {
        return OperatorResult::Cancelled;
    }

    // Snapshot before mutation for undo.
    let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
        return OperatorResult::Cancelled;
    };

    // Map cache vertex indices to BMesh VertKeys via vert_keys parallel array.
    let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let mut vert_keys: Vec<VertKey> = Vec::with_capacity(selection.vertices.len());
    for &vert_idx in &selection.vertices {
        if let Some(&vk) = bmesh_component.vert_keys.get(vert_idx) {
            vert_keys.push(vk);
        }
    }
    if vert_keys.len() < 2 {
        return OperatorResult::Cancelled;
    }

    // Run contextual_create on the selected vertices.
    let _ = contextual_create(&mut bmesh_component.mesh, &vert_keys);

    // Re-cache all face normals.
    let face_keys_all: Vec<_> = bmesh_component.mesh.faces.keys().collect();
    for fk in face_keys_all {
        let face = &bmesh_component.mesh.faces[fk];
        let mut ring_positions = Vec::with_capacity(face.loop_count as usize);
        let mut cur = face.loop_first;
        for _ in 0..face.loop_count {
            let lp = &bmesh_component.mesh.loops[cur];
            ring_positions.push(bmesh_component.mesh.verts[lp.vert].co);
            cur = lp.next;
        }
        let new_normal = jackdaw_geometry::newell_normal(&ring_positions);
        bmesh_component.mesh.faces[fk].normal_cache = new_normal;
    }

    // Flatten BMesh -> topology, sync Brush.faces[i].plane + Brush.topology.
    let new_topology = bmesh_component.mesh.flatten_to_topology();
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };

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
            let v0_idx =
                new_topology.loops[new_topology.polygons[face_idx].loop_start as usize].vert
                    as usize;
            let distance = positions[v0_idx].dot(normal);
            face_data.plane.normal = normal;
            face_data.plane.distance = distance;
        }
    }
    brush.topology = new_topology;

    // Re-lift BMesh from new topology so vert_keys / face_keys are consistent.
    let new_bmesh = jackdaw_geometry::bmesh::BMesh::lift_from_topology(&brush.topology);
    let new_vert_keys: Vec<_> = new_bmesh.verts.keys().collect();
    let mut new_face_keys = vec![Default::default(); new_bmesh.faces.len()];
    for (k, f) in new_bmesh.faces.iter() {
        let slot = f.material_idx as usize;
        if slot < new_face_keys.len() {
            new_face_keys[slot] = k;
        }
    }
    bmesh_component.mesh = new_bmesh;
    bmesh_component.vert_keys = new_vert_keys;
    bmesh_component.face_keys = new_face_keys;

    // Push undo entry.
    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old: brush_before,
        new: brush.clone(),
        label: "Make Edge / Face".to_string(),
    }));

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
