//! `brush.mesh.edge_slide` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::apply_topology_edit;
use jackdaw_geometry::halfedge::ops::edge_slide::edge_slide;

use super::param_edit::{self, Picks};
use super::select_elements::refuse;
use crate::brush::{BrushEditMode, BrushSelection, EditMode};

const DEFAULT_SLIDE_T: f32 = 0.5;

/// Slide the selected edges along their parallel-edge directions in adjacent
/// quad faces. Pure transform; topology unchanged. Available in Edge mode with
/// selected edges.
#[operator(
    id = "brush.mesh.edge_slide",
    label = "Edge Slide",
    is_available = can_run_edge_slide,
    allows_undo = true,
    params(t(
        f64,
        doc = "How far to slide, as a fraction of the neighbouring parallel edge, between -1 \
               and 1 and not 0; the sign picks the side. Defaults to 0.5."
    ))
)]
pub(crate) fn brush_edge_slide(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let t = match param_edit::number(&params, "t") {
        Ok(t) => t.unwrap_or(DEFAULT_SLIDE_T),
        Err(why) => return refuse(world, "Edge Slide", why),
    };
    if t == 0.0 || t.abs() >= 1.0 {
        return refuse(
            world,
            "Edge Slide",
            format!("`t` {t} must be between -1 and 1, and not 0"),
        );
    }
    param_edit::run(
        world,
        "Edge Slide",
        BrushEditMode::Edge,
        |brush, halfedge, selected| {
            let mesh_edges = param_edit::edge_keys(halfedge, &selected.edges);
            let Ok(moved) = apply_topology_edit(
                &mut brush.faces,
                &mut brush.topology,
                &mut halfedge.0,
                |mesh| edge_slide(mesh, &mesh_edges, t).map(|result| result.moved_verts.len()),
            ) else {
                return Ok(None);
            };
            Ok((moved > 0).then(|| Picks::Edges(selected.edges.clone())))
        },
    )
}

pub(crate) fn can_run_edge_slide(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge)
        && selection.active_sub().is_some_and(|s| !s.edges.is_empty())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushEdgeSlideOp>();
    // No keybind; operator is available via menu / command palette only.
}
