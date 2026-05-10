//! Slide selected edges along their parallel-edge directions in adjacent quad
//! faces. Pure transform: no topology change, just vertex position updates.
//!
//! For each endpoint of a selected edge, gather all "slide-along" directions
//! from adjacent quad faces, average them, and translate the vertex by
//! `t * slide_length * average_dir`.

use std::collections::{HashMap, HashSet};

use bevy::math::Vec3;

use crate::bmesh::cycles::radial_walk;
use crate::bmesh::types::*;

#[derive(Debug)]
pub enum SlideError {
    EmptyInput,
}

pub struct SlideResult {
    pub moved_verts: Vec<VertKey>,
}

pub fn edge_slide(bmesh: &mut BMesh, edges: &[EdgeKey], t: f32) -> Result<SlideResult, SlideError> {
    if edges.is_empty() {
        return Err(SlideError::EmptyInput);
    }
    if t == 0.0 {
        return Ok(SlideResult { moved_verts: Vec::new() });
    }

    // For each affected vertex, gather per-face slide-along directions. Each
    // direction is stored as the raw displacement vector (from v toward the
    // slide-along neighbor). We collect all contributions, then resolve them
    // into a single offset per vertex before applying.
    //
    // When two adjacent faces give opposite directions (the common case for a
    // ring-cut between two quads), a naive average cancels to zero. We detect
    // this by checking whether the sum's magnitude is much smaller than the
    // average individual magnitude; if so, we use the first direction seen as
    // the canonical slide direction.
    let mut vert_dirs: HashMap<VertKey, Vec<Vec3>> = HashMap::new();
    let edge_set: HashSet<EdgeKey> = edges.iter().copied().collect();

    for &edge in edges {
        // For each loop on this edge in each face:
        for lp_key in radial_walk(bmesh, edge).collect::<Vec<_>>() {
            let face = bmesh.loops[lp_key].face;
            // Only quads contribute slide directions in this MVP.
            if bmesh.faces[face].loop_count != 4 {
                continue;
            }
            // The loop walks v_start -> v_end. v_start = lp_key.vert.
            let v_start = bmesh.loops[lp_key].vert;
            let v_end = bmesh.loops[bmesh.loops[lp_key].next].vert;

            // Slide-along edge for v_start: the edge incident to v_start in
            // this face that is NOT `edge`. That is lp_key.prev.edge.
            let prev_loop = bmesh.loops[lp_key].prev;
            let slide_edge_for_start = bmesh.loops[prev_loop].edge;
            // Skip if the slide-along edge is also selected (would compound badly).
            if !edge_set.contains(&slide_edge_for_start) {
                let other_vert_for_start = {
                    let e = &bmesh.edges[slide_edge_for_start];
                    if e.v[0] == v_start { e.v[1] } else { e.v[0] }
                };
                let dir = bmesh.verts[other_vert_for_start].co - bmesh.verts[v_start].co;
                vert_dirs.entry(v_start).or_default().push(dir);
            }

            // Slide-along edge for v_end: lp_key.next.edge.
            let next_loop = bmesh.loops[lp_key].next;
            let slide_edge_for_end = bmesh.loops[next_loop].edge;
            if !edge_set.contains(&slide_edge_for_end) {
                let other_vert_for_end = {
                    let e = &bmesh.edges[slide_edge_for_end];
                    if e.v[0] == v_end { e.v[1] } else { e.v[0] }
                };
                let dir = bmesh.verts[other_vert_for_end].co - bmesh.verts[v_end].co;
                vert_dirs.entry(v_end).or_default().push(dir);
            }
        }
    }

    // Resolve each vertex's collected directions into a single offset.
    //
    // Strategy: sum all directions. If the sum's length is at least 10% of
    // the average individual length, use the sum (divided by count) as the
    // average. Otherwise the directions cancel (opposite faces case); use the
    // first direction seen as the canonical slide.
    let mut vert_offset: HashMap<VertKey, Vec3> = HashMap::new();
    for (vk, dirs) in &vert_dirs {
        if dirs.is_empty() {
            continue;
        }
        let sum: Vec3 = dirs.iter().copied().sum();
        let avg_individual_len: f32 = dirs.iter().map(|d| d.length()).sum::<f32>() / dirs.len() as f32;
        let offset = if avg_individual_len < 1e-7 {
            Vec3::ZERO
        } else if sum.length() >= 0.1 * avg_individual_len {
            // Directions mostly agree: use the averaged sum.
            sum / dirs.len() as f32
        } else {
            // Directions cancel (e.g., opposite faces). Use the first direction
            // seen as the canonical slide side.
            dirs[0]
        };
        vert_offset.insert(*vk, offset);
    }

    // Apply offsets scaled by t.
    let mut moved: Vec<VertKey> = Vec::new();
    for (vk, offset) in vert_offset.iter() {
        if offset.length_squared() < 1e-14 {
            continue;
        }
        bmesh.verts[*vk].co += *offset * t;
        moved.push(*vk);
    }

    Ok(SlideResult { moved_verts: moved })
}
