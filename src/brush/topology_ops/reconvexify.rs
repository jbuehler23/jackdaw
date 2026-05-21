//! `brush.mesh.reconvexify` operator. Snap the brush back to the convex hull
//! of its current vertices. Useful as an escape hatch when concave editing
//! produced an unwanted result, or as a prerequisite for CSG (which only
//! supports convex inputs in this PR).

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::HalfedgeMesh;
use jackdaw_geometry::{compute_brush_geometry_from_planes, compute_brush_topology};
use jackdaw_jsn::Brush;

use crate::brush::hull::rebuild_brush_from_vertices;
use crate::brush::{BrushHalfedge, BrushSelection};

/// Snap the selected brush back to the convex hull of its current vertices.
/// Useful when concave editing produced an unwanted result, or as a
/// prerequisite for CSG operations (which currently only support convex
/// inputs). Available when a brush is selected.
#[operator(
    id = "brush.mesh.reconvexify",
    label = "Reconvexify",
    is_available = can_run_reconvexify,
    allows_undo = true
)]
pub(crate) fn brush_reconvexify(
    _: In<OperatorParameters>,
    selection: Res<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) -> OperatorResult {
    let Some(brush_entity) = selection.entity else {
        return OperatorResult::Cancelled;
    };
    let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
        return OperatorResult::Cancelled;
    };

    // Collect current vertex positions from the brush's topology.
    // The plane-intersection fallback is a safety net for malformed
    // legacy brushes whose topology never got populated.
    let current_positions: Vec<Vec3> = if !brush_before.topology.vertices.is_empty() {
        brush_before
            .topology
            .vertices
            .iter()
            .map(|v| v.position)
            .collect()
    } else {
        let (verts, _) = compute_brush_geometry_from_planes(&brush_before.faces);
        verts
    };
    if current_positions.len() < 4 {
        return OperatorResult::Cancelled;
    }

    // Build old face_polygons (parallel to faces) for UV-preservation
    // lookup. Same plane-intersection safety net as above.
    let old_face_polygons: Vec<Vec<usize>> = if !brush_before.topology.polygons.is_empty() {
        (0..brush_before.topology.polygons.len())
            .map(|i| {
                brush_before
                    .topology
                    .face_ring(i)
                    .map(|v| v as usize)
                    .collect()
            })
            .collect()
    } else {
        let (_, polys) = compute_brush_geometry_from_planes(&brush_before.faces);
        polys
    };

    // Run Quickhull-based rebuild. Pass the same positions as both old and new:
    // we want the convex hull of the existing vertices, preserving UV data.
    let Some((mut new_brush, _old_to_new)) = rebuild_brush_from_vertices(
        &brush_before,
        &current_positions,
        &old_face_polygons,
        &current_positions,
    ) else {
        return OperatorResult::Cancelled;
    };

    // Populate topology on the new brush by deriving it from its hull planes.
    new_brush.topology = compute_brush_topology(&new_brush.faces);

    // Apply the new brush.
    let Ok(mut brush_mut) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    *brush_mut = new_brush.clone();

    // If an HalfedgeMesh is present (brush is in vertex/edge/face edit mode),
    // re-lift it from the new topology so indices stay consistent.
    if let Ok(mut halfedge) = halfedge_q.get_mut(brush_entity) {
        let new_mesh = HalfedgeMesh::lift_from_topology(&brush_mut.topology);
        let new_vert_keys: Vec<_> = new_mesh.verts.keys().collect();
        let mut new_face_keys = vec![Default::default(); new_mesh.faces.len()];
        for (k, f) in new_mesh.faces.iter() {
            let slot = f.material_idx as usize;
            if slot < new_face_keys.len() {
                new_face_keys[slot] = k;
            }
        }
        halfedge.mesh = new_mesh;
        halfedge.vert_keys = new_vert_keys;
        halfedge.face_keys = new_face_keys;
    }

    OperatorResult::Finished
}

pub(crate) fn can_run_reconvexify(selection: Res<BrushSelection>, brushes: Query<&Brush>) -> bool {
    let Some(brush_entity) = selection.entity else {
        return false;
    };
    brushes.get(brush_entity).is_ok()
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushReconvexifyOp>();
}
