//! Extrude a face along its normal: duplicate the ring verts (offset along normal by `depth`),
//! connect old ring -> new ring with N side quads. The original face's ring is rewritten to
//! the new (top) ring; N new side-quad faces are added below.
//!
//! Single-face MVP -- multi-face region merging deferred.

use bevy::math::Vec3;

use crate::halfedge::cycles::{disk_insert_edge, radial_insert_loop};
use crate::halfedge::types::*;
use crate::newell::newell_normal;

#[derive(Debug)]
pub enum ExtrudeError {
    FaceNotFound,
    Degenerate,
}

pub struct ExtrudeResult {
    pub new_verts: Vec<VertKey>,
    pub side_faces: Vec<FaceKey>,
    pub top_face: FaceKey,
}

pub fn extrude_face_region(
    mesh: &mut HalfedgeMesh,
    face: FaceKey,
    depth: f32,
) -> Result<ExtrudeResult, ExtrudeError> {
    let face_data = mesh
        .faces
        .get(face)
        .cloned()
        .ok_or(ExtrudeError::FaceNotFound)?;
    let n = face_data.loop_count as usize;
    if n < 3 {
        return Err(ExtrudeError::Degenerate);
    }

    // 1. Walk ring; collect old loop / vert / position.
    let mut old_loops: Vec<LoopKey> = Vec::with_capacity(n);
    let mut old_verts: Vec<VertKey> = Vec::with_capacity(n);
    let mut positions: Vec<Vec3> = Vec::with_capacity(n);
    {
        let mut cur = face_data.loop_first;
        for _ in 0..n {
            old_loops.push(cur);
            let v = mesh.loops[cur].vert;
            old_verts.push(v);
            positions.push(mesh.verts[v].co);
            cur = mesh.loops[cur].next;
        }
    }

    // 2. Compute extrusion direction (face normal).
    let normal = if face_data.normal_cache.length_squared() > 0.5 {
        face_data.normal_cache
    } else {
        newell_normal(&positions)
    };
    let offset = normal * depth;

    // 3. Allocate top-ring verts offset along the face normal.
    let mut new_verts: Vec<VertKey> = Vec::with_capacity(n);
    for i in 0..n {
        let new_pos = positions[i] + offset;
        new_verts.push(mesh.add_vert(new_pos));
    }

    // 4. Allocate top-ring edges (between consecutive new verts) and wall edges (old[i] -> new[i]).
    let mut top_edges: Vec<EdgeKey> = Vec::with_capacity(n);
    for i in 0..n {
        let a = new_verts[i];
        let b = new_verts[(i + 1) % n];
        let (v_lo, v_hi) = if a < b { (a, b) } else { (b, a) };
        let e = mesh.edges.insert(HalfedgeEdge {
            v: [v_lo, v_hi],
            flag: EdgeFlag::empty(),
            loop_first: None,
            disk_next: [EdgeKey::default(); 2],
            disk_prev: [EdgeKey::default(); 2],
        });
        disk_insert_edge(mesh, e);
        top_edges.push(e);
    }
    let mut wall_edges: Vec<EdgeKey> = Vec::with_capacity(n);
    for i in 0..n {
        let a = old_verts[i];
        let b = new_verts[i];
        let (v_lo, v_hi) = if a < b { (a, b) } else { (b, a) };
        let e = mesh.edges.insert(HalfedgeEdge {
            v: [v_lo, v_hi],
            flag: EdgeFlag::empty(),
            loop_first: None,
            disk_next: [EdgeKey::default(); 2],
            disk_prev: [EdgeKey::default(); 2],
        });
        disk_insert_edge(mesh, e);
        wall_edges.push(e);
    }

    // 5. Allocate top-ring loops for the original face (which now becomes the top cap).
    //    New loops walk the new (top) ring: new[0] -> new[1] -> ... -> new[n-1] -> new[0].
    let mut top_loops: Vec<LoopKey> = Vec::with_capacity(n);
    for i in 0..n {
        top_loops.push(mesh.loops.insert(HalfedgeLoop {
            vert: new_verts[i],
            edge: top_edges[i],
            face,
            next: LoopKey::default(),
            prev: LoopKey::default(),
            radial_next: LoopKey::default(),
            radial_prev: LoopKey::default(),
        }));
    }
    for i in 0..n {
        let cur = top_loops[i];
        let nxt = top_loops[(i + 1) % n];
        let prv = top_loops[(i + n - 1) % n];
        mesh.loops[cur].next = nxt;
        mesh.loops[cur].prev = prv;
    }
    for &lp in &top_loops {
        radial_insert_loop(mesh, lp);
    }

    // 6. Allocate N side-quad faces. Each side quad i has ring:
    //    [old[i], old[i+1], new[i+1], new[i]]   (CCW from outside, looking inward)
    //
    // Edges:
    //   L0 (old[i] -> old[i+1]): the ORIGINAL ring edge (still in old_loops[i].edge)
    //   L1 (old[i+1] -> new[i+1]): wall_edges[i+1]
    //   L2 (new[i+1] -> new[i]): top_edges[i] (walked in reverse)
    //   L3 (new[i] -> old[i]): wall_edges[i] (walked in reverse)
    //
    // We REUSE old_loops[i] as L0 of side quad i (preserving radial cycle on the original
    // ring edge). Just rebrand its face and rewire next/prev. Allocate fresh L1, L2, L3.
    let mut side_faces: Vec<FaceKey> = Vec::with_capacity(n);
    for i in 0..n {
        let i_next = (i + 1) % n;

        let side_face = mesh.faces.insert(HalfedgeFace {
            flag: FaceFlag::empty(),
            material_idx: face_data.material_idx,
            loop_first: LoopKey::default(),
            loop_count: 4,
            normal_cache: Vec3::ZERO,
        });
        side_faces.push(side_face);

        // L0: reuse old_loops[i]. Update face pointer; radial cycle already correct.
        let l0 = old_loops[i];
        mesh.loops[l0].face = side_face;

        // L1, L2, L3: allocate fresh.
        let l1 = mesh.loops.insert(HalfedgeLoop {
            vert: old_verts[i_next],
            edge: wall_edges[i_next],
            face: side_face,
            next: LoopKey::default(),
            prev: LoopKey::default(),
            radial_next: LoopKey::default(),
            radial_prev: LoopKey::default(),
        });
        let l2 = mesh.loops.insert(HalfedgeLoop {
            vert: new_verts[i_next],
            edge: top_edges[i],
            face: side_face,
            next: LoopKey::default(),
            prev: LoopKey::default(),
            radial_next: LoopKey::default(),
            radial_prev: LoopKey::default(),
        });
        let l3 = mesh.loops.insert(HalfedgeLoop {
            vert: new_verts[i],
            edge: wall_edges[i],
            face: side_face,
            next: LoopKey::default(),
            prev: LoopKey::default(),
            radial_next: LoopKey::default(),
            radial_prev: LoopKey::default(),
        });

        // Wire ring: l0 -> l1 -> l2 -> l3 -> l0.
        mesh.loops[l0].next = l1;
        mesh.loops[l1].prev = l0;
        mesh.loops[l1].next = l2;
        mesh.loops[l2].prev = l1;
        mesh.loops[l2].next = l3;
        mesh.loops[l3].prev = l2;
        mesh.loops[l3].next = l0;
        mesh.loops[l0].prev = l3;

        // Insert l1, l2, l3 into their edges' radial cycles.
        // l0 is already in its edge's radial cycle (reused from original face).
        radial_insert_loop(mesh, l1);
        radial_insert_loop(mesh, l2);
        radial_insert_loop(mesh, l3);

        mesh.faces[side_face].loop_first = l0;

        // Cache normal via Newell over the four side-quad verts.
        let ring_pos = [
            mesh.verts[old_verts[i]].co,
            mesh.verts[old_verts[i_next]].co,
            mesh.verts[new_verts[i_next]].co,
            mesh.verts[new_verts[i]].co,
        ];
        mesh.faces[side_face].normal_cache = newell_normal(&ring_pos);
    }

    // 7. Update top face metadata (was the original face, now the top cap).
    mesh.faces[face].loop_first = top_loops[0];
    mesh.faces[face].loop_count = n as u32;
    let top_pos: Vec<Vec3> = new_verts.iter().map(|&k| mesh.verts[k].co).collect();
    mesh.faces[face].normal_cache = newell_normal(&top_pos);

    Ok(ExtrudeResult {
        new_verts,
        side_faces,
        top_face: face,
    })
}
