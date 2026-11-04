//! `brush.mesh.connect_verts` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::connect_verts::connect_verts;
use jackdaw_geometry::halfedge::{VertKey, apply_topology_edit};
use jackdaw_scene_types::Brush;

use super::loop_cut::topology_pairs;
use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Connect selected verts in the same face with new edges, splitting the face.
/// Operates on the current vertex selection. Available in Vertex mode with 2+ verts selected.
#[operator(
    id = "brush.mesh.connect_verts",
    label = "Connect Vertex Path",
    is_available = can_run_connect_verts,
    allows_undo = true
)]
pub(crate) fn brush_connect_verts(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) -> Option<OperatorResult> {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Vertex) {
        return None;
    }
    let brush_entity = selection.active_brush?;
    let sel_verts: Vec<usize> = selection
        .sub(brush_entity)
        .map(|s| s.vertices.clone())
        .unwrap_or_default();
    if sel_verts.len() < 2 {
        return None;
    }

    // Map cache vertex indices to HalfedgeMesh VertKeys via vert_keys parallel array.
    let mut halfedge = halfedge_q.get_mut(brush_entity).ok()?;
    let mut vert_keys: Vec<VertKey> = Vec::with_capacity(sel_verts.len());
    for &vert_idx in &sel_verts {
        if let Some(&vk) = halfedge.vert_keys.get(vert_idx) {
            vert_keys.push(vk);
        }
    }
    if vert_keys.len() < 2 {
        return None;
    }

    // Connect the verts and reconcile, capturing the topology vertex index pair
    // for each new edge so the post-commit selection can target them. The pairs
    // are read from the mesh before reconcile; topology vertex order matches the
    // mesh slotmap order (see `flatten_to_topology`) and connect_verts never
    // removes verts. `into_inner` reborrows the change-detected `Mut<Brush>` as
    // `&mut Brush` so the two fields can be borrowed disjointly.
    let brush = brushes.get_mut(brush_entity).ok()?.into_inner();
    let new_edge_pairs: Vec<(usize, usize)> = apply_topology_edit(
        &mut brush.faces,
        &mut brush.topology,
        &mut halfedge.0,
        |mesh| match connect_verts(mesh, &vert_keys) {
            Ok(result) => topology_pairs(mesh, &result.new_edges),
            Err(_) => Vec::new(),
        },
    );

    // Chain selection: write the newly created connecting edges into
    // `BrushSelection.edges` so the user can immediately act on them
    // (e.g. switch to Edge mode and loop-cut / slide).
    let vert_count = brush.topology.vertices.len();
    let inbounds: Vec<(usize, usize)> = new_edge_pairs
        .into_iter()
        .filter(|(a, b)| *a < vert_count && *b < vert_count)
        .collect();
    if !inbounds.is_empty() {
        selection.sub_mut(brush_entity).edges = inbounds;
    }

    Some(OperatorResult::Finished)
}

pub(crate) fn can_run_connect_verts(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex)
        && selection
            .active_sub()
            .is_some_and(|s| s.vertices.len() >= 2)
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushConnectVertsOp>();
    // No keybind; operator is available via menu / command palette only.
}
