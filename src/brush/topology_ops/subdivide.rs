//! `brush.mesh.subdivide` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::subdivide::subdivide;
use jackdaw_geometry::halfedge::{EdgeKey, HalfedgeMesh, VertKey, apply_topology_edit};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Split each selected edge at its midpoint and re-tessellate touched faces.
/// Operates on the current edge selection. No modal interaction. Available
/// in Edge mode with at least one edge selected.
#[operator(
    id = "brush.mesh.subdivide",
    label = "Subdivide",
    is_available = can_run_subdivide,
    allows_undo = true
)]
pub(crate) fn brush_subdivide(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Edge) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.active_brush?;
    let sel_edges: Vec<(usize, usize)> = selection
        .sub(brush_entity)
        .map(|s| s.edges.clone())
        .unwrap_or_default();
    if sel_edges.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Map each selected cache-edge (a, b) to a HalfedgeMesh EdgeKey via vert_keys.
    let mut halfedge = halfedge_q.get_mut(brush_entity)?;
    let mut mesh_edges: Vec<EdgeKey> = Vec::with_capacity(sel_edges.len());
    for &(a, b) in &sel_edges {
        let Some(&va) = halfedge.vert_keys.get(a) else {
            continue;
        };
        let Some(&vb) = halfedge.vert_keys.get(b) else {
            continue;
        };
        if let Some(ek) = find_edge_between(&halfedge.mesh, va, vb) {
            mesh_edges.push(ek);
        }
    }
    if mesh_edges.is_empty() {
        return OperatorResult::Cancelled;
    }

    // Subdivide the edges and reconcile, capturing the topology vertex index
    // pair for each new cross-cut edge so the post-commit selection can target
    // them. The pairs are read from the mesh before reconcile; topology vertex
    // order matches the mesh slotmap order (see `flatten_to_topology`) and
    // subdivide never removes verts, so the slot positions are stable here.
    // `into_inner` reborrows the change-detected `Mut<Brush>` as `&mut Brush` so
    // the two fields can be borrowed disjointly.
    let brush = brushes.get_mut(brush_entity)?.into_inner();
    let source = brush.faces.last().cloned().unwrap_or_default();
    let original_face_count = brush.faces.len();
    let new_edge_pairs: Vec<(usize, usize)> = apply_topology_edit(
        &mut brush.faces,
        &mut brush.topology,
        &mut halfedge.0,
        |mesh| {
            subdivide(mesh, &mesh_edges).map(|subdivide_result| {
                let mut vk_to_topo: std::collections::HashMap<VertKey, usize> =
                    std::collections::HashMap::with_capacity(mesh.verts.len());
                for (i, (k, _)) in mesh.verts.iter().enumerate() {
                    vk_to_topo.insert(k, i);
                }
                let mut out: Vec<(usize, usize)> =
                    Vec::with_capacity(subdivide_result.new_edges.len());
                for ek in &subdivide_result.new_edges {
                    let Some(edge) = mesh.edges.get(*ek) else {
                        continue;
                    };
                    let Some(&a) = vk_to_topo.get(&edge.v[0]) else {
                        continue;
                    };
                    let Some(&b) = vk_to_topo.get(&edge.v[1]) else {
                        continue;
                    };
                    let pair = if a < b { (a, b) } else { (b, a) };
                    if !out.contains(&pair) {
                        out.push(pair);
                    }
                }
                out
            })
        },
    )?;

    // Any face the subdivision created inherits the previous last face's appearance.
    for new_face in original_face_count..brush.faces.len() {
        brush.faces[new_face].copy_appearance_from(&source);
        brush.faces[new_face].ensure_uv_axes();
    }
    // Chain selection: write the new cross-cut edges into `BrushSelection.edges`
    // so a follow-up gesture (loop cut, edge slide, subdivide again) operates
    // on the freshly created geometry.
    let vert_count = brush.topology.vertices.len();
    let inbounds: Vec<(usize, usize)> = new_edge_pairs
        .into_iter()
        .filter(|(a, b)| *a < vert_count && *b < vert_count)
        .collect();
    if !inbounds.is_empty() {
        selection.sub_mut(brush_entity).edges = inbounds;
    }

    OperatorResult::Finished
}

fn find_edge_between(mesh: &HalfedgeMesh, va: VertKey, vb: VertKey) -> Option<EdgeKey> {
    mesh.edges
        .iter()
        .find(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
        .map(|(k, _)| k)
}

pub(crate) fn can_run_subdivide(edit_mode: Res<EditMode>, selection: Res<BrushSelection>) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Edge)
        && selection.active_sub().is_some_and(|s| !s.edges.is_empty())
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushSubdivideOp>();
    // No keybind; operator is available via menu / command palette only.
}
