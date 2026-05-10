//! `brush.mesh.inset` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::editmesh::{EditMesh, FaceKey};
use jackdaw_geometry::editmesh::ops::inset_face::inset_face;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMesh, BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;

const DEFAULT_INSET_AMOUNT: f32 = 0.1;

/// Shrink each selected face inward along its plane by a fixed amount and
/// connect the old and new rings with side quads. Operates on the current
/// face selection. Available in Face mode with at least one face selected.
#[operator(
    id = "brush.mesh.inset",
    label = "Inset",
    is_available = can_run_inset,
    allows_undo = true
)]
pub(crate) fn brush_inset(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushEditMesh>,
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

    // Map cache face indices to EditMesh FaceKeys via face_keys parallel array.
    let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let mut bmesh_faces: Vec<FaceKey> = Vec::with_capacity(selection.faces.len());
    for &face_idx in &selection.faces {
        if let Some(&fk) = bmesh_component.face_keys.get(face_idx) {
            bmesh_faces.push(fk);
        }
    }
    if bmesh_faces.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Run inset_face on each selected face.
    // If an individual face fails (e.g. degenerate), skip silently; other faces still inset.
    for fk in bmesh_faces {
        let _ = inset_face(&mut bmesh_component.mesh, fk, DEFAULT_INSET_AMOUNT);
    }

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

    // Flatten EditMesh -> topology, sync Brush.faces[i].plane + Brush.topology.
    let new_topology = bmesh_component.mesh.flatten_to_topology();
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    // Inset adds new faces. Extend brush.faces with copies of the last
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

    // Re-lift EditMesh from new topology so vert_keys / face_keys are consistent.
    let new_bmesh = EditMesh::lift_from_topology(&brush.topology);
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
        label: "Inset".to_string(),
    }));

    OperatorResult::Finished
}

pub(crate) fn can_run_inset(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !selection.faces.is_empty()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushInsetOp>();
    // No keybind; operator is available via menu / command palette only for MVP.
}
