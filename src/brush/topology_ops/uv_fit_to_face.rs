//! `brush.face.uv.fit_to_face` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushSelection, EditMode};

/// Set `uv_scale` so the face spans 0..1 in UV space. The texture covers the
/// face exactly once.
#[operator(
    id = "brush.face.uv.fit_to_face",
    label = "Fit UV to Face",
    is_available = can_run_uv_fit_to_face,
    allows_undo = true
)]
pub(crate) fn brush_uv_fit_to_face(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
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

    let positions: Vec<Vec3> = brush.topology.vertices.iter().map(|v| v.position).collect();

    for &face_idx in &sel_faces {
        if face_idx >= brush.faces.len() {
            continue;
        }
        let ring_positions: Vec<Vec3> = brush
            .topology
            .face_ring(face_idx)
            .map(|i| positions[i as usize])
            .collect();
        if ring_positions.is_empty() {
            continue;
        }

        let u_axis = brush.faces[face_idx].uv_u_axis;
        let v_axis = brush.faces[face_idx].uv_v_axis;

        let mut min_u = f32::INFINITY;
        let mut max_u = f32::NEG_INFINITY;
        let mut min_v = f32::INFINITY;
        let mut max_v = f32::NEG_INFINITY;
        for p in &ring_positions {
            let u = p.dot(u_axis);
            let v = p.dot(v_axis);
            min_u = min_u.min(u);
            max_u = max_u.max(u);
            min_v = min_v.min(v);
            max_v = max_v.max(v);
        }

        let span_u = (max_u - min_u).max(1e-4);
        let span_v = (max_v - min_v).max(1e-4);

        let face = &mut brush.faces[face_idx];
        face.uv_scale = Vec2::new(1.0 / span_u, 1.0 / span_v);
        // UV = world.dot(axis) * scale + offset; want (min * scale + offset) = 0.
        face.uv_offset = Vec2::new(-min_u * face.uv_scale.x, -min_v * face.uv_scale.y);
    }

    OperatorResult::Finished
}

pub(crate) fn can_run_uv_fit_to_face(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face)
        && selection.active_sub().is_some_and(|s| !s.faces.is_empty())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvFitToFaceOp>();
}
