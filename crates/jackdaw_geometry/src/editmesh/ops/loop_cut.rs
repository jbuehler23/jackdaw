//! Loop cut: walks the parallel-edge ring through quad faces and splits
//! each crossed edge at parameter `t`, then splits each crossed quad face
//! along the new vertices. Standard half-edge loop-cut algorithm.

use std::collections::{HashMap, HashSet};

use slotmap::Key;

use crate::editmesh::cycles::radial_walk;
use crate::editmesh::ops::edge_split::split_edge;
use crate::editmesh::ops::face_split::split_face;
use crate::editmesh::types::*;

#[derive(Debug)]
pub enum LoopCutError {
    EdgeNotFound,
    StartFaceNotQuad,
    EdgeSplit(crate::editmesh::ops::edge_split::SplitError),
    FaceSplit(crate::editmesh::ops::face_split::FaceSplitError),
}

pub struct LoopCutResult {
    pub new_verts: Vec<VertKey>,
    pub new_loop_edges: Vec<EdgeKey>,
    pub new_faces: Vec<FaceKey>,
}

pub fn loop_cut(
    bmesh: &mut EditMesh,
    start_edge: EdgeKey,
    t: f32,
) -> Result<LoopCutResult, LoopCutError> {
    if !bmesh.edges.contains_key(start_edge) {
        return Err(LoopCutError::EdgeNotFound);
    }

    // Phase 1: walk the edge ring, collecting per-face (entry_edge, exit_edge) pairs.
    let walk = walk_loop_cut_ring(bmesh, start_edge);
    if walk.is_empty() {
        return Err(LoopCutError::StartFaceNotQuad);
    }

    // Phase 2: split each unique edge, oriented relative to the slide axis.
    //
    // The slide axis is the canonical direction of the start edge in world space.
    // For every other crossed edge, we check whether its canonical direction aligns
    // with the slide axis (dot > 0) or is reversed (dot < 0).  This ensures that
    // t=0.3 means "30% of the way along the slide axis" uniformly for every edge in
    // the ring, producing a planar cut regardless of how the mesh's loop winding
    // alternates around the ring.
    let start_v0 = bmesh.verts[bmesh.edges[start_edge].v[0]].co;
    let start_v1 = bmesh.verts[bmesh.edges[start_edge].v[1]].co;
    let slide_axis = (start_v1 - start_v0).normalize_or_zero();

    // Collect unique edges from the walk (first occurrence wins for determinism).
    let mut seen_edges: HashSet<EdgeKey> = HashSet::new();
    let mut edges_to_cut: Vec<EdgeKey> = Vec::new();
    for hop in &walk {
        if seen_edges.insert(hop.entry_edge) {
            edges_to_cut.push(hop.entry_edge);
        }
        if seen_edges.insert(hop.exit_edge) {
            edges_to_cut.push(hop.exit_edge);
        }
    }
    // Sort for stable ordering across runs.
    edges_to_cut.sort_by_key(|e| e.data().as_ffi());

    let mut edge_to_new_vert: HashMap<EdgeKey, VertKey> = HashMap::new();
    let mut new_verts: Vec<VertKey> = Vec::new();
    for &edge in &edges_to_cut {
        let v0_pos = bmesh.verts[bmesh.edges[edge].v[0]].co;
        let v1_pos = bmesh.verts[bmesh.edges[edge].v[1]].co;
        let dir = (v1_pos - v0_pos).normalize_or_zero();
        let oriented_t = if dir.dot(slide_axis) >= 0.0 {
            t
        } else {
            1.0 - t
        };
        let v_new = split_edge(bmesh, edge, oriented_t).map_err(LoopCutError::EdgeSplit)?;
        edge_to_new_vert.insert(edge, v_new);
        new_verts.push(v_new);
    }

    // Phase 3: split each crossed face along the two new verts (one per ring edge).
    let mut new_loop_edges: Vec<EdgeKey> = Vec::new();
    let mut new_faces_out: Vec<FaceKey> = Vec::new();
    for hop in &walk {
        let va = edge_to_new_vert[&hop.entry_edge];
        let vb = edge_to_new_vert[&hop.exit_edge];
        let before: HashSet<FaceKey> = bmesh.faces.keys().collect();
        let new_edge = split_face(bmesh, hop.face, va, vb).map_err(LoopCutError::FaceSplit)?;
        let after: HashSet<FaceKey> = bmesh.faces.keys().collect();
        let added: Vec<FaceKey> = after.difference(&before).copied().collect();
        new_faces_out.extend(added);
        new_loop_edges.push(new_edge);
    }

    Ok(LoopCutResult {
        new_verts,
        new_loop_edges,
        new_faces: new_faces_out,
    })
}

/// One quad face in the ring: the face and the two edges the cut crosses on it.
/// `entry_edge` is the side shared with the previous face (or start_edge for the
/// first face). `exit_edge` is the parallel side shared with the next face.
#[derive(Clone, Copy)]
struct FaceHop {
    face: FaceKey,
    entry_edge: EdgeKey,
    exit_edge: EdgeKey,
}

/// Walk both directions from start_edge through the quad strip.
///
/// Returns a list of `FaceHop`s. Each entry_edge and exit_edge in the list is
/// the cut edges bounding that face. For a closed ring the exit_edge of the
/// last face equals the entry_edge of the first (start_edge itself), so
/// start_edge appears as the entry of the first face and the exit of the last.
/// For an open chain, start_edge appears only as an entry edge.
///
/// The caller deduplicates edges across hops before splitting.
fn walk_loop_cut_ring(bmesh: &EditMesh, start_edge: EdgeKey) -> Vec<FaceHop> {
    let mut hops: Vec<FaceHop> = Vec::new();
    let mut visited_faces: HashSet<FaceKey> = HashSet::new();

    // start_edge borders two faces (or one if boundary). Walk one direction
    // from each radial loop.
    let initial_loops: Vec<LoopKey> = radial_walk(bmesh, start_edge).collect();
    for start_loop in initial_loops {
        // The loop `start_loop` is on edge `start_edge` and belongs to one face.
        // We enter that face via start_edge and look for the parallel (exit) edge.
        let mut entry_loop = start_loop; // loop on the entry edge of the current face
        let mut entry_edge = start_edge;

        loop {
            let face = bmesh.loops[entry_loop].face;
            if !visited_faces.insert(face) {
                // Closed ring - we've come back around.
                break;
            }
            // Stop if current face isn't a quad.
            if bmesh.faces[face].loop_count != 4 {
                break;
            }

            // Find the parallel edge: step 2 positions forward from entry_loop
            // in the quad's ring. In a quad ABCD with entry at A (edge AB),
            // the parallel is at C (edge CD). This is the exit edge.
            let next1 = bmesh.loops[entry_loop].next;
            let exit_loop_key = bmesh.loops[next1].next; // loop at C
            let exit_edge = bmesh.loops[exit_loop_key].edge;

            hops.push(FaceHop {
                face,
                entry_edge,
                exit_edge,
            });

            // Boundary check: exit_edge has only one radial loop (itself).
            let radial_other = bmesh.loops[exit_loop_key].radial_next;
            if radial_other == exit_loop_key {
                // Boundary edge - stop this direction.
                break;
            }

            // Cross to the neighbouring face. `radial_other` is a loop on
            // `exit_edge` belonging to the next face.
            entry_loop = radial_other;
            entry_edge = exit_edge;
        }
    }
    hops
}
