//! `brush.select.more` operator. Extend the selection to its neighbors.
//! Vertex mode: add verts sharing an edge with selected. Edge mode: add edges
//! sharing a vert with selected. Face mode: add faces sharing an edge with selected.

use std::collections::HashSet;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::VertKey;
use jackdaw_geometry::halfedge::cycles::{disk_walk, radial_walk};

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Extend the selection to its immediate neighbors based on the current edit mode.
#[operator(
    id = "brush.select.more",
    label = "Select More",
    is_available = can_run_select_more,
    allows_undo = false
)]
pub(crate) fn brush_select_more(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    halfedge_q: Query<&BrushHalfedge>,
) -> OperatorResult {
    let brush_entity = selection.active_brush?;
    let halfedge = halfedge_q.get(brush_entity)?;
    let mesh = &halfedge.mesh;

    match *edit_mode {
        EditMode::BrushEdit(BrushEditMode::Vertex) => {
            let current: Vec<usize> = selection
                .sub(brush_entity)
                .map(|s| s.vertices.clone())
                .unwrap_or_default();
            let mut new_set: HashSet<usize> = current.iter().copied().collect();
            for vi in &current {
                let Some(&vk) = halfedge.vert_keys.get(*vi) else {
                    continue;
                };
                for ek in disk_walk(mesh, vk).collect::<Vec<_>>() {
                    let edge = &mesh.edges[ek];
                    let other = if edge.v[0] == vk {
                        edge.v[1]
                    } else {
                        edge.v[0]
                    };
                    if let Some(other_idx) = halfedge.vert_keys.iter().position(|&k| k == other) {
                        new_set.insert(other_idx);
                    }
                }
            }
            let mut result: Vec<usize> = new_set.into_iter().collect();
            result.sort();
            selection.sub_mut(brush_entity).vertices = result;
            OperatorResult::Finished
        }
        EditMode::BrushEdit(BrushEditMode::Edge) => {
            let current: Vec<(usize, usize)> = selection
                .sub(brush_entity)
                .map(|s| s.edges.clone())
                .unwrap_or_default();
            let mut new_set: HashSet<(usize, usize)> = current.iter().copied().collect();
            let mut key_to_idx: std::collections::HashMap<VertKey, usize> =
                std::collections::HashMap::new();
            for (i, &k) in halfedge.vert_keys.iter().enumerate() {
                key_to_idx.insert(k, i);
            }
            for (a, b) in &current {
                let Some(&va) = halfedge.vert_keys.get(*a) else {
                    continue;
                };
                let Some(&vb) = halfedge.vert_keys.get(*b) else {
                    continue;
                };
                for vk in [va, vb] {
                    for ek in disk_walk(mesh, vk).collect::<Vec<_>>() {
                        let edge = &mesh.edges[ek];
                        let Some(&i0) = key_to_idx.get(&edge.v[0]) else {
                            continue;
                        };
                        let Some(&i1) = key_to_idx.get(&edge.v[1]) else {
                            continue;
                        };
                        let pair = if i0 < i1 { (i0, i1) } else { (i1, i0) };
                        new_set.insert(pair);
                    }
                }
            }
            selection.sub_mut(brush_entity).edges = new_set.into_iter().collect();
            OperatorResult::Finished
        }
        EditMode::BrushEdit(BrushEditMode::Face) => {
            let current: Vec<usize> = selection
                .sub(brush_entity)
                .map(|s| s.faces.clone())
                .unwrap_or_default();
            let mut new_set: HashSet<usize> = current.iter().copied().collect();
            for fi in &current {
                let Some(&fk) = halfedge.face_keys.get(*fi) else {
                    continue;
                };
                let face_data = &mesh.faces[fk];
                let mut cur = face_data.loop_first;
                for _ in 0..face_data.loop_count {
                    let edge = mesh.loops[cur].edge;
                    for radial_lp in radial_walk(mesh, edge).collect::<Vec<_>>() {
                        let neighbor = mesh.loops[radial_lp].face;
                        if let Some(neighbor_idx) =
                            halfedge.face_keys.iter().position(|&k| k == neighbor)
                        {
                            new_set.insert(neighbor_idx);
                        }
                    }
                    cur = mesh.loops[cur].next;
                }
            }
            let mut result: Vec<usize> = new_set.into_iter().collect();
            result.sort();
            selection.sub_mut(brush_entity).faces = result;
            OperatorResult::Finished
        }
        _ => OperatorResult::Cancelled,
    }
}

pub(crate) fn can_run_select_more(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    if !matches!(*edit_mode, EditMode::BrushEdit(_)) {
        return false;
    }
    if selection.active_brush.is_none() {
        return false;
    }
    let Some(sub) = selection.active_sub() else {
        return false;
    };
    match *edit_mode {
        EditMode::BrushEdit(BrushEditMode::Vertex) => !sub.vertices.is_empty(),
        EditMode::BrushEdit(BrushEditMode::Edge) => !sub.edges.is_empty(),
        EditMode::BrushEdit(BrushEditMode::Face) => !sub.faces.is_empty(),
        _ => false,
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushSelectMoreOp>();
}
