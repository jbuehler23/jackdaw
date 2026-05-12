//! BFS over face graph via shared edges. Optionally respects SHARP/SEAM
//! markers on edges as walk blockers (so user can isolate face groups by
//! marking their boundaries).

use std::collections::{HashSet, VecDeque};

use crate::editmesh::cycles::radial_walk;
use crate::editmesh::types::*;

pub fn linked_walk(
    bmesh: &EditMesh,
    start_face: FaceKey,
    respect_sharp_seam: bool,
) -> Vec<FaceKey> {
    if !bmesh.faces.contains_key(start_face) {
        return Vec::new();
    }
    let mut visited: HashSet<FaceKey> = HashSet::new();
    let mut queue: VecDeque<FaceKey> = VecDeque::new();
    let mut result: Vec<FaceKey> = Vec::new();

    visited.insert(start_face);
    queue.push_back(start_face);
    result.push(start_face);

    while let Some(face) = queue.pop_front() {
        let face_data = &bmesh.faces[face];
        let mut cur = face_data.loop_first;
        for _ in 0..face_data.loop_count {
            let edge = bmesh.loops[cur].edge;
            // Check blocker.
            if respect_sharp_seam {
                let edge_flag = bmesh.edges[edge].flag;
                if edge_flag.contains(EdgeFlag::SHARP) || edge_flag.contains(EdgeFlag::SEAM) {
                    cur = bmesh.loops[cur].next;
                    continue;
                }
            }
            // Visit each face incident to this edge.
            for radial_lp in radial_walk(bmesh, edge).collect::<Vec<_>>() {
                let neighbor = bmesh.loops[radial_lp].face;
                if visited.insert(neighbor) {
                    queue.push_back(neighbor);
                    result.push(neighbor);
                }
            }
            cur = bmesh.loops[cur].next;
        }
    }

    result
}
