//! `brush.face.uv.rotate_90` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushSelection, EditMode, SetBrush};
use crate::commands::CommandHistory;

/// Rotate the U and V axes 90 degrees counter-clockwise on each selected face.
#[operator(
    id = "brush.face.uv.rotate_90",
    label = "Rotate UV 90°",
    is_available = can_run_uv_rotate_90,
    allows_undo = true
)]
pub(crate) fn brush_uv_rotate_90(
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

    for &face_idx in &selection.faces {
        if let Some(face) = brush.faces.get_mut(face_idx) {
            let old_u = face.uv_u_axis;
            let old_v = face.uv_v_axis;
            face.uv_u_axis = -old_v;
            face.uv_v_axis = old_u;
        }
    }

    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old: brush_before,
        new: brush.clone(),
        label: "Rotate UV 90°".to_string(),
    }));
    OperatorResult::Finished
}

pub(crate) fn can_run_uv_rotate_90(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !selection.faces.is_empty()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvRotate90Op>();
}
