//! `brush.select.less` operator. Shrink the selection by removing boundary
//! elements (those that have at least one neighbor not in the selection).

use std::collections::HashSet;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_geometry::halfedge::VertKey;
use jackdaw_geometry::halfedge::cycles::{disk_walk, radial_walk};

use crate::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// Shrink the selection by removing elements on its boundary (those with at
/// least one neighbor not in the selection).
#[operator(
    id = "brush.select.less",
    label = "Select Less",
    is_available = can_run_select_less,
    allows_undo = false
)]
pub(crate) fn brush_select_less(
    _: In<OperatorParameters>,
    edit_mode: Res<EditMode>,
    mut selection: ResMut<BrushSelection>,
    halfedge_q: Query<&BrushHalfedge>,
) -> OperatorResult {
    let brush_entity = selection.entity?;
    let halfedge = halfedge_q.get(brush_entity)?;
    let mesh = &halfedge.mesh;

    match *edit_mode {
        EditMode::BrushEdit(BrushEditMode::Vertex) => {
            let current: HashSet<usize> = selection.vertices.iter().copied().collect();
            // Keep only verts whose ALL neighbors are also in the selection.
            let kept: Vec<usize> = current
                .iter()
                .copied()
                .filter(|&vi| {
                    let Some(&vk) = halfedge.vert_keys.get(vi) else {
                        return false;
                    };
                    let mut all_inside = true;
                    for ek in disk_walk(mesh, vk).collect::<Vec<_>>() {
                        let edge = &mesh.edges[ek];
                        let other = if edge.v[0] == vk {
                            edge.v[1]
                        } else {
                            edge.v[0]
                        };
                        if let Some(other_idx) = halfedge.vert_keys.iter().position(|&k| k == other)
                            && !current.contains(&other_idx)
                        {
                            all_inside = false;
                            break;
                        }
                    }
                    all_inside
                })
                .collect();
            let mut sorted = kept;
            sorted.sort();
            selection.vertices = sorted;
            OperatorResult::Finished
        }
        EditMode::BrushEdit(BrushEditMode::Edge) => {
            let current: HashSet<(usize, usize)> = selection.edges.iter().copied().collect();
            let mut key_to_idx: std::collections::HashMap<VertKey, usize> =
                std::collections::HashMap::new();
            for (i, &k) in halfedge.vert_keys.iter().enumerate() {
                key_to_idx.insert(k, i);
            }
            // An edge is "interior" if all edges sharing a vert with it are also selected.
            let kept: Vec<(usize, usize)> = current
                .iter()
                .copied()
                .filter(|&(a, b)| {
                    let Some(&va) = halfedge.vert_keys.get(a) else {
                        return false;
                    };
                    let Some(&vb) = halfedge.vert_keys.get(b) else {
                        return false;
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
                            if !current.contains(&pair) {
                                return false;
                            }
                        }
                    }
                    true
                })
                .collect();
            selection.edges = kept;
            OperatorResult::Finished
        }
        EditMode::BrushEdit(BrushEditMode::Face) => {
            let current: HashSet<usize> = selection.faces.iter().copied().collect();
            let kept: Vec<usize> = current
                .iter()
                .copied()
                .filter(|&fi| {
                    let Some(&fk) = halfedge.face_keys.get(fi) else {
                        return false;
                    };
                    let face_data = &mesh.faces[fk];
                    let mut all_inside = true;
                    let mut cur = face_data.loop_first;
                    for _ in 0..face_data.loop_count {
                        let edge = mesh.loops[cur].edge;
                        for radial_lp in radial_walk(mesh, edge).collect::<Vec<_>>() {
                            let neighbor = mesh.loops[radial_lp].face;
                            if let Some(neighbor_idx) =
                                halfedge.face_keys.iter().position(|&k| k == neighbor)
                                && !current.contains(&neighbor_idx)
                            {
                                all_inside = false;
                                break;
                            }
                        }
                        if !all_inside {
                            break;
                        }
                        cur = mesh.loops[cur].next;
                    }
                    all_inside
                })
                .collect();
            let mut sorted = kept;
            sorted.sort();
            selection.faces = sorted;
            OperatorResult::Finished
        }
        _ => OperatorResult::Cancelled,
    }
}

pub(crate) fn can_run_select_less(
    edit_mode: Res<EditMode>,
    selection: Res<BrushSelection>,
) -> bool {
    if !matches!(*edit_mode, EditMode::BrushEdit(_)) {
        return false;
    }
    if selection.entity.is_none() {
        return false;
    }
    match *edit_mode {
        EditMode::BrushEdit(BrushEditMode::Vertex) => !selection.vertices.is_empty(),
        EditMode::BrushEdit(BrushEditMode::Edge) => !selection.edges.is_empty(),
        EditMode::BrushEdit(BrushEditMode::Face) => !selection.faces.is_empty(),
        _ => false,
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushSelectLessOp>();
}
