//! Split a face into two by inserting a new edge between two of its existing
//! vertices. Both verts must be in the face's ring and non-adjacent.

use crate::bmesh::cycles::{disk_insert_edge, radial_insert_loop};
use crate::bmesh::types::*;

#[derive(Debug)]
pub enum FaceSplitError {
    FaceNotFound,
    BadVerts,   // either va or vb not in the face's ring
    Adjacent,   // va and vb are consecutive in the ring (1 apart)
    Degenerate, // resulting face would have < 3 verts
}

/// Split `face` into two faces by inserting a new edge between `va` and `vb`.
/// Both verts must already appear in the face's loop ring and must not be
/// adjacent. Returns the key of the newly-created edge.
///
/// Ring model: each loop `l` has `l.vert` = vert it starts at and `l.edge` =
/// edge from `l.vert` to `l.next.vert`. The two bridge loops created here
/// carry `edge = new_edge`, one per new face.
///
/// Partition scheme (no overlap):
///   Forward ring (original face):   ring[ia], ..., ring[ib-1], bridge_orig
///     bridge_orig.vert = vb, .edge = new_edge, .next = ring[ia]
///   Backward ring (new face):       ring[ib], ..., ring[ia-1], bridge_new
///     bridge_new.vert  = va, .edge = new_edge, .next = ring[ib]
///
/// This ensures every original loop ends up in exactly one face.
pub fn bm_face_split(
    bmesh: &mut BMesh,
    face: FaceKey,
    va: VertKey,
    vb: VertKey,
) -> Result<EdgeKey, FaceSplitError> {
    if va == vb {
        return Err(FaceSplitError::BadVerts);
    }

    let face_data = bmesh.faces.get(face).cloned().ok_or(FaceSplitError::FaceNotFound)?;

    // Walk the ring once and collect all loop keys in order.
    let mut ring_loops: Vec<LoopKey> = Vec::with_capacity(face_data.loop_count as usize);
    let mut cur = face_data.loop_first;
    for _ in 0..face_data.loop_count {
        ring_loops.push(cur);
        cur = bmesh.loops[cur].next;
    }

    // Find positions of va and vb in the ring.
    let pos_a = ring_loops.iter().position(|&k| bmesh.loops[k].vert == va);
    let pos_b = ring_loops.iter().position(|&k| bmesh.loops[k].vert == vb);
    let (Some(ia), Some(ib)) = (pos_a, pos_b) else {
        return Err(FaceSplitError::BadVerts);
    };

    let n = ring_loops.len();
    // Forward distance: number of steps from ia to ib going forward (mod n).
    let dist_forward = (ib + n - ia) % n;
    // Backward distance: number of steps from ib to ia going forward (mod n).
    let dist_backward = (ia + n - ib) % n;

    // Adjacent means 1 step apart in either direction.
    if dist_forward == 1 || dist_backward == 1 {
        return Err(FaceSplitError::Adjacent);
    }

    // Each resulting face: forward gets dist_forward loops + 1 bridge = dist_forward+1.
    // Backward gets dist_backward loops + 1 bridge = dist_backward+1.
    // Both must be at least 3.
    if dist_forward + 1 < 3 || dist_backward + 1 < 3 {
        return Err(FaceSplitError::Degenerate);
    }

    // Allocate the new edge with canonical (smaller-key-first) vertex ordering.
    let (v_lo, v_hi) = if va < vb { (va, vb) } else { (vb, va) };
    let new_edge = bmesh.edges.insert(BMEdge {
        v: [v_lo, v_hi],
        flag: EdgeFlag::empty(),
        loop_first: None,
        disk_next: [EdgeKey::default(); 2],
        disk_prev: [EdgeKey::default(); 2],
    });
    disk_insert_edge(bmesh, new_edge);

    // Allocate the new face (inherits material_idx and flag from original).
    let new_face = bmesh.faces.insert(BMFace {
        flag: face_data.flag,
        material_idx: face_data.material_idx,
        loop_first: LoopKey::default(), // patched below
        loop_count: 0,                  // patched below
        normal_cache: face_data.normal_cache,
    });

    // -----------------------------------------------------------------------
    // Collect the two non-overlapping partitions.
    //
    // Forward partition: ring[ia], ring[ia+1], ..., ring[ib-1]  (dist_forward loops)
    // These stay on the original face `face`.
    let forward_loops: Vec<LoopKey> = (0..dist_forward)
        .map(|step| ring_loops[(ia + step) % n])
        .collect();

    // Backward partition: ring[ib], ring[ib+1], ..., ring[ia-1]  (dist_backward loops)
    // These move to the new face `new_face`.
    let backward_loops: Vec<LoopKey> = (0..dist_backward)
        .map(|step| ring_loops[(ib + step) % n])
        .collect();
    // -----------------------------------------------------------------------

    // Create the two bridge loops for the new edge.
    //
    // bridge_orig: on the original face, closes the forward ring back to lp_va.
    //   vert = vb  (the loop "starts at vb"), edge = new_edge (goes vb → va)
    let bridge_orig = bmesh.loops.insert(BMLoop {
        vert: vb,
        edge: new_edge,
        face,
        next: LoopKey::default(),
        prev: LoopKey::default(),
        radial_next: LoopKey::default(),
        radial_prev: LoopKey::default(),
    });

    // bridge_new: on the new face, closes the backward ring back to lp_vb.
    //   vert = va  (the loop "starts at va"), edge = new_edge (goes va → vb)
    let bridge_new = bmesh.loops.insert(BMLoop {
        vert: va,
        edge: new_edge,
        face: new_face,
        next: LoopKey::default(),
        prev: LoopKey::default(),
        radial_next: LoopKey::default(),
        radial_prev: LoopKey::default(),
    });

    // -----------------------------------------------------------------------
    // Wire the original face ring:
    //   forward_loops[0] <-> ... <-> forward_loops[last] <-> bridge_orig <-> (loop back)
    //
    // forward_loops[last] is ring[ib-1].  Its next should now be bridge_orig.
    // bridge_orig's next should be forward_loops[0] (= ring[ia] = lp_va).
    // -----------------------------------------------------------------------
    let last_fwd = *forward_loops.last().unwrap();

    // Wire internal forward chain (already linked from the original ring, just
    // re-confirm prev pointers are consistent — only the boundary seams need
    // changing since the internal links are untouched).
    for i in 0..forward_loops.len() {
        let cur = forward_loops[i];
        let nxt = if i + 1 < forward_loops.len() { forward_loops[i + 1] } else { bridge_orig };
        let prv = if i > 0 { forward_loops[i - 1] } else { bridge_orig };
        bmesh.loops[cur].next = nxt;
        bmesh.loops[cur].prev = prv;
    }
    // Wire bridge_orig into the forward ring.
    bmesh.loops[bridge_orig].next = forward_loops[0];
    bmesh.loops[bridge_orig].prev = last_fwd;

    // -----------------------------------------------------------------------
    // Wire the new face ring:
    //   backward_loops[0] <-> ... <-> backward_loops[last] <-> bridge_new <-> (loop back)
    //
    // backward_loops[last] is ring[ia-1].  Its next should now be bridge_new.
    // bridge_new's next should be backward_loops[0] (= ring[ib] = lp_vb).
    // -----------------------------------------------------------------------
    let last_back = *backward_loops.last().unwrap();

    for i in 0..backward_loops.len() {
        let cur = backward_loops[i];
        let nxt = if i + 1 < backward_loops.len() { backward_loops[i + 1] } else { bridge_new };
        let prv = if i > 0 { backward_loops[i - 1] } else { bridge_new };
        bmesh.loops[cur].next = nxt;
        bmesh.loops[cur].prev = prv;
    }
    // Wire bridge_new into the backward ring.
    bmesh.loops[bridge_new].next = backward_loops[0];
    bmesh.loops[bridge_new].prev = last_back;

    // -----------------------------------------------------------------------
    // Reassign backward loops' face pointer to new_face, and update bridge loops.
    // -----------------------------------------------------------------------
    for &lp in &backward_loops {
        bmesh.loops[lp].face = new_face;
    }
    // (bridge_orig and bridge_new were already created with correct face fields)

    // -----------------------------------------------------------------------
    // Patch face metadata.
    // Each face: partition_len + 1 (for the bridge loop).
    // -----------------------------------------------------------------------
    bmesh.faces[face].loop_first = forward_loops[0];
    bmesh.faces[face].loop_count = (forward_loops.len() + 1) as u32;
    bmesh.faces[new_face].loop_first = backward_loops[0];
    bmesh.faces[new_face].loop_count = (backward_loops.len() + 1) as u32;

    // Insert both bridge loops into new_edge's radial cycle.
    radial_insert_loop(bmesh, bridge_orig);
    radial_insert_loop(bmesh, bridge_new);

    Ok(new_edge)
}
