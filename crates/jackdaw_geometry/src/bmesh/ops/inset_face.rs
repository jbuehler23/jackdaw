//! Inset a face: shrink its ring inward along the face plane by `amount`.
//! Connects old ring -> new inner ring with N side quads. The original face's
//! ring is rewritten to the inner ring; N new side-quad faces are added.
//!
//! New geometry per inset on an N-ring face:
//!   - N inner verts (offset = old_pos + inward_dir * amount, projected on face plane)
//!   - N inner-ring edges
//!   - N wall edges (old[i] -> new[i])
//!   - N side-quad faces (CCW from outside): [old[i], old[i+1], new[i+1], new[i]]

use bevy::math::Vec3;

use crate::bmesh::cycles::{disk_insert_edge, radial_insert_loop};
use crate::bmesh::types::*;
use crate::newell::newell_normal;

#[derive(Debug)]
pub enum InsetError {
    FaceNotFound,
    Degenerate,
}

pub struct InsetResult {
    pub new_verts: Vec<VertKey>,
    pub side_faces: Vec<FaceKey>,
    pub inner_face: FaceKey,
}

pub fn inset_face(bmesh: &mut BMesh, face: FaceKey, amount: f32) -> Result<InsetResult, InsetError> {
    let face_data = bmesh.faces.get(face).cloned().ok_or(InsetError::FaceNotFound)?;
    let n = face_data.loop_count as usize;
    if n < 3 {
        return Err(InsetError::Degenerate);
    }

    // 1. Walk ring; collect old loop keys, vert keys, positions.
    let mut old_loops: Vec<LoopKey> = Vec::with_capacity(n);
    let mut old_verts: Vec<VertKey> = Vec::with_capacity(n);
    let mut positions: Vec<Vec3> = Vec::with_capacity(n);
    {
        let mut cur = face_data.loop_first;
        for _ in 0..n {
            old_loops.push(cur);
            let v = bmesh.loops[cur].vert;
            old_verts.push(v);
            positions.push(bmesh.verts[v].co);
            cur = bmesh.loops[cur].next;
        }
    }

    // 2. Compute centroid + face normal + inward direction per vert.
    let centroid: Vec3 = positions.iter().copied().sum::<Vec3>() / n as f32;
    let face_normal = if face_data.normal_cache.length_squared() > 0.5 {
        face_data.normal_cache
    } else {
        newell_normal(&positions)
    };

    // 3. Allocate inner verts.
    //    For each old vert, move toward the centroid by `amount`, projected on the face plane.
    let mut new_verts: Vec<VertKey> = Vec::with_capacity(n);
    for i in 0..n {
        let to_centroid = centroid - positions[i];
        // Project onto the face plane (remove normal component).
        let inward = to_centroid - face_normal * to_centroid.dot(face_normal);
        let inward_norm = inward.normalize_or_zero();
        let new_pos = positions[i] + inward_norm * amount;
        new_verts.push(bmesh.add_vert(new_pos));
    }

    // 4. Allocate inner-ring edges: new[i] -- new[i+1].
    let mut inner_edges: Vec<EdgeKey> = Vec::with_capacity(n);
    for i in 0..n {
        let a = new_verts[i];
        let b = new_verts[(i + 1) % n];
        let (v_lo, v_hi) = if a < b { (a, b) } else { (b, a) };
        let e = bmesh.edges.insert(BMEdge {
            v: [v_lo, v_hi],
            flag: EdgeFlag::empty(),
            loop_first: None,
            disk_next: [EdgeKey::default(); 2],
            disk_prev: [EdgeKey::default(); 2],
        });
        disk_insert_edge(bmesh, e);
        inner_edges.push(e);
    }

    // 5. Allocate wall edges: old[i] -- new[i].
    let mut wall_edges: Vec<EdgeKey> = Vec::with_capacity(n);
    for i in 0..n {
        let a = old_verts[i];
        let b = new_verts[i];
        let (v_lo, v_hi) = if a < b { (a, b) } else { (b, a) };
        let e = bmesh.edges.insert(BMEdge {
            v: [v_lo, v_hi],
            flag: EdgeFlag::empty(),
            loop_first: None,
            disk_next: [EdgeKey::default(); 2],
            disk_prev: [EdgeKey::default(); 2],
        });
        disk_insert_edge(bmesh, e);
        wall_edges.push(e);
    }

    // 6. Rewire the inner face: allocate N fresh loops over the inner ring.
    //    The old loops are freed from the original face and will be reused as
    //    the outer ring of the side quads (step 7).
    //
    //    inner_loops[i]: vert = new[i], edge = inner_edges[i] (walks new[i] -> new[i+1]).
    let mut inner_loops: Vec<LoopKey> = Vec::with_capacity(n);
    for i in 0..n {
        inner_loops.push(bmesh.loops.insert(BMLoop {
            vert: new_verts[i],
            edge: inner_edges[i],
            face,
            next: LoopKey::default(),
            prev: LoopKey::default(),
            radial_next: LoopKey::default(),
            radial_prev: LoopKey::default(),
        }));
    }
    // Wire next/prev around the inner face ring.
    for i in 0..n {
        let cur = inner_loops[i];
        let nxt = inner_loops[(i + 1) % n];
        let prv = inner_loops[(i + n - 1) % n];
        bmesh.loops[cur].next = nxt;
        bmesh.loops[cur].prev = prv;
    }
    // Insert inner loops into their edges' radial cycles.
    for &lp in &inner_loops {
        radial_insert_loop(bmesh, lp);
    }

    // 7. Allocate N side-quad faces. Side quad i covers:
    //      old[i], old[i+1], new[i+1], new[i]
    //
    //    Edge reference per side quad walk:
    //      L0: old[i]    -> old[i+1]  = old_loops[i].edge  (the original ring edge)
    //      L1: old[i+1]  -> new[i+1]  = wall_edges[i+1]
    //      L2: new[i+1]  -> new[i]    = inner_edges[i]  (walked "backward" but same edge)
    //      L3: new[i]    -> old[i]    = wall_edges[i]   (walked "backward" but same edge)
    //
    //    We REUSE old_loops[i] as L0 for the side quad. old_loops[i] already sits in the
    //    radial cycle of its edge (the original ring edge between old[i] and old[i+1]);
    //    we simply update its face pointer and rewire next/prev. This preserves the radial
    //    link so the original ring edge's radial cycle is automatically correct.
    let mut side_faces: Vec<FaceKey> = Vec::with_capacity(n);
    for i in 0..n {
        let i_next = (i + 1) % n;

        // Allocate the side-quad face (loop_first patched below).
        let side_face = bmesh.faces.insert(BMFace {
            flag: FaceFlag::empty(),
            material_idx: face_data.material_idx,
            loop_first: LoopKey::default(),
            loop_count: 4,
            normal_cache: Vec3::ZERO,
        });
        side_faces.push(side_face);

        // L0: reuse old_loops[i]. Update face pointer; radial cycle is already correct.
        let l0 = old_loops[i];
        bmesh.loops[l0].face = side_face;

        // L1, L2, L3: allocate fresh.
        let l1 = bmesh.loops.insert(BMLoop {
            vert: old_verts[i_next],
            edge: wall_edges[i_next],
            face: side_face,
            next: LoopKey::default(),
            prev: LoopKey::default(),
            radial_next: LoopKey::default(),
            radial_prev: LoopKey::default(),
        });
        let l2 = bmesh.loops.insert(BMLoop {
            vert: new_verts[i_next],
            edge: inner_edges[i],
            face: side_face,
            next: LoopKey::default(),
            prev: LoopKey::default(),
            radial_next: LoopKey::default(),
            radial_prev: LoopKey::default(),
        });
        let l3 = bmesh.loops.insert(BMLoop {
            vert: new_verts[i],
            edge: wall_edges[i],
            face: side_face,
            next: LoopKey::default(),
            prev: LoopKey::default(),
            radial_next: LoopKey::default(),
            radial_prev: LoopKey::default(),
        });

        // Wire ring: l0 -> l1 -> l2 -> l3 -> l0.
        bmesh.loops[l0].next = l1;
        bmesh.loops[l1].prev = l0;
        bmesh.loops[l1].next = l2;
        bmesh.loops[l2].prev = l1;
        bmesh.loops[l2].next = l3;
        bmesh.loops[l3].prev = l2;
        bmesh.loops[l3].next = l0;
        bmesh.loops[l0].prev = l3;

        // Insert l1, l2, l3 into their edges' radial cycles.
        // l0 is already in its edge's radial cycle (reused from original face).
        radial_insert_loop(bmesh, l1);
        radial_insert_loop(bmesh, l2);
        radial_insert_loop(bmesh, l3);

        bmesh.faces[side_face].loop_first = l0;

        // Cache normal via Newell over the four side-quad verts.
        let ring_pos = [
            bmesh.verts[old_verts[i]].co,
            bmesh.verts[old_verts[i_next]].co,
            bmesh.verts[new_verts[i_next]].co,
            bmesh.verts[new_verts[i]].co,
        ];
        bmesh.faces[side_face].normal_cache = newell_normal(&ring_pos);
    }

    // 8. Update the inner face's metadata.
    bmesh.faces[face].loop_first = inner_loops[0];
    bmesh.faces[face].loop_count = n as u32;
    let inner_pos: Vec<Vec3> = new_verts.iter().map(|&k| bmesh.verts[k].co).collect();
    bmesh.faces[face].normal_cache = newell_normal(&inner_pos);

    Ok(InsetResult { new_verts, side_faces, inner_face: face })
}
