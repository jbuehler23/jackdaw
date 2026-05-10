//! Dissolve verts: for each selected vert, remove it and merge all its
//! incident faces into a single face whose ring is the "outer boundary" of
//! the vert's neighborhood.
//!
//! Handles any valence:
//!   - Isolated vert (valence 0): just remove it.
//!   - Valence 1 (wire endpoint): remove vert + edge.
//!   - Valence 2 with no incident faces (wire): remove vert + both edges.
//!   - Valence 2 with incident faces: splice the vert out of each face ring and
//!     merge the two incident edges into one (classic midpoint-dissolve). Faces
//!     are preserved; each face just loses one loop entry.
//!   - Valence >= 3 with incident faces: build the outer ring from all incident
//!     faces, remove them, remove vert + incident edges, create merged face.
//!
//! If the outer ring cannot be walked coherently (non-manifold neighborhood),
//! the vert is silently skipped.

use std::collections::HashSet;

use crate::bmesh::cycles::{
    disk_remove_edge, disk_walk, radial_insert_loop, radial_remove_loop, radial_walk,
};
use crate::bmesh::ops::edge_create::bm_edge_create;
use crate::bmesh::types::*;

#[derive(Debug)]
pub enum DissolveError {
    EmptyInput,
}

pub struct DissolveVertsResult {
    pub removed_verts: usize,
}

pub fn dissolve_verts(
    bmesh: &mut BMesh,
    verts: &[VertKey],
) -> Result<DissolveVertsResult, DissolveError> {
    if verts.is_empty() {
        return Err(DissolveError::EmptyInput);
    }
    let mut removed = 0;
    for &v in verts {
        if dissolve_one_vert(bmesh, v) {
            removed += 1;
        }
    }
    Ok(DissolveVertsResult { removed_verts: removed })
}

fn dissolve_one_vert(bmesh: &mut BMesh, v: VertKey) -> bool {
    if !bmesh.verts.contains_key(v) {
        return false;
    }

    // Step 1: gather incident edges via disk walk.
    let incident_edges: Vec<EdgeKey> = disk_walk(bmesh, v).collect();

    if incident_edges.is_empty() {
        // Isolated vert: just remove it.
        bmesh.verts.remove(v);
        return true;
    }

    if incident_edges.len() == 1 {
        // Wire endpoint: remove vert + edge.
        disk_remove_edge(bmesh, incident_edges[0]);
        bmesh.edges.remove(incident_edges[0]);
        bmesh.verts.remove(v);
        return true;
    }

    // Collect all incident faces (via all incident edges' radial cycles).
    let mut incident_faces: HashSet<FaceKey> = HashSet::new();
    for &e in &incident_edges {
        for lp in radial_walk(bmesh, e).collect::<Vec<_>>() {
            incident_faces.insert(bmesh.loops[lp].face);
        }
    }

    if incident_faces.is_empty() {
        // All wire edges (no incident faces); remove them and the vert.
        for &e in &incident_edges {
            disk_remove_edge(bmesh, e);
            bmesh.edges.remove(e);
        }
        bmesh.verts.remove(v);
        return true;
    }

    if incident_edges.len() == 2 {
        // Valence-2 with incident faces: splice the vert out of each face ring,
        // merging the two incident edges into one. Faces are preserved.
        dissolve_valence2(bmesh, v, incident_edges[0], incident_edges[1])
    } else {
        // Valence >= 3 with incident faces: merge all incident faces into one.
        dissolve_valence_n(bmesh, v, incident_edges, incident_faces)
    }
}

/// Valence-2 dissolve: splice `v` out of every incident face ring and replace
/// the two incident edges with a single merged edge.  Faces are preserved.
fn dissolve_valence2(bmesh: &mut BMesh, v: VertKey, e1: EdgeKey, e2: EdgeKey) -> bool {
    let edge1 = bmesh.edges[e1].clone();
    let edge2 = bmesh.edges[e2].clone();
    let a = if edge1.v[0] == v { edge1.v[1] } else { edge1.v[0] };
    let b = if edge2.v[0] == v { edge2.v[1] } else { edge2.v[0] };
    if a == b {
        // Degenerate: both edges connect to the same vert.
        return false;
    }

    // Create the merged edge (a, b). bm_edge_create is idempotent.
    let new_edge = bm_edge_create(bmesh, a, b);

    // Collect all loops whose vert == v from the radial cycles of e1 and e2.
    let mut lp_v_list: Vec<LoopKey> = Vec::new();
    for e in [e1, e2] {
        for lp in radial_walk(bmesh, e).collect::<Vec<_>>() {
            if bmesh.loops[lp].vert == v && !lp_v_list.contains(&lp) {
                lp_v_list.push(lp);
            }
        }
    }

    // For each v-loop: splice it out of its face ring and re-edge lp_prev.
    let mut affected_faces: Vec<FaceKey> = Vec::new();
    for &lp_v in &lp_v_list {
        let face = bmesh.loops[lp_v].face;
        let lp_prev = bmesh.loops[lp_v].prev;
        let lp_next = bmesh.loops[lp_v].next;

        // Splice ring: skip over lp_v.
        bmesh.loops[lp_prev].next = lp_next;
        bmesh.loops[lp_next].prev = lp_prev;

        // lp_prev now walks to lp_next.vert; the edge for that step is new_edge.
        radial_remove_loop(bmesh, lp_prev);
        bmesh.loops[lp_prev].edge = new_edge;
        radial_insert_loop(bmesh, lp_prev);

        // Remove lp_v from its old edge's radial cycle and drop it.
        radial_remove_loop(bmesh, lp_v);

        // Fix face anchor if it pointed at lp_v.
        if bmesh.faces[face].loop_first == lp_v {
            bmesh.faces[face].loop_first = lp_prev;
        }
        bmesh.faces[face].loop_count -= 1;
        bmesh.loops.remove(lp_v);

        if !affected_faces.contains(&face) {
            affected_faces.push(face);
        }
    }

    // Remove old edges from disk cycles and slotmap.
    disk_remove_edge(bmesh, e1);
    bmesh.edges.remove(e1);
    disk_remove_edge(bmesh, e2);
    bmesh.edges.remove(e2);

    // Remove the vert.
    bmesh.verts.remove(v);

    // Re-cache face normals.
    for face in affected_faces {
        if !bmesh.faces.contains_key(face) {
            continue;
        }
        let loop_count = bmesh.faces[face].loop_count as usize;
        let mut ring_positions = Vec::with_capacity(loop_count);
        let mut cur = bmesh.faces[face].loop_first;
        for _ in 0..loop_count {
            ring_positions.push(bmesh.verts[bmesh.loops[cur].vert].co);
            cur = bmesh.loops[cur].next;
        }
        bmesh.faces[face].normal_cache = crate::newell_normal(&ring_positions);
    }

    true
}

/// Valence-N (N >= 3) dissolve: merge all incident faces into one new face
/// whose ring is the outer boundary of `v`'s neighborhood.
fn dissolve_valence_n(
    bmesh: &mut BMesh,
    v: VertKey,
    incident_edges: Vec<EdgeKey>,
    incident_faces: HashSet<FaceKey>,
) -> bool {
    // Build the outer ring by walking incident faces cyclically around v.
    let mut outer_ring = match build_outer_ring(bmesh, v, &incident_faces) {
        Some(r) if r.len() >= 3 => r,
        _ => return false, // non-manifold or degenerate neighborhood
    };

    // Compute expected outward normal: average of incident face normals.
    let mut expected_normal = bevy::math::Vec3::ZERO;
    for &face in &incident_faces {
        expected_normal += bmesh.faces[face].normal_cache;
    }
    let expected_normal = expected_normal.normalize_or_zero();

    // Compute the proposed ring's Newell normal.
    let ring_positions: Vec<bevy::math::Vec3> = outer_ring.iter()
        .map(|&k| bmesh.verts[k].co)
        .collect();
    let ring_normal = crate::newell_normal(&ring_positions);

    // If the ring's normal points opposite to expected outward, reverse the ring.
    if expected_normal.length_squared() > 0.5 && ring_normal.dot(expected_normal) < 0.0 {
        outer_ring.reverse();
    }

    // Remove all incident faces (and their loops) from the BMesh.
    for &face in &incident_faces {
        if !bmesh.faces.contains_key(face) {
            continue;
        }
        let face_data = bmesh.faces[face].clone();
        let mut cur = face_data.loop_first;
        let mut loops_to_remove: Vec<LoopKey> = Vec::with_capacity(face_data.loop_count as usize);
        for _ in 0..face_data.loop_count {
            loops_to_remove.push(cur);
            cur = bmesh.loops[cur].next;
        }
        for &lp in &loops_to_remove {
            radial_remove_loop(bmesh, lp);
            bmesh.loops.remove(lp);
        }
        bmesh.faces.remove(face);
    }

    // Remove all incident edges and the vert.
    for e in incident_edges {
        disk_remove_edge(bmesh, e);
        bmesh.edges.remove(e);
    }
    bmesh.verts.remove(v);

    // Create the merged face from the outer ring.
    let _ = crate::bmesh::ops::face_create::bm_face_create_from_verts(bmesh, &outer_ring);

    true
}

/// Walk incident faces cyclically around `v` to produce the outer boundary ring.
///
/// For each incident face visited in cyclic order, the vert that comes BEFORE v
/// in that face's loop ring (i.e. `lp_v.prev.vert`) is appended to the ring.
/// We advance to the next incident face via the edge from v to the next vert in
/// the current face (`lp_v`'s edge), crossing its radial cycle.
///
/// Returns `None` if the walk doesn't cover every incident face (non-manifold or
/// open boundary that can't be dissolved cleanly).
fn build_outer_ring(
    bmesh: &BMesh,
    v: VertKey,
    incident_faces: &HashSet<FaceKey>,
) -> Option<Vec<VertKey>> {
    let start_face = *incident_faces.iter().next()?;
    let start_loop_v = find_loop_at_vert_in_face(bmesh, start_face, v)?;

    let mut ring: Vec<VertKey> = Vec::new();
    let mut visited_faces: HashSet<FaceKey> = HashSet::new();
    let mut cur_face = start_face;
    let mut cur_loop_v = start_loop_v;

    loop {
        if visited_faces.contains(&cur_face) {
            break;
        }
        visited_faces.insert(cur_face);

        // The vert immediately before v in this face's ring is an outer ring vert.
        let lp_prev = bmesh.loops[cur_loop_v].prev;
        ring.push(bmesh.loops[lp_prev].vert);

        // Cross to the next incident face via the edge of cur_loop_v
        // (which connects v to the next vert in this face).
        let next_edge = bmesh.loops[cur_loop_v].edge;
        let mut next_face_opt: Option<FaceKey> = None;
        let mut next_loop_v_opt: Option<LoopKey> = None;

        for radial_lp in radial_walk(bmesh, next_edge).collect::<Vec<_>>() {
            let radial_face = bmesh.loops[radial_lp].face;
            if radial_face == cur_face {
                continue;
            }
            if !incident_faces.contains(&radial_face) {
                continue;
            }
            if let Some(lp_v_in_other) = find_loop_at_vert_in_face(bmesh, radial_face, v) {
                next_face_opt = Some(radial_face);
                next_loop_v_opt = Some(lp_v_in_other);
                break;
            }
        }

        match (next_face_opt, next_loop_v_opt) {
            (Some(f), Some(l)) => {
                cur_face = f;
                cur_loop_v = l;
            }
            _ => break, // Boundary reached or non-manifold.
        }
    }

    if visited_faces.len() != incident_faces.len() {
        // Walk didn't visit every incident face; dissolve is not safe.
        return None;
    }

    Some(ring)
}

fn find_loop_at_vert_in_face(bmesh: &BMesh, face: FaceKey, vert: VertKey) -> Option<LoopKey> {
    let f = &bmesh.faces[face];
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        if bmesh.loops[cur].vert == vert {
            return Some(cur);
        }
        cur = bmesh.loops[cur].next;
    }
    None
}
