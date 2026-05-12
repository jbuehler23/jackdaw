//! `brush.mesh.merge_by_distance` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::editmesh::EditMesh;
use jackdaw_geometry::editmesh::ops::remove_doubles::remove_doubles;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMesh, EditMode, SetBrush};
use crate::commands::CommandHistory;

const DEFAULT_MERGE_DISTANCE: f32 = 0.0001;

/// Weld vertices within a small distance threshold (default 0.0001 m). Removes
/// degenerate edges and faces left after the merge. Operates on the entire brush,
/// not just selection. Available in any brush edit mode.
#[operator(
    id = "brush.mesh.merge_by_distance",
    label = "Merge by Distance",
    is_available = can_run_merge,
    allows_undo = true
)]
pub(crate) fn brush_merge_by_distance(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<crate::brush::BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushEditMesh>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    // Check that we're in any brush edit mode.
    if !matches!(*edit_mode, EditMode::BrushEdit(_)) {
        return OperatorResult::Cancelled;
    }

    // Get the currently edited brush entity.
    let Some(brush_entity) = selection.entity else {
        return OperatorResult::Cancelled;
    };

    // Snapshot before mutation for undo.
    let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
        return OperatorResult::Cancelled;
    };

    // Get mutable EditMesh and run remove_doubles.
    let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    // Run remove_doubles on the whole mesh.
    let Ok(_) = remove_doubles(&mut bmesh_component.mesh, DEFAULT_MERGE_DISTANCE) else {
        return OperatorResult::Cancelled;
    };

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

    // remove_doubles may drop degenerate faces. Truncate brush.faces
    // if the topology now has fewer faces than before.
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
        label: "Merge by Distance".to_string(),
    }));

    OperatorResult::Finished
}

pub(crate) fn can_run_merge(edit_mode: Res<EditMode>) -> bool {
    matches!(*edit_mode, EditMode::BrushEdit(_))
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushMergeByDistanceOp>();
    // No keybind; operator is available via menu / command palette only.
}
