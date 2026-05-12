//! Lift `BrushTopology` into in-memory `EditMesh`. Builds vert table, edge table
//! (with disk cycles), and per-face loop rings (with radial cycles). Inspired
//! by Blender's `BM_mesh_bm_from_me()` (algorithmic reference); implementation
//! original.

use crate::BrushTopology;
use crate::editmesh::cycles::{disk_insert_edge, radial_insert_loop};
use crate::editmesh::types::*;
use crate::newell::newell_normal;

impl EditMesh {
    pub fn lift_from_topology(t: &BrushTopology) -> Self {
        let mut bmesh = EditMesh::default();

        // 1) Verts: build a topology-vert-index -> VertKey table.
        let vert_keys: Vec<VertKey> = t
            .vertices
            .iter()
            .map(|v| bmesh.add_vert(v.position))
            .collect();

        // 2) Edges: build a topology-edge-index -> EdgeKey table.
        let edge_keys: Vec<EdgeKey> = t
            .edges
            .iter()
            .map(|e| {
                let key = bmesh.edges.insert(EditEdge {
                    v: [vert_keys[e.v[0] as usize], vert_keys[e.v[1] as usize]],
                    flag: EdgeFlag::from_bits_truncate(e.flags.bits()),
                    loop_first: None,
                    disk_next: [EdgeKey::default(); 2],
                    disk_prev: [EdgeKey::default(); 2],
                });
                disk_insert_edge(&mut bmesh, key);
                key
            })
            .collect();

        // 3) Polygons -> faces + loops + radial cycles.
        for (face_idx, poly) in t.polygons.iter().enumerate() {
            let start = poly.loop_start as usize;
            let total = poly.loop_total as usize;
            let face_loops_topology: Vec<&_> = t.loops[start..start + total].iter().collect();

            let face_key = bmesh.faces.insert(EditFace {
                flag: FaceFlag::empty(),
                material_idx: face_idx as u32,
                loop_first: LoopKey::default(), // patched below
                loop_count: total as u32,
                normal_cache: bevy::math::Vec3::ZERO,
            });

            let loop_keys: Vec<LoopKey> = face_loops_topology
                .iter()
                .map(|l| {
                    bmesh.loops.insert(EditLoop {
                        vert: vert_keys[l.vert as usize],
                        edge: edge_keys[l.edge as usize],
                        face: face_key,
                        next: LoopKey::default(),
                        prev: LoopKey::default(),
                        radial_next: LoopKey::default(),
                        radial_prev: LoopKey::default(),
                    })
                })
                .collect();

            // Wire next / prev around the face ring.
            for i in 0..total {
                let cur = loop_keys[i];
                let nxt = loop_keys[(i + 1) % total];
                let prv = loop_keys[(i + total - 1) % total];
                bmesh.loops[cur].next = nxt;
                bmesh.loops[cur].prev = prv;
            }

            // Wire radial cycles.
            for &lp in &loop_keys {
                radial_insert_loop(&mut bmesh, lp);
            }

            // Patch face's loop_first.
            bmesh.faces[face_key].loop_first = loop_keys[0];

            // Cache normal (Newell over ring).
            let positions: Vec<bevy::math::Vec3> = (0..total)
                .map(|i| t.vertices[face_loops_topology[i].vert as usize].position)
                .collect();
            bmesh.faces[face_key].normal_cache = newell_normal(&positions);
        }

        bmesh
    }
}
