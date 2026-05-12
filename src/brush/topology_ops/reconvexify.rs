//! `brush.mesh.reconvexify` operator. Snap the brush back to the convex hull
//! of its current vertices. Useful as an escape hatch when concave editing
//! produced an unwanted result, or as a prerequisite for CSG (which only
//! supports convex inputs in this PR).

use std::collections::HashMap;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::editmesh::EditMesh;
use jackdaw_geometry::{
    compute_brush_geometry_from_planes,
    topology::{BrushTopology, EdgeFlag, MeshEdge, MeshLoop, MeshPoly, MeshVert},
};
use jackdaw_jsn::Brush;

use crate::brush::hull::rebuild_brush_from_vertices;
use crate::brush::{BrushEditMesh, BrushSelection, SetBrush};
use crate::commands::CommandHistory;

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
    mut bmesh_q: Query<&mut BrushEditMesh>,
    mut history: ResMut<CommandHistory>,
) -> OperatorResult {
    let Some(brush_entity) = selection.entity else {
        return OperatorResult::Cancelled;
    };
    let Ok(brush_before) = brushes.get(brush_entity).cloned() else {
        return OperatorResult::Cancelled;
    };

    // Collect current vertex positions from the brush's topology if available;
    // fall back to deriving them from the plane representation for legacy brushes.
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

    // Build old face_polygons (parallel to faces) for UV-preservation lookup.
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
    let (hull_verts, hull_face_polygons) = compute_brush_geometry_from_planes(&new_brush.faces);
    new_brush.topology = build_topology_from_face_polygons(hull_verts, hull_face_polygons);

    // Apply the new brush.
    let Ok(mut brush_mut) = brushes.get_mut(brush_entity) else {
        return OperatorResult::Cancelled;
    };
    *brush_mut = new_brush.clone();

    // If an EditMesh is present (brush is in vertex/edge/face edit mode),
    // re-lift it from the new topology so indices stay consistent.
    if let Ok(mut bmesh_component) = bmesh_q.get_mut(brush_entity) {
        let new_bmesh = EditMesh::lift_from_topology(&brush_mut.topology);
        let new_vert_keys: Vec<_> = new_bmesh.verts.keys().collect();
        let mut new_face_keys = vec![Default::default(); new_bmesh.faces.len()];
        for (k, f) in new_bmesh.faces.iter() {
            let slot = f.material_idx as usize;
            if slot < new_face_keys.len() {
                new_face_keys[slot] = k;
            }
        }
        bmesh_component.mesh = new_bmesh;
        bmesh_component.vert_keys = new_vert_keys;
        bmesh_component.face_keys = new_face_keys;
    }

    history.push_executed(Box::new(SetBrush {
        entity: brush_entity,
        old: brush_before,
        new: new_brush,
        label: "Reconvexify".to_string(),
    }));

    OperatorResult::Finished
}

/// Build a `BrushTopology` from positions and face polygon rings. Mirrors the
/// logic in `topology_migration::derive_topology_from_planes`.
fn build_topology_from_face_polygons(
    positions: Vec<Vec3>,
    face_polygons: Vec<Vec<usize>>,
) -> BrushTopology {
    let vertices: Vec<MeshVert> = positions
        .into_iter()
        .map(|p| MeshVert { position: p })
        .collect();

    let mut edge_map: HashMap<(u32, u32), u32> = HashMap::new();
    let mut edges: Vec<MeshEdge> = Vec::new();
    let mut canonicalize = |a: u32, b: u32| -> u32 {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        if let Some(&idx) = edge_map.get(&(lo, hi)) {
            idx
        } else {
            let idx = edges.len() as u32;
            edges.push(MeshEdge {
                v: [lo, hi],
                flags: EdgeFlag::empty(),
            });
            edge_map.insert((lo, hi), idx);
            idx
        }
    };

    let mut polygons: Vec<MeshPoly> = Vec::with_capacity(face_polygons.len());
    let mut loops: Vec<MeshLoop> = Vec::new();
    for ring in &face_polygons {
        if ring.len() < 3 {
            polygons.push(MeshPoly {
                loop_start: loops.len() as u32,
                loop_total: 0,
            });
            continue;
        }
        let loop_start = loops.len() as u32;
        for i in 0..ring.len() {
            let v_cur = ring[i] as u32;
            let v_next = ring[(i + 1) % ring.len()] as u32;
            let edge_idx = canonicalize(v_cur, v_next);
            loops.push(MeshLoop {
                vert: v_cur,
                edge: edge_idx,
            });
        }
        polygons.push(MeshPoly {
            loop_start,
            loop_total: ring.len() as u32,
        });
    }

    BrushTopology {
        vertices,
        edges,
        polygons,
        loops,
        attributes: Default::default(),
    }
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
