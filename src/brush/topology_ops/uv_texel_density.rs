//! `brush.face.uv.texel_density` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushSelection, EditMode};

const DEFAULT_TEXEL_DENSITY: f32 = 64.0; // texels per world unit
const ASSUMED_TEXTURE_PIXELS: f32 = 1024.0;

/// Set `uv_scale` so the face has a uniform texel density (default 64 texels per
/// world unit, assuming a 1024 px texture). Useful for keeping a consistent
/// look across brushes of different sizes.
#[operator(
    id = "brush.face.uv.texel_density",
    label = "Set Texel Density",
    is_available = can_run_uv_texel_density,
    allows_undo = true
)]
pub(crate) fn brush_uv_texel_density(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Face) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.entity?;
    if selection.faces.is_empty() {
        return OperatorResult::Cancelled;
    }
    let mut brush = brushes.get_mut(brush_entity)?;

    let scale = DEFAULT_TEXEL_DENSITY / ASSUMED_TEXTURE_PIXELS;

    for &face_idx in &selection.faces {
        if let Some(face) = brush.faces.get_mut(face_idx) {
            face.uv_scale = Vec2::new(scale, scale);
        }
    }

    OperatorResult::Finished
}

pub(crate) fn can_run_uv_texel_density(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Face) && !selection.faces.is_empty()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvTexelDensityOp>();
}
