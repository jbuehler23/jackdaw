//! Slide selected edges along the parallel-edge direction of ONE adjacent
//! quad face. The sign of `t` picks which side of the edge to slide toward:
//! `t > 0` uses the first face in the radial cycle, `t < 0` uses the second
//! (when present). `|t|` scales the slide from 0 to the parallel edge's length.
//!
//! Picking a single face per edge (rather than averaging across both adjacent
//! faces) keeps the edge parallel to its original orientation. Averaging
//! across two perpendicular faces (e.g. a cube's top edge between top + front
//! faces) produced a diagonal slide, which is wrong.

use std::collections::{HashMap, HashSet};

use bevy::math::Vec3;

use crate::editmesh::cycles::radial_walk;
use crate::editmesh::types::*;

#[derive(Debug)]
pub enum SlideError {
    EmptyInput,
}

pub struct SlideResult {
    pub moved_verts: Vec<VertKey>,
}

pub fn edge_slide(
    bmesh: &mut EditMesh,
    edges: &[EdgeKey],
    t: f32,
) -> Result<SlideResult, SlideError> {
    if edges.is_empty() {
        return Err(SlideError::EmptyInput);
    }
    if t == 0.0 {
        return Ok(SlideResult {
            moved_verts: Vec::new(),
        });
    }

    let edge_set: HashSet<EdgeKey> = edges.iter().copied().collect();

    // Accumulate per-vert displacement contributions. For a single-edge slide
    // each vert gets one contribution; for a ring slide adjacent ring-edges
    // share a vert and contribute one direction each (averaged below).
    //
    // Ring-consistency note: when sliding a closed loop, the chosen face for
    // each edge (radial[0] vs radial[1]) should be consistent across the
    // ring. Today we pick by sign(t) for each edge independently; if
    // radial walk order varies around the ring, contributions can disagree
    // and the average will skew. That's acceptable for the MVP single-edge
    // case and the loop-cut ring test (which only verifies movement, not
    // direction); a future fix would propagate a face-side hint across
    // ring-adjacent edges.
    let mut vert_dirs: HashMap<VertKey, Vec<Vec3>> = HashMap::new();

    for &edge in edges {
        let quad_loops: Vec<LoopKey> = radial_walk(bmesh, edge)
            .filter(|&lp| bmesh.faces[bmesh.loops[lp].face].loop_count == 4)
            .collect();
        if quad_loops.is_empty() {
            continue;
        }
        let chosen = if t >= 0.0 || quad_loops.len() < 2 {
            quad_loops[0]
        } else {
            quad_loops[1]
        };

        let v_start = bmesh.loops[chosen].vert;
        let v_end = bmesh.loops[bmesh.loops[chosen].next].vert;

        // Slide-along edge for v_start: the edge incident to v_start in this
        // face that isn't `edge`. That's lp.prev.edge.
        let prev_loop = bmesh.loops[chosen].prev;
        let slide_edge_for_start = bmesh.loops[prev_loop].edge;
        if !edge_set.contains(&slide_edge_for_start) {
            let other = {
                let e = &bmesh.edges[slide_edge_for_start];
                if e.v[0] == v_start { e.v[1] } else { e.v[0] }
            };
            let dir = bmesh.verts[other].co - bmesh.verts[v_start].co;
            vert_dirs.entry(v_start).or_default().push(dir);
        }

        // Slide-along edge for v_end: lp.next.edge.
        let next_loop = bmesh.loops[chosen].next;
        let slide_edge_for_end = bmesh.loops[next_loop].edge;
        if !edge_set.contains(&slide_edge_for_end) {
            let other = {
                let e = &bmesh.edges[slide_edge_for_end];
                if e.v[0] == v_end { e.v[1] } else { e.v[0] }
            };
            let dir = bmesh.verts[other].co - bmesh.verts[v_end].co;
            vert_dirs.entry(v_end).or_default().push(dir);
        }
    }

    // Apply averaged offset per vert, scaled by |t|. Sign is already baked
    // into the chosen face above.
    let abs_t = t.abs();
    let mut moved: Vec<VertKey> = Vec::new();
    for (vk, dirs) in &vert_dirs {
        if dirs.is_empty() {
            continue;
        }
        let avg: Vec3 = dirs.iter().copied().sum::<Vec3>() / dirs.len() as f32;
        if avg.length_squared() < 1e-14 {
            continue;
        }
        bmesh.verts[*vk].co += avg * abs_t;
        moved.push(*vk);
    }

    Ok(SlideResult { moved_verts: moved })
}
