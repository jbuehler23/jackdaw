//! `brush.mesh.weld_selected` operator. Welds all selected verts together
//! at their centroid, regardless of original distance. Use this when you want
//! to "merge these specific verts into one". Different from "Merge by Distance"
//! which only welds verts that are already coincident within a threshold.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::apply_topology_edit;
use jackdaw_geometry::halfedge::ops::remove_doubles::remove_doubles;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

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
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Vertex) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.active_brush?;
    let sel_verts: Vec<usize> = selection
        .sub(brush_entity)
        .map(|s| s.vertices.clone())
        .unwrap_or_default();
    if sel_verts.len() < 2 {
        return OperatorResult::Cancelled;
    }

    let mut halfedge = halfedge_q.get_mut(brush_entity)?;

    // Map cache vertex indices to HalfedgeMesh VertKeys.
    let mut vert_keys = Vec::with_capacity(sel_verts.len());
    for &vi in &sel_verts {
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

    // Weld the coincident verts and reconcile. The tiny threshold merges only
    // the verts just moved together, not distant ones.
    let brush = brushes.get_mut(brush_entity)?.into_inner();
    apply_topology_edit(
        &mut brush.faces,
        &mut brush.topology,
        &mut halfedge.0,
        |mesh| {
            let _ = remove_doubles(mesh, 0.0001);
        },
    );

    OperatorResult::Finished
}

pub(crate) fn can_run_weld(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex)
        && selection
            .active_sub()
            .is_some_and(|s| s.vertices.len() >= 2)
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushWeldSelectedOp>();
}
