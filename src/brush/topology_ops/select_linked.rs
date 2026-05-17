//! `brush.select.linked` operator. Expands the face selection to all faces
//! connected to each selected face via shared edges. SHARP/SEAM edges act
//! as walk blockers, so users can isolate face groups by marking boundaries.

use std::collections::HashSet;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::FaceKey;
use jackdaw_geometry::halfedge::select::linked_walk::linked_walk;

use crate::brush::{BrushHalfedge, BrushEditMode, BrushSelection, EditMode};

/// Expand the face selection to all faces reachable via shared edges from
/// each selected face. Edges flagged SHARP or SEAM act as walk blockers
/// (so a face surrounded by SHARP edges is isolated). Available in Face
/// mode with at least one face selected.
#[operator(
    id = "brush.select.linked",
    label = "Linked Select",
    is_available = can_run_select_linked,
    allows_undo = false
)]
pub(crate) fn brush_select_linked(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    halfedge_q: Query<&BrushHalfedge>,
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
    let Ok(halfedge) = halfedge_q.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    // Map each selected cache face index to its HalfedgeMesh FaceKey.
    let mut mesh_faces: Vec<FaceKey> = Vec::with_capacity(selection.faces.len());
    for &face_idx in &selection.faces {
        if let Some(&fk) = halfedge.face_keys.get(face_idx) {
            mesh_faces.push(fk);
        }
    }
    if mesh_faces.is_empty() {
        return OperatorResult::Cancelled;
    }

    // For each starting face, walk linked components. Union all.
    let mut walked: HashSet<FaceKey> = HashSet::new();
    for fk in mesh_faces {
        for k in linked_walk(&halfedge.mesh, fk, true) {
            walked.insert(k);
        }
    }
    if walked.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Convert FaceKeys back to cache face indices via face_keys.
    let mut new_faces: Vec<usize> = Vec::with_capacity(walked.len());
    for (i, &k) in halfedge.face_keys.iter().enumerate() {
        if walked.contains(&k) {
            new_faces.push(i);
        }
    }

    selection.faces = new_faces;
    OperatorResult::Finished
}

pub(crate) fn can_run_select_linked(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !selection.faces.is_empty()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushSelectLinkedOp>();
}
