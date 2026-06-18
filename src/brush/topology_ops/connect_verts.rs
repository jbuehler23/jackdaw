//! `brush.mesh.connect_verts` operator.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::ops::connect_verts::connect_verts;
use jackdaw_geometry::halfedge::{VertKey, apply_topology_edit};
use jackdaw_jsn::Brush;

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Connect selected verts in the same face with new edges, splitting the face.
/// Operates on the current vertex selection. Available in Vertex mode with 2+ verts selected.
#[operator(
    id = "brush.mesh.connect_verts",
    label = "Connect Vertex Path",
    is_available = can_run_connect_verts,
    allows_undo = true
)]
pub(crate) fn brush_connect_verts(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    mut brushes: Query<&mut Brush>,
    mut halfedge_q: Query<&mut BrushHalfedge>,
) -> OperatorResult {
    if *edit_mode != EditMode::BrushEdit(BrushEditMode::Vertex) {
        return OperatorResult::Cancelled;
    }
    let brush_entity = selection.active_brush?;
    let sel_verts: Vec<usize> = selection
        .sub(brush_entity)
        .map(|s| s.vertices.clone())
        .unwrap_or_default();
    if sel_verts.len() < 2 {
        return OperatorResult::Cancelled;
    }

    // Map cache vertex indices to HalfedgeMesh VertKeys via vert_keys parallel array.
    let mut halfedge = halfedge_q.get_mut(brush_entity)?;
    let mut vert_keys: Vec<VertKey> = Vec::with_capacity(sel_verts.len());
    for &vert_idx in &sel_verts {
        if let Some(&vk) = halfedge.vert_keys.get(vert_idx) {
            vert_keys.push(vk);
        }
    }
    if vert_keys.len() < 2 {
        return OperatorResult::Cancelled;
    }

    // Connect the verts and reconcile, capturing the topology vertex index pair
    // for each new edge so the post-commit selection can target them. The pairs
    // are read from the mesh before reconcile; topology vertex order matches the
    // mesh slotmap order (see `flatten_to_topology`) and connect_verts never
    // removes verts. `into_inner` reborrows the change-detected `Mut<Brush>` as
    // `&mut Brush` so the two fields can be borrowed disjointly.
    let brush = brushes.get_mut(brush_entity)?.into_inner();
    let source = brush.faces.last().cloned().unwrap_or_default();
    let original_face_count = brush.faces.len();
    let new_edge_pairs: Vec<(usize, usize)> = apply_topology_edit(
        &mut brush.faces,
        &mut brush.topology,
        &mut halfedge.0,
        |mesh| match connect_verts(mesh, &vert_keys) {
            Ok(r) => {
                let mut vk_to_topo: std::collections::HashMap<VertKey, usize> =
                    std::collections::HashMap::with_capacity(mesh.verts.len());
                for (i, (k, _)) in mesh.verts.iter().enumerate() {
                    vk_to_topo.insert(k, i);
                }
                let mut out: Vec<(usize, usize)> = Vec::with_capacity(r.new_edges.len());
                for ek in &r.new_edges {
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
            }
            Err(_) => Vec::new(),
        },
    );

    // Any face the split created inherits the previous last face's appearance.
    for new_face in original_face_count..brush.faces.len() {
        brush.faces[new_face].copy_appearance_from(&source);
        brush.faces[new_face].ensure_uv_axes();
    }
    // Chain selection: write the newly created connecting edges into
    // `BrushSelection.edges` so the user can immediately act on them
    // (e.g. switch to Edge mode and loop-cut / slide).
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

pub(crate) fn can_run_connect_verts(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    *edit_mode == EditMode::BrushEdit(BrushEditMode::Vertex)
        && selection
            .active_sub()
            .is_some_and(|s| s.vertices.len() >= 2)
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushConnectVertsOp>();
    // No keybind; operator is available via menu / command palette only for MVP.
}
