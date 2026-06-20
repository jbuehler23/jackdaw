//! Shared preamble for the per-face UV operators.
//!
//! Every `brush.face.uv.*` operator runs the same setup: require Face edit mode,
//! find the active brush, gather the selected face indices (cancel if none), get
//! the brush mutably, and apply a projection to each selected face. `for_each_selected_face`
//! encapsulates that, calling a per-face closure with the brush topology and the
//! face's data so the closure only has to do the projection math. `can_run`
//! is the matching availability predicate the operators share.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::BrushFaceData;
use jackdaw_jsn::Brush;
use jackdaw_jsn::types::BrushTopology;

use crate::brush::{BrushEditMode, BrushSelection, EditMode};

/// Run `apply` for each selected face of the active brush.
///
/// Cancels when the edit mode is not Face or the face selection is empty, and
/// propagates a missing active brush or brush query miss with `?`. The closure
/// receives each selected face index, the brush topology (for the face normal
/// and ring), and a mutable borrow of that face's data to write the new UV
/// projection onto. Out-of-range indices are skipped.
pub(crate) fn for_each_selected_face(
    edit_mode: &EditMode,
    selection: &BrushSelection,
    brushes: &mut Query<&mut Brush>,
    mut apply: impl FnMut(usize, &BrushTopology, &mut BrushFaceData),
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
    let mut brush = brushes.get_mut(brush_entity)?;
    // Split the borrow so the closure can read topology while mutating a face.
    let Brush { faces, topology } = &mut *brush;

    for face_idx in sel_faces {
        if let Some(face) = faces.get_mut(face_idx) {
            apply(face_idx, topology, face);
        }
    }

    OperatorResult::Finished
}

/// Availability predicate shared by every per-face UV operator: Face edit mode
/// with a non-empty face selection.
pub(crate) fn can_run(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face)
        && selection.active_sub().is_some_and(|s| !s.faces.is_empty())
}
