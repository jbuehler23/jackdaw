//! `brush.mesh.edge_slide` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::editmesh::{EditMesh, EdgeKey, VertKey};
use jackdaw_geometry::editmesh::ops::edge_slide::edge_slide;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMesh, BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;

const DEFAULT_SLIDE_T: f32 = 0.5;

/// Slide the selected edges along their parallel-edge directions in adjacent
/// quad faces. Pure transform; topology unchanged. Available in Edge mode with
/// selected edges.
#[operator(
    id = "brush.mesh.edge_slide",
    label = "Edge Slide",
    is_available = can_run_edge_slide,
    allows_undo = true
)]
pub(crate) fn brush_edge_slide(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushEditMesh>,
    mut history: ResMut<CommandHistory>,
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
    let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
        return OperatorResult::Cancelled;
    };

    // Map each selected cache-edge (a, b) to a EditMesh EdgeKey via vert_keys.
    let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let mut bmesh_edges: Vec<EdgeKey> = Vec::with_capacity(selection.edges.len());
    for &(a, b) in &selection.edges {
        let Some(&va) = bmesh_component.vert_keys.get(a) else {
            continue;
        };
        let Some(&vb) = bmesh_component.vert_keys.get(b) else {
            continue;
        };
        if let Some(ek) = find_edge_between(&bmesh_component.mesh, va, vb) {
            bmesh_edges.push(ek);
        }
    }
    if bmesh_edges.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Run the EditMesh op.
    let Ok(_edge_slide_result) = edge_slide(&mut bmesh_component.mesh, &bmesh_edges, DEFAULT_SLIDE_T) else {
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

    // Edge slide does not add new faces; no need to grow brush.faces.
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
        label: "Edge Slide".to_string(),
    }));

    OperatorResult::Finished
}

fn find_edge_between(bmesh: &EditMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    bmesh
        .edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

pub(crate) fn can_run_edge_slide(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge) && !selection.edges.is_empty()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushEdgeSlideOp>();
    // No keybind; operator is available via menu / command palette only.
}
