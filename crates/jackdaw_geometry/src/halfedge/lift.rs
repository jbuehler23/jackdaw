//! Lift `BrushTopology` into in-memory `HalfedgeMesh`. Builds vert table, edge table
//! (with disk cycles), and per-face loop rings (with radial cycles). Inspired
use crate::BrushTopology;
use crate::halfedge::cycles::{disk_insert_edge, radial_insert_loop};
use crate::halfedge::types::*;
use crate::newell::newell_normal;

impl HalfedgeMesh {
    pub fn lift_from_topology(t: &BrushTopology) -> Self {
        let mut mesh = HalfedgeMesh::default();

        // 1) Verts: build a topology-vert-index -> VertKey table.
        let vert_keys: Vec<VertKey> = t
            .vertices
            .iter()
            .map(|v| mesh.add_vert(v.position))
            .collect();

        // 2) Edges: build a topology-edge-index -> EdgeKey table.
        let edge_keys: Vec<EdgeKey> = t
            .edges
            .iter()
            .map(|e| {
                let key = mesh.edges.insert(HalfedgeEdge {
                    v: [vert_keys[e.v[0] as usize], vert_keys[e.v[1] as usize]],
                    flag: EdgeFlag::from_bits_truncate(e.flags.bits()),
                    loop_first: None,
                    disk_next: [EdgeKey::default(); 2],
                    disk_prev: [EdgeKey::default(); 2],
                });
                disk_insert_edge(&mut mesh, key);
                key
            })
            .collect();

        // 3) Polygons -> faces + loops + radial cycles.
        for (face_idx, poly) in t.polygons.iter().enumerate() {
            let start = poly.loop_start as usize;
            let total = poly.loop_total as usize;
            let face_loops_topology: Vec<&_> = t.loops[start..start + total].iter().collect();

            let face_key = mesh.faces.insert(HalfedgeFace {
                flag: FaceFlag::empty(),
                material_idx: face_idx as u32,
                loop_first: LoopKey::default(), // patched below
                loop_count: total as u32,
                normal_cache: bevy_math::Vec3::ZERO,
            });

            let loop_keys: Vec<LoopKey> = face_loops_topology
                .iter()
                .map(|l| {
                    mesh.loops.insert(HalfedgeLoop {
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
                mesh.loops[cur].next = nxt;
                mesh.loops[cur].prev = prv;
            }

            // Wire radial cycles.
            for &lp in &loop_keys {
                radial_insert_loop(&mut mesh, lp);
            }

            // Patch face's loop_first.
            mesh.faces[face_key].loop_first = loop_keys[0];

            // Cache normal (Newell over ring).
            let positions: Vec<bevy_math::Vec3> = (0..total)
                .map(|i| t.vertices[face_loops_topology[i].vert as usize].position)
                .collect();
            mesh.faces[face_key].normal_cache = newell_normal(&positions);
        }

        mesh
    }
}
