//! Subdivide selected edges at their midpoints, then re-tessellate touched faces.
//!
//! For each face: count how many of its original edges were cut. Cases:
//!   - 0 cuts: untouched.
//!   - 1 cut: connect the new midpoint to the opposite ring vertex (for quads).
//!   - 2 opposite cuts: connect the two midpoints.
//!   - 2 adjacent cuts: connect the two midpoints (yields tri + pentagon for MVP).
//!   - 3+ cuts: triangulate from one of the midpoints (MVP fallback).
//!   - 4 cuts (quad): 2x2 subdivision via two cross-cuts through opposite midpoints.
//!
//! MVP limitation: the 4-cut case does not produce 4 perfect quads because
//! `bm_face_poke` (insert center vert into face interior) is not yet implemented.
//! Instead, two sequential `split_face` calls are made, producing 3 sub-faces.
//! Full 2x2 subdivision is deferred until `bm_face_poke` lands.

use std::collections::{HashMap, HashSet};

use crate::editmesh::cycles::radial_walk;
use crate::editmesh::ops::edge_split::split_edge;
use crate::editmesh::ops::face_split::split_face;
use crate::editmesh::types::*;

#[derive(Debug)]
pub enum SubdivideError {
    EdgeSplit(crate::editmesh::ops::edge_split::SplitError),
    FaceSplit(crate::editmesh::ops::face_split::FaceSplitError),
}

pub struct SubdivideResult {
    pub new_verts: Vec<VertKey>,
    pub new_edges: Vec<EdgeKey>,
    pub new_faces: Vec<FaceKey>,
}

pub fn subdivide(
    bmesh: &mut EditMesh,
    edges_to_cut: &[EdgeKey],
) -> Result<SubdivideResult, SubdivideError> {
    // Phase 1: snapshot which faces are incident to each edge BEFORE splitting.
    // We map each face to the list of new midpoint verts it will receive.
    // Also snapshot the original edge -> face list for faces that need tessellation.
    let mut face_incident_edges: HashMap<FaceKey, Vec<EdgeKey>> = HashMap::new();
    for &edge in edges_to_cut {
        for lp in radial_walk(bmesh, edge) {
            let face = bmesh.loops[lp].face;
            face_incident_edges.entry(face).or_default().push(edge);
        }
    }

    // Phase 2: split each edge at t=0.5. Save (edge -> new_vert).
    let mut new_verts_out: Vec<VertKey> = Vec::new();
    let mut edge_to_new_vert: HashMap<EdgeKey, VertKey> = HashMap::new();
    for &edge in edges_to_cut {
        // Skip if this edge key no longer exists (edge_split may have retired
        // the key and reissued a new one -- in practice slotmap retains the key
        // but the edge's vertices change. Guard for safety.)
        if !bmesh.edges.contains_key(edge) {
            continue;
        }
        let v_new = split_edge(bmesh, edge, 0.5).map_err(SubdivideError::EdgeSplit)?;
        edge_to_new_vert.insert(edge, v_new);
        new_verts_out.push(v_new);
    }

    // Build the (face -> [midpoint verts]) mapping now that splits are done.
    let mut face_midpoints: HashMap<FaceKey, Vec<VertKey>> = HashMap::new();
    for (face, orig_edges) in &face_incident_edges {
        for edge in orig_edges {
            if let Some(&v_new) = edge_to_new_vert.get(edge) {
                face_midpoints.entry(*face).or_default().push(v_new);
            }
        }
    }

    // Phase 3: re-tessellate each affected face.
    let mut new_edges_out: Vec<EdgeKey> = Vec::new();
    let mut new_faces_out: Vec<FaceKey> = Vec::new();
    for (face, midpoint_verts) in face_midpoints {
        // Face may have been invalidated if it shared an edge with another cut face
        // that was already split. Re-check it exists.
        if !bmesh.faces.contains_key(face) {
            continue;
        }

        let cuts = midpoint_verts.len();
        match cuts {
            0 => {
                // untouched
            }
            1 => {
                // Connect the single midpoint to the opposite ring vert.
                let mid = midpoint_verts[0];
                if let Some(opp) = find_opposite_ring_vert(bmesh, face, mid) {
                    let before_faces: HashSet<_> = bmesh.faces.keys().collect();
                    match split_face(bmesh, face, mid, opp) {
                        Ok(new_edge) => {
                            new_edges_out.push(new_edge);
                            let after_faces: HashSet<_> = bmesh.faces.keys().collect();
                            new_faces_out.extend(after_faces.difference(&before_faces).copied());
                        }
                        Err(_) => {
                            // Cannot split (e.g. adjacent or degenerate); leave face as-is.
                        }
                    }
                }
            }
            2 => {
                // Determine if the two midpoints are on opposite or adjacent edges,
                // then connect them. split_face handles both cases (returns
                // Adjacent error if they happen to be adjacent on the now-expanded ring,
                // which we silently skip).
                let before_faces: HashSet<_> = bmesh.faces.keys().collect();
                match split_face(bmesh, face, midpoint_verts[0], midpoint_verts[1]) {
                    Ok(new_edge) => {
                        new_edges_out.push(new_edge);
                        let after_faces: HashSet<_> = bmesh.faces.keys().collect();
                        new_faces_out.extend(after_faces.difference(&before_faces).copied());
                    }
                    Err(_) => {
                        // Adjacent or degenerate; leave as pentagon.
                    }
                }
            }
            4 => {
                // Quad with all 4 edges cut: classic 2x2 subdivision.
                // MVP: two sequential cross-cuts through opposite midpoints.
                // This produces 3 sub-faces rather than 4 quads. Full 2x2
                // subdivision is deferred until bm_face_poke lands.
                //
                // Attempt to find two pairs of opposite midpoints (by their ring
                // positions) and do two split_face calls.
                let pairs = find_opposite_pairs(bmesh, face, &midpoint_verts);
                let mut current_face = face;
                for (va, vb) in pairs {
                    // After the first split, `current_face` is replaced by two faces.
                    // Find the sub-face that still contains both va and vb.
                    let target = find_face_containing_both(bmesh, current_face, va, vb);
                    let target_face = target.unwrap_or(current_face);
                    if !bmesh.faces.contains_key(target_face) {
                        continue;
                    }
                    let before_faces: HashSet<_> = bmesh.faces.keys().collect();
                    match split_face(bmesh, target_face, va, vb) {
                        Ok(new_edge) => {
                            new_edges_out.push(new_edge);
                            let after_faces: HashSet<_> = bmesh.faces.keys().collect();
                            let added: Vec<_> = after_faces.difference(&before_faces).copied().collect();
                            // After first cut, the face is gone; next iteration looks for
                            // a face containing the second pair among newly added faces.
                            if let Some(&f) = added.first() {
                                current_face = f;
                            }
                            new_faces_out.extend(added);
                        }
                        Err(_) => {}
                    }
                }
            }
            _ => {
                // 3-cut or other unusual case: fan-split from the first midpoint
                // to each remaining midpoint, updating the target face each time.
                let pivot = midpoint_verts[0];
                let mut current_face = face;
                for &other in &midpoint_verts[1..] {
                    let target = find_face_containing_both(bmesh, current_face, pivot, other);
                    let target_face = match target {
                        Some(tf) => tf,
                        None => {
                            // Search all faces for one containing both verts.
                            match find_any_face_with_verts(bmesh, &[pivot, other]) {
                                Some(tf) => tf,
                                None => continue,
                            }
                        }
                    };
                    if !bmesh.faces.contains_key(target_face) {
                        continue;
                    }
                    let before_faces: HashSet<_> = bmesh.faces.keys().collect();
                    match split_face(bmesh, target_face, pivot, other) {
                        Ok(new_edge) => {
                            new_edges_out.push(new_edge);
                            let after_faces: HashSet<_> = bmesh.faces.keys().collect();
                            let added: Vec<_> = after_faces.difference(&before_faces).copied().collect();
                            if let Some(&f) = added.first() {
                                current_face = f;
                            }
                            new_faces_out.extend(added);
                        }
                        Err(_) => {}
                    }
                }
            }
        }
    }

    Ok(SubdivideResult { new_verts: new_verts_out, new_edges: new_edges_out, new_faces: new_faces_out })
}

/// For a face with one newly inserted midpoint vert, find the vert on the opposite
/// side of the ring (index + loop_count/2). Returns `None` for triangles or if the
/// midpoint is not in the ring.
fn find_opposite_ring_vert(bmesh: &EditMesh, face: FaceKey, vert: VertKey) -> Option<VertKey> {
    let f = &bmesh.faces[face];
    if f.loop_count < 4 {
        return None;
    }
    let mut ring_verts: Vec<VertKey> = Vec::with_capacity(f.loop_count as usize);
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        ring_verts.push(bmesh.loops[cur].vert);
        cur = bmesh.loops[cur].next;
    }
    let pos = ring_verts.iter().position(|&v| v == vert)?;
    let opp_pos = (pos + ring_verts.len() / 2) % ring_verts.len();
    Some(ring_verts[opp_pos])
}

/// Given four midpoint verts on a (now 8-vertex) face, return up to two (va, vb)
/// pairs that are at ring distance 4 from each other (true opposites on the
/// expanded ring). Falls back to any two non-adjacent pairs if geometry is
/// irregular.
fn find_opposite_pairs(bmesh: &EditMesh, face: FaceKey, midpoints: &[VertKey]) -> Vec<(VertKey, VertKey)> {
    let f = &bmesh.faces[face];
    let mut ring_verts: Vec<VertKey> = Vec::with_capacity(f.loop_count as usize);
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        ring_verts.push(bmesh.loops[cur].vert);
        cur = bmesh.loops[cur].next;
    }
    let n = ring_verts.len();

    // Find ring positions of each midpoint.
    let positions: Vec<usize> = midpoints
        .iter()
        .filter_map(|&v| ring_verts.iter().position(|&rv| rv == v))
        .collect();

    if positions.len() < 2 {
        return Vec::new();
    }

    // Try to pair midpoints at distance n/2 apart (opposite on an 8-ring = distance 4).
    let half = n / 2;
    let mut paired: HashSet<usize> = HashSet::new();
    let mut pairs: Vec<(VertKey, VertKey)> = Vec::new();

    for (i, &pa) in positions.iter().enumerate() {
        if paired.contains(&i) {
            continue;
        }
        for (j, &pb) in positions.iter().enumerate() {
            if i == j || paired.contains(&j) {
                continue;
            }
            let dist = ((pa + n) - pb) % n;
            let dist_rev = ((pb + n) - pa) % n;
            if dist == half || dist_rev == half {
                pairs.push((midpoints[i], midpoints[j]));
                paired.insert(i);
                paired.insert(j);
                break;
            }
        }
    }

    // If no opposite pair found (unusual face shape), fall back to first two midpoints.
    if pairs.is_empty() && positions.len() >= 2 {
        pairs.push((midpoints[0], midpoints[1]));
    }

    pairs
}

/// Search `candidate_face` and then `new_faces` for a face whose ring contains
/// both `va` and `vb`. Used after a split to find the right sub-face.
fn find_face_containing_both(
    bmesh: &EditMesh,
    candidate: FaceKey,
    va: VertKey,
    vb: VertKey,
) -> Option<FaceKey> {
    if bmesh.faces.contains_key(candidate) && face_ring_contains(bmesh, candidate, va, vb) {
        return Some(candidate);
    }
    None
}

/// Search ALL faces for one whose ring contains every vert in `verts`.
fn find_any_face_with_verts(bmesh: &EditMesh, verts: &[VertKey]) -> Option<FaceKey> {
    for (fk, f) in bmesh.faces.iter() {
        let mut ring: HashSet<VertKey> = HashSet::new();
        let mut cur = f.loop_first;
        for _ in 0..f.loop_count {
            ring.insert(bmesh.loops[cur].vert);
            cur = bmesh.loops[cur].next;
        }
        if verts.iter().all(|v| ring.contains(v)) {
            return Some(fk);
        }
    }
    None
}

/// Returns true iff face `f`'s ring contains both `va` and `vb`.
fn face_ring_contains(bmesh: &EditMesh, face: FaceKey, va: VertKey, vb: VertKey) -> bool {
    let f = &bmesh.faces[face];
    let mut found_a = false;
    let mut found_b = false;
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        let v = bmesh.loops[cur].vert;
        if v == va {
            found_a = true;
        }
        if v == vb {
            found_b = true;
        }
        cur = bmesh.loops[cur].next;
    }
    found_a && found_b
}
