//! `brush.face.uv.align_to_edge` operator.

use bevy_ecs::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;

/// Rotate UV axes so the U direction aligns with a selected edge of the face.
/// Useful for getting a texture's grain to follow a particular feature edge.
/// Prefers a selected edge that belongs to this face; falls back to the face's
/// first ring edge if none is selected.
#[operator(
    id = "brush.face.uv.align_to_edge",
    label = "Align UV to Edge",
    is_available = can_run_uv_align_to_edge,
    allows_undo = true
)]
pub(crate) fn brush_uv_align_to_edge(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut history: ResMut<CommandHistory>,
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
    let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
        return OperatorResult::Cancelled;
    };
    let Ok(mut brush) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };

    let selected_edges = selection.edges.clone();

    for &face_idx in &selection.faces {
        if face_idx >= brush.faces.len() {
            continue;
        }
        let normal = brush.topology.face_normal(face_idx);
        let ring: Vec<u32> = brush.topology.face_ring(face_idx).collect();
        if ring.len() < 2 {
            continue;
        }

        // Find a selected edge whose both endpoints are in this face's ring.
        let mut target_edge: Option<(usize, usize)> = None;
        for &(a, b) in &selected_edges {
            if ring.contains(&(a as u32)) && ring.contains(&(b as u32)) {
                target_edge = Some((a, b));
                break;
            }
        }
        let (a_idx, b_idx) = target_edge.unwrap_or_else(|| (ring[0] as usize, ring[1] as usize));

        let a_pos = brush.topology.vertices[a_idx].position;
        let b_pos = brush.topology.vertices[b_idx].position;
        let edge_dir = b_pos - a_pos;

        // Project onto face plane, then normalize.
        let edge_dir_planar = (edge_dir - normal * edge_dir.dot(normal)).normalize_or_zero();
        if edge_dir_planar.length_squared() > 0.5 {
            let face = &mut brush.faces[face_idx];
            face.uv_u_axis = edge_dir_planar;
            face.uv_v_axis = normal.cross(edge_dir_planar).normalize_or_zero();
        }
    }

    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old: brush_before,
        new: brush.clone(),
        label: "Align UV to Edge".to_string(),
    }));
    OperatorResult::Finished
}

pub(crate) fn can_run_uv_align_to_edge(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !selection.faces.is_empty()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvAlignToEdgeOp>();
}
