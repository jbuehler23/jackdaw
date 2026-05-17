//! Walk the perpendicular-edge ring around a starting edge, through quad faces.
//! Different from `loop_walk`: collects `next` (perpendicular) edges in each face
//! while crossing via `next.next` (parallel) edges, the same traversal path as
//! `loop_walk` but recording the adjacent perpendicular edge at each step.

use std::collections::HashSet;

use crate::halfedge::cycles::radial_walk;
use crate::halfedge::types::*;

pub fn ring_walk(mesh: &HalfedgeMesh, start_edge: EdgeKey) -> Vec<EdgeKey> {
    if !mesh.edges.contains_key(start_edge) {
        return Vec::new();
    }

    let mut visited_parallel: HashSet<EdgeKey> = HashSet::new();
    let mut result: Vec<EdgeKey> = Vec::new();
    let mut result_set: HashSet<EdgeKey> = HashSet::new();

    result_set.insert(start_edge);
    result.push(start_edge);
    // Track parallel edges for loop termination, as in loop_walk.
    visited_parallel.insert(start_edge);

    let initial_loops: Vec<LoopKey> = radial_walk(mesh, start_edge).collect();
    for start_loop in initial_loops {
        let mut cur_loop = start_loop;
        loop {
            let face = mesh.loops[cur_loop].face;
            if mesh.faces[face].loop_count != 4 {
                break;
            }
            let next_loop = mesh.loops[cur_loop].next;
            let parallel_loop = mesh.loops[next_loop].next;
            let parallel_edge = mesh.loops[parallel_loop].edge;
            if visited_parallel.contains(&parallel_edge) {
                break;
            }
            visited_parallel.insert(parallel_edge);
            // Collect the perpendicular edge (next) rather than the parallel edge.
            let perp_edge = mesh.loops[next_loop].edge;
            if !result_set.contains(&perp_edge) {
                result_set.insert(perp_edge);
                result.push(perp_edge);
            }
            // Cross to the neighboring face via the parallel edge (same as loop_walk).
            let radial_other = mesh.loops[parallel_loop].radial_next;
            if radial_other == parallel_loop {
                break;
            }
            cur_loop = radial_other;
        }
    }

    result
}
