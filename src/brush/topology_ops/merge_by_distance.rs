//! `brush.mesh.merge_by_distance` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::apply_topology_edit;
use jackdaw_geometry::halfedge::ops::remove_doubles::remove_doubles;
use jackdaw_jsn::Brush;

use crate::brush::{BrushHalfedge, EditMode};

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
    mut halfedge_q: Query<&mut BrushHalfedge>,
) -> OperatorResult {
    // Check that we're in any brush edit mode.
    if !matches!(*edit_mode, EditMode::BrushEdit(_)) {
        return OperatorResult::Cancelled;
    }

    // Get the currently edited brush entity.
    let brush_entity = selection.active_brush?;

    // Get mutable HalfedgeMesh and run remove_doubles.
    let mut halfedge = halfedge_q.get_mut(brush_entity)?;

    // Weld coincident verts across the whole mesh and reconcile the brush's
    // faces, topology, and binding. `into_inner` reborrows the change-detected
    // `Mut<Brush>` as `&mut Brush` so the two fields can be borrowed disjointly.
    let brush = brushes.get_mut(brush_entity)?.into_inner();
    apply_topology_edit(
        &mut brush.faces,
        &mut brush.topology,
        &mut halfedge.0,
        |mesh| remove_doubles(mesh, DEFAULT_MERGE_DISTANCE),
    )?;

    OperatorResult::Finished
}

pub(crate) fn can_run_merge(edit_mode: Res<EditMode>) -> bool {
    matches!(*edit_mode, EditMode::BrushEdit(_))
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushMergeByDistanceOp>();
    // No keybind; operator is available via menu / command palette only.
}
