//! `brush.mesh.loop_cut` operator.

use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;
use jackdaw_geometry::bmesh::{BMesh, EdgeKey, VertKey};
use jackdaw_geometry::bmesh::ops::loop_cut::loop_cut;
use jackdaw_jsn::Brush;

use crate::brush::{BrushBMesh, BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;
use crate::core_extension::CoreExtensionInputContext;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushLoopCutOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<BrushLoopCutOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyR.with_mod_keys(ModKeys::CONTROL),
                Press::default(),
            )],
        ));
    });
}

/// Insert a new edge loop across a strip of quad faces. Walks the edge ring
/// from the first selected edge until it hits a non-quad or boundary. The
/// loop is inserted at the midpoint of each crossed edge. Requires Edge mode
/// with at least one edge selected.
#[operator(
    id = "brush.mesh.loop_cut",
    label = "Loop Cut",
    is_available = can_run_loop_cut,
    allows_undo = true
)]
pub(crate) fn brush_loop_cut(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut bmesh_q: Query<&mut BrushBMesh>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
        return OperatorResult::Cancelled;
    }
    let Some(brush_entity) = selection.entity else {
        return OperatorResult::Cancelled;
    };
    let Some(&(a, b)) = selection.edges.first() else {
        return OperatorResult::Cancelled;
    };

    // Snapshot before mutation for undo.
    let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
        return OperatorResult::Cancelled;
    };

    // Map cache edge (a, b) -> BMesh EdgeKey via vert_keys.
    let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    let va: VertKey = match bmesh_component.vert_keys.get(a) {
        Some(&k) => k,
        None => return OperatorResult::Cancelled,
    };
    let vb: VertKey = match bmesh_component.vert_keys.get(b) {
        Some(&k) => k,
        None => return OperatorResult::Cancelled,
    };
    let edge_key: EdgeKey = match find_edge_between(&bmesh_component.mesh, va, vb) {
        Some(k) => k,
        None => return OperatorResult::Cancelled,
    };

    // Run the BMesh op.
    let result = loop_cut(&mut bmesh_component.mesh, edge_key, 0.5);
    let Ok(_loop_cut_result) = result else {
        return OperatorResult::Cancelled;
    };

    // Re-cache all face normals (drag op pattern).
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

    // Loop cut adds new faces (one per crossed quad). Extend brush.faces with
    // copies of parent face data per new face. The flatten step assigns
    // material_idx to each face; new faces inherited material_idx from their
    // parent face. We duplicate the last existing face's data as a default
    // (caller adjusts UV axes or material if needed).
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

    // Re-lift BMesh from the new topology so vert_keys / face_keys are consistent.
    let new_bmesh = BMesh::lift_from_topology(&brush.topology);
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
        label: "Loop Cut".to_string(),
    }));

    OperatorResult::Finished
}

fn find_edge_between(bmesh: &BMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    bmesh
        .edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

pub(crate) fn can_run_loop_cut(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge) && !selection.edges.is_empty()
}
