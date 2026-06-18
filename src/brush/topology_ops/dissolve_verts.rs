//! `brush.mesh.dissolve_verts` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::dissolve_verts::dissolve_verts;
use jackdaw_geometry::halfedge::{VertKey, apply_topology_edit};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Remove the selected verts and merge incident faces. MVP: only valence-2 verts are
/// dissolved; higher-valence verts skipped silently. Available in Vertex mode.
#[operator(
    id = "brush.mesh.dissolve_verts",
    label = "Dissolve Vertices",
    is_available = can_run_dissolve_verts,
    allows_undo = true
)]
pub(crate) fn brush_dissolve_verts(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Vertex) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.active_brush?;
    let sel_verts: Vec<usize> = selection
        .sub(brush_entity)
        .map(|s| s.vertices.clone())
        .unwrap_or_default();
    if sel_verts.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Map cache vertex indices to HalfedgeMesh VertKeys via vert_keys parallel array.
    let mut halfedge = halfedge_q.get_mut(brush_entity)?;
    let mut vert_keys: Vec<VertKey> = Vec::with_capacity(sel_verts.len());
    for &vert_idx in &sel_verts {
        if let Some(&vk) = halfedge.vert_keys.get(vert_idx) {
            vert_keys.push(vk);
        }
    }
    if vert_keys.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Apply the dissolve and reconcile the brush's faces, topology, and binding.
    // Merging faces leaves the surviving faces' `material_idx` values gapped, so
    // capture the appearance from the source slot keyed on `material_idx` instead
    // of letting the reconcile default-fill. The closure returns the post-edit
    // material_idxes sorted ascending, matching the order `flatten_to_topology`
    // lays out the new polygons. `into_inner` reborrows the change-detected
    // `Mut<Brush>` as `&mut Brush` so the two fields can be borrowed disjointly.
    let brush = brushes.get_mut(brush_entity)?.into_inner();
    let old_faces = brush.faces.clone();
    let sorted_mat_idxes = apply_topology_edit(
        &mut brush.faces,
        &mut brush.topology,
        &mut halfedge.0,
        |mesh| {
            let result = dissolve_verts(mesh, &vert_keys);
            let mut idxes: Vec<u32> = mesh.faces.values().map(|f| f.material_idx).collect();
            idxes.sort_unstable();
            result.map(|_| idxes)
        },
    )?;

    // Rebuild brush.faces parallel to the new polygons. For each slot, look up
    // the old appearance by the face's material_idx, falling back to the last
    // entry if the index is out of range.
    let mut new_faces: Vec<jackdaw_jsn::BrushFaceData> = Vec::with_capacity(sorted_mat_idxes.len());
    for &mat_idx in &sorted_mat_idxes {
        let old = old_faces
            .get(mat_idx as usize)
            .cloned()
            .unwrap_or_else(|| old_faces.last().cloned().unwrap_or_default());
        new_faces.push(old);
    }
    brush.faces = new_faces;
    brush.topology.recompute_face_planes(&mut brush.faces);

    OperatorResult::Finished
}

pub(crate) fn can_run_dissolve_verts(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex)
        && selection
            .active_sub()
            .is_some_and(|s| !s.vertices.is_empty())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushDissolveVertsOp>();
    // No keybind; operator is available via menu / command palette only for MVP.
}
