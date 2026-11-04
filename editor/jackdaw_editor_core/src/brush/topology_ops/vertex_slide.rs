//! `brush.mesh.vertex_slide` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::apply_topology_edit;
use jackdaw_geometry::halfedge::ops::vertex_slide::vertex_slide;

use super::param_edit::{self, Picks};
use super::select_elements::refuse;
use crate::brush::{BrushEditMode, BrushSelection, EditMode};

const DEFAULT_SLIDE_T: f32 = 0.5;

/// Slide each selected vertex along its first incident edge. Pure transform;
/// topology unchanged. Available in Vertex mode.
#[operator(
    id = "brush.mesh.vertex_slide",
    label = "Vertex Slide",
    is_available = can_run_vertex_slide,
    allows_undo = true,
    params(t(
        f64,
        doc = "How far to slide, as a fraction of the edge, between 0 and 1. Defaults to 0.5."
    ))
)]
pub(crate) fn brush_vertex_slide(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let t = match param_edit::number(&params, "t") {
        Ok(t) => t.unwrap_or(DEFAULT_SLIDE_T),
        Err(why) => return refuse(world, "Vertex Slide", why),
    };
    if t <= 0.0 || t >= 1.0 {
        return refuse(
            world,
            "Vertex Slide",
            format!("`t` {t} must be between 0 and 1"),
        );
    }
    param_edit::run(
        world,
        "Vertex Slide",
        BrushEditMode::Vertex,
        |brush, halfedge, selected| {
            let vert_keys = param_edit::vert_keys(halfedge, &selected.vertices);
            let Ok(moved) = apply_topology_edit(
                &mut brush.faces,
                &mut brush.topology,
                &mut halfedge.0,
                |mesh| vertex_slide(mesh, &vert_keys, t).map(|result| result.moved_verts.len()),
            ) else {
                return Ok(None);
            };
            Ok((moved > 0).then(|| Picks::Vertices(selected.vertices.clone())))
        },
    )
}

pub(crate) fn can_run_vertex_slide(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex)
        && selection
            .active_sub()
            .is_some_and(|s| !s.vertices.is_empty())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushVertexSlideOp>();
    // No keybind; operator is available via menu / command palette only.
}
