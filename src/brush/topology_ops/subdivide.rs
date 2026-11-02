//! `brush.mesh.subdivide` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::apply_topology_edit;
use jackdaw_geometry::halfedge::ops::subdivide::subdivide_cuts;

use super::loop_cut::topology_pairs;
use super::param_edit::{self, Picks};
use super::select_elements::refuse;
use crate::brush::{BrushEditMode, BrushSelection, EditMode};

/// The most points a subdivide places on each edge.
const MAX_CUTS: u32 = 32;

/// Split each selected edge into equal pieces and re-tessellate touched faces.
/// Operates on the current edge selection and leaves the new cross edges
/// selected. No modal interaction. Available in Edge mode with at least one
/// edge selected.
#[operator(
    id = "brush.mesh.subdivide",
    label = "Subdivide",
    is_available = can_run_subdivide,
    allows_undo = true,
    params(cuts(
        i64,
        doc = "How many evenly spaced points to put on each selected edge, 1 to 32. With \
               more than one, a quad cut on two opposite sides is split into strips. \
               Defaults to 1, the midpoint."
    ))
)]
pub(crate) fn brush_subdivide(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let cuts = match param_edit::count(&params, "cuts", MAX_CUTS) {
        Ok(cuts) => cuts.unwrap_or(1),
        Err(why) => return refuse(world, "Subdivide", why),
    };
    param_edit::run(
        world,
        "Subdivide",
        BrushEditMode::Edge,
        |brush, halfedge, selected| {
            let mesh_edges = param_edit::edge_keys(halfedge, &selected.edges);
            if mesh_edges.is_empty() {
                return Ok(None);
            }
            // The pairs are read before reconcile; subdivide never removes
            // verts, so slot order is the flattened vertex order.
            let Ok(new_edge_pairs) = apply_topology_edit(
                &mut brush.faces,
                &mut brush.topology,
                &mut halfedge.0,
                |mesh| {
                    subdivide_cuts(mesh, &mesh_edges, cuts)
                        .map(|result| topology_pairs(mesh, &result.new_edges))
                },
            ) else {
                return Ok(None);
            };
            let vert_count = brush.topology.vertices.len();
            Ok(Some(Picks::Edges(
                new_edge_pairs
                    .into_iter()
                    .filter(|(a, b)| *a < vert_count && *b < vert_count)
                    .collect(),
            )))
        },
    )
}

pub(crate) fn can_run_subdivide(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge)
        && selection.active_sub().is_some_and(|s| !s.edges.is_empty())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushSubdivideOp>();
    // No keybind; operator is available via menu / command palette only.
}
