//! `brush.face.uv.texel_density` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::Brush;

use crate::brush::BrushSelection;
use crate::brush::EditMode;
use crate::brush::topology_ops::uv_common::{can_run, for_each_selected_face};

const DEFAULT_TEXEL_DENSITY: f32 = 64.0; // texels per world unit
const ASSUMED_TEXTURE_PIXELS: f32 = 1024.0;

/// Set `uv_scale` so the face has a uniform texel density (default 64 texels per
/// world unit, assuming a 1024 px texture). Useful for keeping a consistent
/// look across brushes of different sizes.
#[operator(
    id = "brush.face.uv.texel_density",
    label = "Set Texel Density",
    is_available = can_run,
    allows_undo = true
)]
pub(crate) fn brush_uv_texel_density(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
) -> OperatorResult {
    let scale = jackdaw_uv::texel_density_scale(DEFAULT_TEXEL_DENSITY, ASSUMED_TEXTURE_PIXELS);

    for_each_selected_face(
        &edit_mode,
        &selection,
        &mut brushes,
        |_face_idx, _topology, face| {
            face.uv_scale = scale;
        },
    )
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushUvTexelDensityOp>();
}
