//! Face creation from a CCW-ordered vert ring. Allocates the face, loops, and
//! any missing edges; wires next/prev around the ring, inserts loops into
//! radial cycles, and caches the face normal.

use crate::bmesh::cycles::radial_insert_loop;
use crate::bmesh::ops::edge_create::bm_edge_create;
use crate::bmesh::types::*;
use crate::newell::newell_normal;

#[derive(Debug)]
pub enum FaceCreateError {
    TooFewVerts,
}

pub fn bm_face_create_from_verts(
    bmesh: &mut BMesh,
    verts: &[VertKey],
) -> Result<FaceKey, FaceCreateError> {
    let n = verts.len();
    if n < 3 {
        return Err(FaceCreateError::TooFewVerts);
    }

    // Ensure all edges exist.
    let mut edges: Vec<EdgeKey> = Vec::with_capacity(n);
    for i in 0..n {
        let v0 = verts[i];
        let v1 = verts[(i + 1) % n];
        edges.push(bm_edge_create(bmesh, v0, v1));
    }

    // Allocate face (with placeholder loop_first).
    let face = bmesh.faces.insert(BMFace {
        flag: FaceFlag::empty(),
        material_idx: 0,
        loop_first: LoopKey::default(),
        loop_count: n as u32,
        normal_cache: bevy::math::Vec3::ZERO,
    });

    // Allocate loops.
    let mut loops: Vec<LoopKey> = Vec::with_capacity(n);
    for i in 0..n {
        loops.push(bmesh.loops.insert(BMLoop {
            vert: verts[i],
            edge: edges[i], // walks verts[i] -> verts[i+1]
            face,
            next: LoopKey::default(),
            prev: LoopKey::default(),
            radial_next: LoopKey::default(),
            radial_prev: LoopKey::default(),
        }));
    }

    // Wire ring.
    for i in 0..n {
        let cur = loops[i];
        let nxt = loops[(i + 1) % n];
        let prv = loops[(i + n - 1) % n];
        bmesh.loops[cur].next = nxt;
        bmesh.loops[cur].prev = prv;
    }

    // Wire radial cycles.
    for &lp in &loops {
        radial_insert_loop(bmesh, lp);
    }

    bmesh.faces[face].loop_first = loops[0];

    // Cache normal.
    let positions: Vec<bevy::math::Vec3> =
        verts.iter().map(|&k| bmesh.verts[k].co).collect();
    bmesh.faces[face].normal_cache = newell_normal(&positions);

    Ok(face)
}
