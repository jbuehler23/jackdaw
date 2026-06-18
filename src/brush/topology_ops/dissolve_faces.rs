//! `brush.mesh.dissolve_faces` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::dissolve_faces::dissolve_faces;
use jackdaw_geometry::halfedge::{FaceKey, apply_topology_edit};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

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
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Face) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.active_brush?;
    let sel_faces: Vec<usize> = selection
        .sub(brush_entity)
        .map(|s| s.faces.clone())
        .unwrap_or_default();
    if sel_faces.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Map cache face indices to HalfedgeMesh FaceKeys via face_keys parallel array.
    let mut halfedge = halfedge_q.get_mut(brush_entity)?;
    let mut mesh_faces: Vec<FaceKey> = Vec::with_capacity(sel_faces.len());
    for &face_idx in &sel_faces {
        if let Some(&fk) = halfedge.face_keys.get(face_idx) {
            mesh_faces.push(fk);
        }
    }
    if mesh_faces.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Apply the dissolve and reconcile the brush's faces, topology, and binding.
    // `into_inner` reborrows the change-detected `Mut<Brush>` as `&mut Brush` so
    // the two fields can be borrowed disjointly.
    let brush = brushes.get_mut(brush_entity)?.into_inner();
    apply_topology_edit(
        &mut brush.faces,
        &mut brush.topology,
        &mut halfedge.0,
        |mesh| dissolve_faces(mesh, &mesh_faces),
    )?;

    OperatorResult::Finished
}

pub(crate) fn can_run_dissolve_faces(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face)
        && selection.active_sub().is_some_and(|s| !s.faces.is_empty())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushDissolveFacesOp>();
    // No keybind; operator is available via menu / command palette only for MVP.
}
