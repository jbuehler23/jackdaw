//! `brush.select.invert` operator. Replace the current selection with the
//! complement: full set of element keys (in current edit mode) minus current
//! selection. Works in Vertex, Edge, or Face mode.

use bevy::prelude::*;
use jackdaw_api::prelude::*;

use crate::brush::{BrushEditMesh, BrushEditMode, BrushSelection, EditMode};

/// Flip the current selection: select everything that wasn't selected, deselect
/// everything that was. Operates per the current edit mode (Vertex / Edge /
/// Face).
#[operator(
    id = "brush.select.invert",
    label = "Invert Selection",
    is_available = can_run_select_invert,
    allows_undo = false
)]
pub(crate) fn brush_select_invert(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    bmesh_q: Query<&BrushEditMesh>,
) -> OperatorResult {
    let Some(brush_entity) = selection.entity else {
        return OperatorResult::Cancelled;
    };
    let Ok(bmesh_component) = bmesh_q.get(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    match *edit_mode {
        EditMode::BrushEdit(BrushEditMode::Vertex) => {
            let total = bmesh_component.vert_keys.len();
            let current: std::collections::HashSet<usize> =
                selection.vertices.iter().copied().collect();
            selection.vertices = (0..total).filter(|i| !current.contains(i)).collect();
            OperatorResult::Finished
        }
        EditMode::BrushEdit(BrushEditMode::Edge) => {
            // Build the canonical-pair representation for ALL EditMesh edges, then invert.
            let mut all_edges: Vec<(usize, usize)> =
                Vec::with_capacity(bmesh_component.mesh.edges.len());
            // Build VertKey -> idx lookup.
            let mut key_to_idx: std::collections::HashMap<
                jackdaw_geometry::editmesh::VertKey,
                usize,
            > = std::collections::HashMap::with_capacity(bmesh_component.vert_keys.len());
            for (i, &k) in bmesh_component.vert_keys.iter().enumerate() {
                key_to_idx.insert(k, i);
            }
            for (_, edge) in bmesh_component.mesh.edges.iter() {
                let Some(&a) = key_to_idx.get(&edge.v[0]) else {
                    continue;
                };
                let Some(&b) = key_to_idx.get(&edge.v[1]) else {
                    continue;
                };
                let pair = if a < b { (a, b) } else { (b, a) };
                all_edges.push(pair);
            }
            let current: std::collections::HashSet<(usize, usize)> =
                selection.edges.iter().copied().collect();
            selection.edges = all_edges.into_iter().filter(|p| !current.contains(p)).collect();
            OperatorResult::Finished
        }
        EditMode::BrushEdit(BrushEditMode::Face) => {
            let total = bmesh_component.face_keys.len();
            let current: std::collections::HashSet<usize> =
                selection.faces.iter().copied().collect();
            selection.faces = (0..total).filter(|i| !current.contains(i)).collect();
            OperatorResult::Finished
        }
        _ => OperatorResult::Cancelled,
    }
}

pub(crate) fn can_run_select_invert(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    matches!(*edit_mode, EditMode::BrushEdit(_)) && selection.entity.is_some()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushSelectInvertOp>();
}
