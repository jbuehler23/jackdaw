//! Dissolve edges: for each edge, remove it and merge its two adjacent faces.

use crate::bmesh::cycles::{disk_remove_edge, radial_remove_loop, radial_walk};
use crate::bmesh::types::*;

#[derive(Debug)]
pub enum DissolveError {
    EmptyInput,
}

pub struct DissolveEdgesResult {
    pub removed_edges: usize,
    pub removed_faces: usize,
}

pub fn dissolve_edges(
    bmesh: &mut BMesh,
    edges: &[EdgeKey],
) -> Result<DissolveEdgesResult, DissolveError> {
    if edges.is_empty() {
        return Err(DissolveError::EmptyInput);
    }
    let mut removed_edges = 0;
    let mut removed_faces = 0;
    for &edge in edges {
        if dissolve_one_edge(bmesh, edge) {
            removed_edges += 1;
            removed_faces += 1;
        }
    }
    Ok(DissolveEdgesResult { removed_edges, removed_faces })
}

fn dissolve_one_edge(bmesh: &mut BMesh, edge: EdgeKey) -> bool {
    if !bmesh.edges.contains_key(edge) {
        return false;
    }

    // Find the two incident loops via radial cycle.
    let incident_loops: Vec<LoopKey> = radial_walk(bmesh, edge).collect();
    if incident_loops.len() != 2 {
        // boundary (len == 1) or non-manifold (len > 2) — skip.
        return false;
    }

    let lp_a = incident_loops[0];
    let lp_b = incident_loops[1];
    let f_a = bmesh.loops[lp_a].face;
    let f_b = bmesh.loops[lp_b].face;
    if f_a == f_b {
        // Both loops in same face — degenerate, skip.
        return false;
    }

    let lp_a_prev = bmesh.loops[lp_a].prev;
    let lp_a_next = bmesh.loops[lp_a].next;
    let lp_b_prev = bmesh.loops[lp_b].prev;
    let lp_b_next = bmesh.loops[lp_b].next;

    // 1. Walk f_b's ring and reassign all its loops to f_a before splicing,
    //    so we don't have to reason about which side of the splice they land on.
    let f_b_count = bmesh.faces[f_b].loop_count;
    let f_a_count = bmesh.faces[f_a].loop_count;
    {
        let mut cur = bmesh.faces[f_b].loop_first;
        for _ in 0..f_b_count {
            bmesh.loops[cur].face = f_a;
            cur = bmesh.loops[cur].next;
        }
    }

    // 2. Splice the two face rings.
    //    Before: ... lp_a_prev -> lp_a -> lp_a_next ...
    //            ... lp_b_prev -> lp_b -> lp_b_next ...
    //    After:  ... lp_a_prev -> lp_b_next ...
    //            ... lp_b_prev -> lp_a_next ...
    //    (lp_a and lp_b are excised; the two open chains are joined end-to-end.)
    bmesh.loops[lp_a_prev].next = lp_b_next;
    bmesh.loops[lp_b_next].prev = lp_a_prev;
    bmesh.loops[lp_b_prev].next = lp_a_next;
    bmesh.loops[lp_a_next].prev = lp_b_prev;

    // 3. Remove lp_a and lp_b from radial cycles, then drop them.
    radial_remove_loop(bmesh, lp_a);
    radial_remove_loop(bmesh, lp_b);
    bmesh.loops.remove(lp_a);
    bmesh.loops.remove(lp_b);

    // 4. Remove edge from disk cycles, drop it.
    disk_remove_edge(bmesh, edge);
    bmesh.edges.remove(edge);

    // 5. Drop f_b.
    bmesh.faces.remove(f_b);

    // 6. Update f_a: fix loop_first if it pointed at lp_a, update loop_count, re-cache normal.
    let f_a_new_count = f_a_count + f_b_count - 2;
    let f_a_first = if bmesh.faces[f_a].loop_first == lp_a {
        // lp_a was f_a's anchor; pick the loop that now follows it in the merged ring.
        lp_b_next
    } else {
        bmesh.faces[f_a].loop_first
    };
    bmesh.faces[f_a].loop_first = f_a_first;
    bmesh.faces[f_a].loop_count = f_a_new_count;

    // Re-cache normal via Newell's method.
    let mut ring_positions: Vec<bevy::math::Vec3> = Vec::with_capacity(f_a_new_count as usize);
    {
        let mut cur = f_a_first;
        for _ in 0..f_a_new_count {
            ring_positions.push(bmesh.verts[bmesh.loops[cur].vert].co);
            cur = bmesh.loops[cur].next;
        }
    }
    bmesh.faces[f_a].normal_cache = crate::newell_normal(&ring_positions);

    true
}
