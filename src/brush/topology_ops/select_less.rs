//! `brush.select.less` operator. Shrink the selection by removing boundary
//! elements (those that have at least one neighbor not in the selection).

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Shrink the selection by removing elements on its boundary (those with at
/// least one neighbor not in the selection).
#[operator(
    id = "brush.select.less",
    label = "Select Less",
    is_available = can_run_select_less,
    allows_undo = false
)]
pub(crate) fn brush_select_less(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    halfedge_q: Query<&BrushHalfedge>,
) -> OperatorResult {
    let brush_entity = selection.active_brush?;
    let halfedge = halfedge_q.get(brush_entity)?;
    let mesh = &halfedge.mesh;

    match *edit_mode {
        EditMode::BrushEdit(BrushEditMode::Vertex) => {
            let current: Vec<usize> = selection
                .sub(brush_entity)
                .map(|s| s.vertices.clone())
                .unwrap_or_default();
            let result = jackdaw_select::shrink_verts(mesh, &halfedge.vert_keys, &current);
            selection.sub_mut(brush_entity).vertices = result;
            OperatorResult::Finished
        }
        EditMode::BrushEdit(BrushEditMode::Edge) => {
            let current: Vec<(usize, usize)> = selection
                .sub(brush_entity)
                .map(|s| s.edges.clone())
                .unwrap_or_default();
            let result = jackdaw_select::shrink_edges(mesh, &halfedge.vert_keys, &current);
            selection.sub_mut(brush_entity).edges = result;
            OperatorResult::Finished
        }
        EditMode::BrushEdit(BrushEditMode::Face) => {
            let current: Vec<usize> = selection
                .sub(brush_entity)
                .map(|s| s.faces.clone())
                .unwrap_or_default();
            let result = jackdaw_select::shrink_faces(mesh, &halfedge.face_keys, &current);
            selection.sub_mut(brush_entity).faces = result;
            OperatorResult::Finished
        }
        _ => OperatorResult::Cancelled,
    }
}

pub(crate) fn can_run_select_less(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    if !matches!(*edit_mode, EditMode::BrushEdit(_)) {
        return false;
    }
    if selection.active_brush.is_none() {
        return false;
    }
    let Some(sub) = selection.active_sub() else {
        return false;
    };
    match *edit_mode {
        EditMode::BrushEdit(BrushEditMode::Vertex) => !sub.vertices.is_empty(),
        EditMode::BrushEdit(BrushEditMode::Edge) => !sub.edges.is_empty(),
        EditMode::BrushEdit(BrushEditMode::Face) => !sub.faces.is_empty(),
        _ => false,
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushSelectLessOp>();
}
