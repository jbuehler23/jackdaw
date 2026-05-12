//! Vertex bevel: replace a single vertex with a small N-gon face where N is
//! the degree of that vertex. Single-vertex MVP.
//!
//! For a vertex `v` with incident edges `e_1, ..., e_N` reaching neighbors
//! `other_1, ..., other_N`:
//!   - Create offset vertices `v_i = v + (other_i - v).normalize() * width`.
//!   - Each adjacent face `F` of `v` has its ring updated: the corner at `v`
//!     becomes the pair `(v_a, v_b)` where `v_a` and `v_b` are the two `v_i`
//!     corresponding to the two incident-at-v edges of `F`.
//!   - A new N-gon "bevel face" is created using all `v_i` ordered around the
//!     outward direction (average of adjacent face normals).
//!
//! For a cube corner (degree 3): 3 offset verts, 3 boundary edges, 1 new
//! triangular bevel face, 3 rebuilt adjacent quad faces.
//! Net delta: +2 verts, +3 edges, +1 face.
//!
//! Limitations:
//!   - Single vertex per call (`vert: VertKey`, not `verts: &[VertKey]`).
//!   - Cap / profile shapes for the bevel face are not implemented; the
//!     result is a flat N-gon.
//!   - Vertices with degree < 3 are rejected as `DegreeTooLow`.

use std::collections::HashSet;

use bevy::math::Vec3;

use crate::editmesh::cycles::{disk_remove_edge, disk_walk, radial_remove_loop, radial_walk};
use crate::editmesh::ops::face_create::create_face_from_verts_with_material;
use crate::editmesh::types::*;
use crate::newell::newell_normal;

#[derive(Debug)]
pub enum VertexBevelError {
    VertNotFound,
    DegreeTooLow,
    WidthTooSmall,
    Degenerate,
}

#[derive(Debug)]
pub struct VertexBevelResult {
    /// The bevel N-gon face that replaces the corner.
    pub new_face: FaceKey,
    /// The offset vertices, in the bevel face's CCW winding order (viewed
    /// from outside the brush).
    pub new_verts: Vec<VertKey>,
}

/// Bevel `vert` into an N-gon face of width `width`.
pub fn vertex_bevel(
    bmesh: &mut EditMesh,
    vert: VertKey,
    width: f32,
) -> Result<VertexBevelResult, VertexBevelError> {
    if width < 1e-6 {
        return Err(VertexBevelError::WidthTooSmall);
    }
    if !bmesh.verts.contains_key(vert) {
        return Err(VertexBevelError::VertNotFound);
    }

    // 1. Walk the disk cycle to collect incident edges and their "other ends".
    let incident: Vec<(EdgeKey, VertKey)> = disk_walk(bmesh, vert)
        .map(|e| {
            let ed = &bmesh.edges[e];
            let other = if ed.v[0] == vert { ed.v[1] } else { ed.v[0] };
            (e, other)
        })
        .collect();
    if incident.len() < 3 {
        return Err(VertexBevelError::DegreeTooLow);
    }

    let v_pos = bmesh.verts[vert].co;

    // 2. Compute offset positions for each incident edge.
    let mut offsets: Vec<(EdgeKey, VertKey, Vec3)> = Vec::with_capacity(incident.len());
    for (e, other) in &incident {
        let other_pos = bmesh.verts[*other].co;
        let dir = (other_pos - v_pos).normalize_or_zero();
        if dir.length_squared() < 1e-12 {
            return Err(VertexBevelError::Degenerate);
        }
        offsets.push((*e, *other, v_pos + dir * width));
    }

    // 3. Collect all adjacent faces of `v`.
    let mut adjacent_faces: HashSet<FaceKey> = HashSet::new();
    for (e, _other) in &incident {
        for lp in radial_walk(bmesh, *e).collect::<Vec<_>>() {
            adjacent_faces.insert(bmesh.loops[lp].face);
        }
    }

    // 4. Compute the outward direction: average of adjacent face normals.
    let mut outward = Vec3::ZERO;
    for &fk in &adjacent_faces {
        outward += bmesh.faces[fk].normal_cache;
    }
    let outward = outward.normalize_or_zero();
    if outward.length_squared() < 1e-6 {
        return Err(VertexBevelError::Degenerate);
    }

    // 5. Allocate the offset vertices in the EditMesh. Remember which edge
    //    each offset corresponds to so we can substitute properly when
    //    rebuilding adjacent face rings.
    let mut edge_to_offset: std::collections::HashMap<EdgeKey, VertKey> =
        std::collections::HashMap::with_capacity(offsets.len());
    let mut offset_keys: Vec<(EdgeKey, VertKey, Vec3)> = Vec::with_capacity(offsets.len());
    for (e, _other, pos) in &offsets {
        let key = bmesh.add_vert(*pos);
        edge_to_offset.insert(*e, key);
        offset_keys.push((*e, key, *pos));
    }

    // 6. Snapshot each adjacent face's ring (with material) so we can rebuild
    //    after teardown. For each face, walk its ring; whenever we hit `vert`,
    //    substitute with the two offsets corresponding to the two ring-adjacent
    //    edges at that corner (prev_edge and next_edge, both incident at v).
    let mut rebuilds: Vec<(Vec<VertKey>, u32)> = Vec::with_capacity(adjacent_faces.len());
    for &fk in &adjacent_faces {
        let face_data = &bmesh.faces[fk];
        let n = face_data.loop_count as usize;
        let mut new_ring: Vec<VertKey> = Vec::with_capacity(n + 1);
        let mut cur = face_data.loop_first;
        for _ in 0..n {
            let lp = &bmesh.loops[cur];
            let v_here = lp.vert;
            if v_here != vert {
                new_ring.push(v_here);
                cur = lp.next;
                continue;
            }

            // We are at the beveled corner. The two ring-adjacent edges are
            // `prev_loop.edge` (arriving at `vert`) and `lp.edge` (leaving
            // `vert`). Substitute the corner with `[offset_of_prev_edge,
            // offset_of_next_edge]`, in that order, to preserve CCW winding.
            let prev_edge = bmesh.loops[lp.prev].edge;
            let next_edge = lp.edge;
            let off_prev = edge_to_offset
                .get(&prev_edge)
                .copied()
                .ok_or(VertexBevelError::Degenerate)?;
            let off_next = edge_to_offset
                .get(&next_edge)
                .copied()
                .ok_or(VertexBevelError::Degenerate)?;
            new_ring.push(off_prev);
            if off_prev != off_next {
                new_ring.push(off_next);
            }
            cur = lp.next;
        }
        let mat = bmesh.faces[fk].material_idx;
        rebuilds.push((new_ring, mat));
    }

    // 7. Tear down all adjacent faces. Then purge all incident edges (they'll
    //    be re-created by face_create with the offset verts as endpoints).
    //    Finally remove `vert` itself.
    for &fk in &adjacent_faces {
        tear_down_face(bmesh, fk);
    }
    purge_edges_at_vert(bmesh, vert);
    if bmesh.verts.contains_key(vert) {
        bmesh.verts.remove(vert);
    }

    // 8. Recreate each adjacent face from its new ring, preserving material_idx.
    for (ring, mat) in &rebuilds {
        if ring.len() < 3 {
            continue;
        }
        if let Ok(face) = create_face_from_verts_with_material(bmesh, ring, Some(*mat)) {
            let positions: Vec<Vec3> = ring.iter().map(|&k| bmesh.verts[k].co).collect();
            bmesh.faces[face].normal_cache = newell_normal(&positions);
        }
    }

    // 9. Sort the offset verts in bevel-face winding order: project each
    //    `(v_i - v)` onto the plane perpendicular to `outward`, then sort by
    //    polar angle around `outward`. Use one of the offset directions as
    //    the polar reference. CCW around `outward` viewed from outside.
    let mut sorted: Vec<(VertKey, f32)> = Vec::with_capacity(offset_keys.len());
    let ref_dir = {
        let (_, _, first_pos) = offset_keys[0];
        let raw = first_pos - v_pos;
        let projected = raw - outward * raw.dot(outward);
        let len = projected.length();
        if len < 1e-6 {
            return Err(VertexBevelError::Degenerate);
        }
        projected / len
    };
    let bitangent = outward.cross(ref_dir).normalize_or_zero();
    if bitangent.length_squared() < 1e-6 {
        return Err(VertexBevelError::Degenerate);
    }
    for (_e, k, pos) in &offset_keys {
        let raw = *pos - v_pos;
        let projected = raw - outward * raw.dot(outward);
        let x = projected.dot(ref_dir);
        let y = projected.dot(bitangent);
        let angle = y.atan2(x);
        sorted.push((*k, angle));
    }
    sorted.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut bevel_ring: Vec<VertKey> = sorted.into_iter().map(|(k, _)| k).collect();

    // 10. Create the bevel face. Pick winding so its Newell normal aligns
    //     with the outward direction.
    let candidate_positions: Vec<Vec3> = bevel_ring.iter().map(|&k| bmesh.verts[k].co).collect();
    let candidate_normal = newell_normal(&candidate_positions);
    if candidate_normal.dot(outward) < 0.0 {
        bevel_ring.reverse();
    }
    let bevel_mat = next_material_idx(bmesh);
    let bevel_face = create_face_from_verts_with_material(bmesh, &bevel_ring, Some(bevel_mat))
        .map_err(|_| VertexBevelError::Degenerate)?;
    let positions: Vec<Vec3> = bevel_ring.iter().map(|&k| bmesh.verts[k].co).collect();
    bmesh.faces[bevel_face].normal_cache = newell_normal(&positions);

    Ok(VertexBevelResult {
        new_face: bevel_face,
        new_verts: bevel_ring,
    })
}

/// Free `face`'s loops (removing them from radial cycles) and remove the face
/// entry. Edges are NOT touched here; they remain in the slotmap and disk
/// cycles for downstream cleanup.
fn tear_down_face(bmesh: &mut EditMesh, face: FaceKey) {
    if !bmesh.faces.contains_key(face) {
        return;
    }
    let face_data = bmesh.faces[face].clone();
    let n = face_data.loop_count as usize;
    let mut cur = face_data.loop_first;
    let mut loops: Vec<LoopKey> = Vec::with_capacity(n);
    for _ in 0..n {
        loops.push(cur);
        cur = bmesh.loops[cur].next;
    }
    for lp in loops {
        radial_remove_loop(bmesh, lp);
        bmesh.loops.remove(lp);
    }
    bmesh.faces.remove(face);
}

/// Remove every remaining edge incident at `v` (after faces have been torn
/// down). Used to scrub orphaned edges that would otherwise leak when v is
/// deleted, AND so `create_face_from_verts_with_material` doesn't reuse a
/// stale edge whose endpoint is about to vanish.
fn purge_edges_at_vert(bmesh: &mut EditMesh, v: VertKey) {
    if !bmesh.verts.contains_key(v) {
        return;
    }
    let incident: Vec<EdgeKey> = disk_walk(bmesh, v).collect();
    for e in incident {
        disk_remove_edge(bmesh, e);
        bmesh.edges.remove(e);
    }
}

/// Next free `material_idx`: one above the current max. Mirrors the
/// convention used by `create_face_from_verts_with_material` when called with
/// `None`, but lets the caller hold onto the chosen index.
fn next_material_idx(bmesh: &EditMesh) -> u32 {
    bmesh
        .faces
        .values()
        .map(|f| f.material_idx)
        .max()
        .map_or(0, |m| m + 1)
}
