//! `brush.select.ring` operator. Expands the edge selection by walking the
//! perpendicular-edge ring around each selected edge through quad faces.

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Expand the edge selection by walking the perpendicular-edge ring around each
/// selected edge through quad faces. Stops at non-quad faces or boundary
/// edges. Available in Edge mode with at least one edge selected.
#[operator(
    id = "brush.select.ring",
    label = "Ring Select",
    is_available = can_run_select_ring,
    allows_undo = false
)]
pub(crate) fn brush_select_ring(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    halfedge_q: Query<&BrushHalfedge>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.active_brush?;
    let sel_edges: Vec<(usize, usize)> = selection
        .sub(brush_entity)
        .map(|s| s.edges.clone())
        .unwrap_or_default();
    if sel_edges.is_empty() {
        return OperatorResult::Cancelled;
    }
    let halfedge = halfedge_q.get(brush_entity)?;

    let new_edges = jackdaw_select::ring_edges(&halfedge.mesh, &halfedge.vert_keys, &sel_edges);
    if new_edges.is_empty() {
        return OperatorResult::Cancelled;
    }

    selection.sub_mut(brush_entity).edges = new_edges;
    OperatorResult::Finished
}

pub(crate) fn can_run_select_ring(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge)
        && selection.active_sub().is_some_and(|s| !s.edges.is_empty())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushSelectRingOp>();
}
