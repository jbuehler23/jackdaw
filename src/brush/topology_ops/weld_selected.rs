//! `brush.mesh.weld_selected` operator. Welds all selected verts together
//! at their centroid, regardless of original distance. Use this when you want
//! to "merge these specific verts into one". Different from "Merge by Distance"
//! which only welds verts that are already coincident within a threshold.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::HalfedgeMesh;
use jackdaw_geometry::halfedge::ops::remove_doubles::remove_doubles;
use jackdaw_jsn::Brush;

use crate::brush::{BrushHalfedge, BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;

/// Weld all selected verts together at their centroid, regardless of distance.
/// Different from "Merge by Distance" (threshold-based). Available in Vertex
/// mode with at least 2 verts selected.
#[operator(
    id = "brush.mesh.weld_selected",
    label = "Weld Selected Vertices",
    is_available = can_run_weld,
    allows_undo = true
)]
pub(crate) fn brush_weld_selected(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
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

    let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
        return OperatorResult::Cancelled;
    };
    let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    // Map cache vertex indices to HalfedgeMesh VertKeys.
    let mut vert_keys = Vec::with_capacity(selection.vertices.len());
    for &vi in &selection.vertices {
        if let Some(&k) = halfedge.vert_keys.get(vi) {
            vert_keys.push(k);
        }
    }
    if vert_keys.len() < 2 {
        return OperatorResult::Cancelled;
    }

    // Compute centroid of selected verts.
    let mut sum = Vec3::ZERO;
    for &k in &vert_keys {
        if let Some(v) = halfedge.mesh.verts.get(k) {
            sum += v.co;
        }
    }
    let centroid = sum / vert_keys.len() as f32;

    // Move all selected verts to the centroid.
    for &k in &vert_keys {
        if let Some(v) = halfedge.mesh.verts.get_mut(k) {
            v.co = centroid;
        }
    }

    // Run remove_doubles to weld coincident verts into one. The tiny threshold
    // ensures only verts we just moved together are merged, not distant ones.
    let _ = remove_doubles(&mut halfedge.mesh, 0.0001);

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

    let new_face_count = new_topology.polygons.len();
    if brush.faces.len() > new_face_count {
        brush.faces.truncate(new_face_count);
    }
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

    // Re-lift HalfedgeMesh from new topology so vert_keys / face_keys are consistent.
    let new_bmesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let new_vert_keys: Vec<_> = new_bmesh.verts.keys().collect();
    let mut new_face_keys = vec![Default::default(); new_bmesh.faces.len()];
    for (k, f) in new_bmesh.faces.iter() {
        let slot = f.material_idx as usize;
        if slot < new_face_keys.len() {
            new_face_keys[slot] = k;
        }
    }
    halfedge.mesh = new_bmesh;
    halfedge.vert_keys = new_vert_keys;
    halfedge.face_keys = new_face_keys;

    // Push undo entry.
    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old: brush_before,
        new: brush.clone(),
        label: "Weld Selected Vertices".to_string(),
    }));

    OperatorResult::Finished
}

pub(crate) fn can_run_weld(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex) && selection.vertices.len() >= 2
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushWeldSelectedOp>();
}
