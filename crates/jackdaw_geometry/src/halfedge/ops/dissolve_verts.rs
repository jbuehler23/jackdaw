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
//!   - Valence >= 3 with incident faces: iteratively dissolve internal edges
//!     to preserve face structure, falling back to outer-ring
//!     merge for all-boundary cases.
//!
//! If the outer ring cannot be walked coherently (non-manifold neighborhood),
//! the vert is silently skipped.

use std::collections::HashSet;

use crate::halfedge::cycles::{
    disk_remove_edge, disk_walk, radial_insert_loop, radial_remove_loop, radial_walk,
};
use crate::halfedge::ops::edge_create::create_edge;
use crate::halfedge::types::*;

#[derive(Debug)]
pub enum DissolveError {
    EmptyInput,
}

pub struct DissolveVertsResult {
    pub removed_verts: usize,
}

pub fn dissolve_verts(
    mesh: &mut HalfedgeMesh,
    verts: &[VertKey],
) -> Result<DissolveVertsResult, DissolveError> {
    if verts.is_empty() {
        return Err(DissolveError::EmptyInput);
    }
    let mut removed = 0;
    for &v in verts {
        if dissolve_one_vert(mesh, v) {
            removed += 1;
        }
    }
    Ok(DissolveVertsResult {
        removed_verts: removed,
    })
}

fn dissolve_one_vert(mesh: &mut HalfedgeMesh, v: VertKey) -> bool {
    if !mesh.verts.contains_key(v) {
        return false;
    }

    let incident_edges: Vec<EdgeKey> = disk_walk(mesh, v).collect();

    if incident_edges.is_empty() {
        mesh.verts.remove(v);
        return true;
    }

    if incident_edges.len() == 1 {
        // Wire endpoint: just remove vert + edge.
        disk_remove_edge(mesh, incident_edges[0]);
        mesh.edges.remove(incident_edges[0]);
        mesh.verts.remove(v);
        return true;
    }

    // Use the iterative dissolve algorithm for valence >= 2.
    dissolve_valence_n(mesh, v)
}

/// Iterative dissolve for valence >= 2.
///
/// Handles the wire case, then selectively dissolves "co-planar structural
/// diagonal" edges (face_split diagonals whose two adjacent faces share the
/// same surface plane). If none exist, falls back to the outer-ring merge.
///
/// The co-planarity criterion identifies structural diagonals without needing
/// operation history: when `split_face` subdivides a face, the two
/// resulting sub-faces share the same geometric plane. Dissolving those
/// diagonals restores the original face. By contrast, a cube corner's incident
/// edges each connect faces on DIFFERENT planes, so no co-planar diagonal
/// candidates exist and the outer-ring merge applies directly.
fn dissolve_valence_n(mesh: &mut HalfedgeMesh, v: VertKey) -> bool {
    // Handle wire (no incident faces) case first.
    let initial_edges: Vec<EdgeKey> = disk_walk(mesh, v).collect();
    let has_faces = initial_edges
        .iter()
        .any(|&e| radial_walk(mesh, e).next().is_some());
    if !has_faces {
        for e in initial_edges {
            disk_remove_edge(mesh, e);
            mesh.edges.remove(e);
        }
        mesh.verts.remove(v);
        return true;
    }

    // Iteratively dissolve "co-planar diagonal" edges: cluster-internal edges
    // whose two adjacent faces share the same surface normal (i.e. were created
    // by a face_split of a single face). Only attempt if the merge is also
    // topologically valid (no duplicate vertices in the merged ring).
    //
    // If no co-planar diagonals exist (all edges connect faces on distinct
    // planes, e.g. a cube corner), skip the loop entirely and use the
    // outer-ring fallback below.
    let coplanar_threshold = 0.99f32; // cos(~8 deg) - faces must share same normal
    loop {
        let cluster_faces = faces_incident_to(mesh, v);
        if cluster_faces.len() <= 1 {
            break;
        }

        let mut coplanar_safe_edge: Option<EdgeKey> = None;
        for e in disk_walk(mesh, v).collect::<Vec<_>>() {
            let radial_loops: Vec<_> = radial_walk(mesh, e).collect();
            if radial_loops.len() != 2 {
                continue;
            }
            let f0 = mesh.loops[radial_loops[0]].face;
            let f1 = mesh.loops[radial_loops[1]].face;
            if f0 == f1 || !cluster_faces.contains(&f0) || !cluster_faces.contains(&f1) {
                continue;
            }
            // Check co-planarity: the two adjacent faces must share the same normal.
            let n0 = mesh.faces[f0].normal_cache;
            let n1 = mesh.faces[f1].normal_cache;
            if n0.dot(n1).abs() < coplanar_threshold {
                continue; // different planes -> not a face_split diagonal
            }
            // Also check that merging them produces a topologically valid face.
            if would_merge_produce_valid_face(mesh, e) {
                coplanar_safe_edge = Some(e);
                break;
            }
        }

        match coplanar_safe_edge {
            Some(e) => {
                let _ = crate::halfedge::ops::dissolve_edges::dissolve_edges(mesh, &[e]);
            }
            None => {
                break;
            }
        }
    }

    // v may have been removed if it became isolated during dissolution.
    if !mesh.verts.contains_key(v) {
        return true;
    }

    let remaining_edges: Vec<EdgeKey> = disk_walk(mesh, v).collect();

    if remaining_edges.is_empty() {
        mesh.verts.remove(v);
        return true;
    }

    if remaining_edges.len() == 2 {
        return dissolve_valence_2(mesh, v, remaining_edges[0], remaining_edges[1]);
    }

    if remaining_edges.len() == 1 {
        // Valence dropped to 1 after co-planar dissolves.
        disk_remove_edge(mesh, remaining_edges[0]);
        mesh.edges.remove(remaining_edges[0]);
        mesh.verts.remove(v);
        return true;
    }

    // Valence >= 3 with no co-planar diagonals: outer-ring fallback.
    // This handles cube corners and other all-distinct-plane clusters.
    let incident_faces = faces_incident_to(mesh, v);
    dissolve_valence_n_fallback(mesh, v, remaining_edges, incident_faces)
}

/// Returns true if dissolving edge `e` would produce a merged face with no
/// duplicate vertices. This detects structural diagonal edges (created by
/// face_split, safe to dissolve) vs. boundary half-edges (unsafe).
///
/// A dissolve is valid iff chain A (loops in face_a, excluding the removed
/// lp_a) and chain B (loops in face_b, excluding lp_b) have disjoint vertex
/// sets.
fn would_merge_produce_valid_face(mesh: &HalfedgeMesh, e: EdgeKey) -> bool {
    let radial: Vec<_> = radial_walk(mesh, e).collect();
    if radial.len() != 2 {
        return false;
    }
    let lp_a = radial[0];
    let lp_b = radial[1];

    // Collect all verts in face_a's ring excluding lp_a.vert (the removed loop).
    let mut verts_a: HashSet<VertKey> = HashSet::new();
    let mut cur = mesh.loops[lp_a].next;
    while cur != lp_a {
        verts_a.insert(mesh.loops[cur].vert);
        cur = mesh.loops[cur].next;
    }

    // Check face_b's ring (excluding lp_b) for any overlap with face_a.
    let mut cur = mesh.loops[lp_b].next;
    while cur != lp_b {
        if verts_a.contains(&mesh.loops[cur].vert) {
            return false;
        }
        cur = mesh.loops[cur].next;
    }
    true
}

/// Returns the set of faces incident to `v`.
fn faces_incident_to(mesh: &HalfedgeMesh, v: VertKey) -> HashSet<FaceKey> {
    let mut out = HashSet::new();
    for e in disk_walk(mesh, v).collect::<Vec<_>>() {
        for lp in radial_walk(mesh, e).collect::<Vec<_>>() {
            out.insert(mesh.loops[lp].face);
        }
    }
    out
}

/// Valence-2 dissolve: splice `v` out of every incident face ring and replace
/// the two incident edges with a single merged edge.  Faces are preserved.
fn dissolve_valence_2(mesh: &mut HalfedgeMesh, v: VertKey, e1: EdgeKey, e2: EdgeKey) -> bool {
    // Guard: both edges must still exist.
    if !mesh.edges.contains_key(e1) || !mesh.edges.contains_key(e2) {
        return false;
    }

    let edge1 = mesh.edges[e1].clone();
    let edge2 = mesh.edges[e2].clone();
    let a = if edge1.v[0] == v {
        edge1.v[1]
    } else {
        edge1.v[0]
    };
    let b = if edge2.v[0] == v {
        edge2.v[1]
    } else {
        edge2.v[0]
    };
    if a == b {
        // Degenerate: both edges connect to the same vert.
        return false;
    }

    // Check whether v has any incident faces.
    let has_faces = [e1, e2]
        .iter()
        .any(|&e| radial_walk(mesh, e).next().is_some());
    if !has_faces {
        // Wire vert with no incident faces: just remove both edges and the vert.
        disk_remove_edge(mesh, e1);
        mesh.edges.remove(e1);
        disk_remove_edge(mesh, e2);
        mesh.edges.remove(e2);
        mesh.verts.remove(v);
        return true;
    }

    // Create the merged edge (a, b). create_edge is idempotent.
    let new_edge = create_edge(mesh, a, b);

    // Collect all loops whose vert == v from the radial cycles of e1 and e2.
    let mut lp_v_list: Vec<LoopKey> = Vec::new();
    for e in [e1, e2] {
        for lp in radial_walk(mesh, e).collect::<Vec<_>>() {
            if mesh.loops[lp].vert == v && !lp_v_list.contains(&lp) {
                lp_v_list.push(lp);
            }
        }
    }

    // For each v-loop: splice it out of its face ring and re-edge lp_prev.
    let mut affected_faces: Vec<FaceKey> = Vec::new();
    for &lp_v in &lp_v_list {
        let face = mesh.loops[lp_v].face;
        let lp_prev = mesh.loops[lp_v].prev;
        let lp_next = mesh.loops[lp_v].next;

        // Splice ring: skip over lp_v.
        mesh.loops[lp_prev].next = lp_next;
        mesh.loops[lp_next].prev = lp_prev;

        // lp_prev now walks to lp_next.vert; the edge for that step is new_edge.
        radial_remove_loop(mesh, lp_prev);
        mesh.loops[lp_prev].edge = new_edge;
        radial_insert_loop(mesh, lp_prev);

        // Remove lp_v from its old edge's radial cycle and drop it.
        radial_remove_loop(mesh, lp_v);

        // Fix face anchor if it pointed at lp_v.
        if mesh.faces[face].loop_first == lp_v {
            mesh.faces[face].loop_first = lp_prev;
        }
        mesh.faces[face].loop_count -= 1;
        mesh.loops.remove(lp_v);

        if !affected_faces.contains(&face) {
            affected_faces.push(face);
        }
    }

    // Remove old edges from disk cycles and slotmap.
    disk_remove_edge(mesh, e1);
    mesh.edges.remove(e1);
    disk_remove_edge(mesh, e2);
    mesh.edges.remove(e2);

    // Remove the vert.
    mesh.verts.remove(v);

    // Re-cache face normals.
    for face in affected_faces {
        if !mesh.faces.contains_key(face) {
            continue;
        }
        let loop_count = mesh.faces[face].loop_count as usize;
        let mut ring_positions = Vec::with_capacity(loop_count);
        let mut cur = mesh.faces[face].loop_first;
        for _ in 0..loop_count {
            ring_positions.push(mesh.verts[mesh.loops[cur].vert].co);
            cur = mesh.loops[cur].next;
        }
        mesh.faces[face].normal_cache = crate::newell_normal(&ring_positions);
    }

    true
}

/// Valence-N (N >= 3) dissolve fallback: merge all incident faces into one new face
/// whose ring is the outer boundary of `v`'s neighborhood.
///
/// Used when all remaining edges around `v` are boundary edges (each touches
/// faces outside the v-cluster), e.g. dissolving a cube corner (3 distinct faces).
fn dissolve_valence_n_fallback(
    mesh: &mut HalfedgeMesh,
    v: VertKey,
    incident_edges: Vec<EdgeKey>,
    incident_faces: HashSet<FaceKey>,
) -> bool {
    // Build the outer ring by walking incident faces cyclically around v.
    let mut outer_ring = match build_outer_ring(mesh, v, &incident_faces) {
        Some(r) if r.len() >= 3 => r,
        _ => return false, // non-manifold or degenerate neighborhood
    };

    // Snapshot material_idx from one incident face BEFORE removal so the merged
    // face can inherit it, avoiding duplicate material_idx values after dissolve.
    let inherit_material_idx = incident_faces
        .iter()
        .next()
        .map(|&f| mesh.faces[f].material_idx)
        .unwrap_or(0);

    // Compute expected outward normal: average of incident face normals.
    let mut expected_normal = bevy::math::Vec3::ZERO;
    for &face in &incident_faces {
        expected_normal += mesh.faces[face].normal_cache;
    }
    let expected_normal = expected_normal.normalize_or_zero();

    // Compute the proposed ring's Newell normal and centroid.
    let ring_positions: Vec<bevy::math::Vec3> =
        outer_ring.iter().map(|&k| mesh.verts[k].co).collect();
    let ring_normal = crate::newell_normal(&ring_positions);
    let ring_centroid: bevy::math::Vec3 =
        ring_positions.iter().copied().sum::<bevy::math::Vec3>() / ring_positions.len() as f32;

    // Determine whether to reverse the ring's winding.
    let should_reverse = if expected_normal.length_squared() > 0.5 {
        // Normal case: check against the average of incident face normals.
        ring_normal.dot(expected_normal) < 0.0
    } else {
        // Symmetric-normal case (e.g. dissolving across a cube edge between
        // opposing faces): fall back to the brush interior centroid as reference.
        // The merged face's normal should point AWAY from the interior centroid.
        let mut brush_centroid_sum = bevy::math::Vec3::ZERO;
        let mut count = 0u32;
        for (k, vert) in mesh.verts.iter() {
            if k == v {
                continue;
            }
            brush_centroid_sum += vert.co;
            count += 1;
        }
        if count == 0 {
            false
        } else {
            let brush_centroid = brush_centroid_sum / count as f32;
            let outward_dir = (ring_centroid - brush_centroid).normalize_or_zero();
            outward_dir.length_squared() > 0.5 && ring_normal.dot(outward_dir) < 0.0
        }
    };

    if should_reverse {
        outer_ring.reverse();
    }

    // Remove all incident faces (and their loops) from the HalfedgeMesh.
    for &face in &incident_faces {
        if !mesh.faces.contains_key(face) {
            continue;
        }
        let face_data = mesh.faces[face].clone();
        let mut cur = face_data.loop_first;
        let mut loops_to_remove: Vec<LoopKey> = Vec::with_capacity(face_data.loop_count as usize);
        for _ in 0..face_data.loop_count {
            loops_to_remove.push(cur);
            cur = mesh.loops[cur].next;
        }
        for &lp in &loops_to_remove {
            radial_remove_loop(mesh, lp);
            mesh.loops.remove(lp);
        }
        mesh.faces.remove(face);
    }

    // Remove all incident edges and the vert.
    for e in incident_edges {
        disk_remove_edge(mesh, e);
        mesh.edges.remove(e);
    }
    mesh.verts.remove(v);

    // Create the merged face, inheriting material_idx from one of the dissolved
    // faces so we never introduce a duplicate material_idx in the HalfedgeMesh.
    let _ = crate::halfedge::ops::face_create::create_face_from_verts_with_material(
        mesh,
        &outer_ring,
        Some(inherit_material_idx),
    );

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
    mesh: &HalfedgeMesh,
    v: VertKey,
    incident_faces: &HashSet<FaceKey>,
) -> Option<Vec<VertKey>> {
    let start_face = *incident_faces.iter().next()?;
    let start_loop_v = find_loop_at_vert_in_face(mesh, start_face, v)?;

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
        let lp_prev = mesh.loops[cur_loop_v].prev;
        ring.push(mesh.loops[lp_prev].vert);

        // Cross to the next incident face via the edge of cur_loop_v
        // (which connects v to the next vert in this face).
        let next_edge = mesh.loops[cur_loop_v].edge;
        let mut next_face_opt: Option<FaceKey> = None;
        let mut next_loop_v_opt: Option<LoopKey> = None;

        for radial_lp in radial_walk(mesh, next_edge).collect::<Vec<_>>() {
            let radial_face = mesh.loops[radial_lp].face;
            if radial_face == cur_face {
                continue;
            }
            if !incident_faces.contains(&radial_face) {
                continue;
            }
            if let Some(lp_v_in_other) = find_loop_at_vert_in_face(mesh, radial_face, v) {
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

fn find_loop_at_vert_in_face(mesh: &HalfedgeMesh, face: FaceKey, vert: VertKey) -> Option<LoopKey> {
    let f = &mesh.faces[face];
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        if mesh.loops[cur].vert == vert {
            return Some(cur);
        }
        cur = mesh.loops[cur].next;
    }
    None
}
