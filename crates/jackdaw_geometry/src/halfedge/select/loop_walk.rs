//! Walk the parallel-edge ring around a starting edge, through quad faces.
//! Stops at non-quad faces, boundary edges, or when closing back to start.
//!
//! Useful for "loop select" operators that want to highlight the full ring
//! after the user clicks one edge.

use std::collections::HashSet;

use crate::halfedge::cycles::radial_walk;
use crate::halfedge::types::*;

pub fn loop_walk(mesh: &HalfedgeMesh, start_edge: EdgeKey) -> Vec<EdgeKey> {
    if !mesh.edges.contains_key(start_edge) {
        return Vec::new();
    }

    let mut visited: HashSet<EdgeKey> = HashSet::new();
    let mut result: Vec<EdgeKey> = Vec::new();
    visited.insert(start_edge);
    result.push(start_edge);

    // For each adjacent face on start_edge, walk forward.
    let initial_loops: Vec<LoopKey> = radial_walk(mesh, start_edge).collect();
    for start_loop in initial_loops {
        let mut cur_loop = start_loop;
        loop {
            let face = mesh.loops[cur_loop].face;
            // Stop at non-quad face.
            if mesh.faces[face].loop_count != 4 {
                break;
            }
            // Parallel loop in this quad: cur.next.next.
            let parallel_loop = mesh.loops[mesh.loops[cur_loop].next].next;
            let parallel_edge = mesh.loops[parallel_loop].edge;
            if visited.contains(&parallel_edge) {
                // Closed ring or revisited.
                break;
            }
            visited.insert(parallel_edge);
            result.push(parallel_edge);
            // Cross to neighboring face via radial cycle of parallel_edge.
            let radial_other = mesh.loops[parallel_loop].radial_next;
            if radial_other == parallel_loop {
                // Boundary edge -- terminate this direction.
                break;
            }
            cur_loop = radial_other;
        }
    }

    result
}
