//! Flatten `EditMesh` back to `BrushTopology`. Re-keys SlotMap keys to dense u32
//! indices. Inspired by Blender's `BM_mesh_bm_to_me()` (algorithmic reference);
//! implementation original.

use std::collections::HashMap;

use crate::EdgeFlag as TopologyEdgeFlag;
use crate::editmesh::types::*;
use crate::topology::{BrushTopology, MeshEdge, MeshLoop, MeshPoly, MeshVert};

impl EditMesh {
    pub fn flatten_to_topology(&self) -> BrushTopology {
        // Vertex re-keying.
        let mut vert_idx: HashMap<VertKey, u32> = HashMap::with_capacity(self.verts.len());
        let mut vertices: Vec<MeshVert> = Vec::with_capacity(self.verts.len());
        for (k, v) in self.verts.iter() {
            vert_idx.insert(k, vertices.len() as u32);
            vertices.push(MeshVert { position: v.co });
        }

        // Edge re-keying with canonical (v[0] < v[1]) ordering.
        let mut edge_idx: HashMap<EdgeKey, u32> = HashMap::with_capacity(self.edges.len());
        let mut edges: Vec<MeshEdge> = Vec::with_capacity(self.edges.len());
        for (k, e) in self.edges.iter() {
            let v0 = vert_idx[&e.v[0]];
            let v1 = vert_idx[&e.v[1]];
            let (a, b) = if v0 <= v1 { (v0, v1) } else { (v1, v0) };
            edge_idx.insert(k, edges.len() as u32);
            edges.push(MeshEdge {
                v: [a, b],
                flags: TopologyEdgeFlag::from_bits_truncate(e.flag.bits()),
            });
        }

        // Faces -> polygons + loops, sorted by material_idx so parallel arrays line up
        // with `Brush::faces`.
        let mut polygons: Vec<MeshPoly> = Vec::with_capacity(self.faces.len());
        let mut loops: Vec<MeshLoop> = Vec::new();
        let mut faces_sorted: Vec<_> = self.faces.iter().collect();
        faces_sorted.sort_by_key(|(_, f)| f.material_idx);
        for (_, face) in faces_sorted {
            let loop_start = loops.len() as u32;
            let mut lk = face.loop_first;
            for _ in 0..face.loop_count {
                let lp = &self.loops[lk];
                loops.push(MeshLoop {
                    vert: vert_idx[&lp.vert],
                    edge: edge_idx[&lp.edge],
                });
                lk = lp.next;
            }
            polygons.push(MeshPoly {
                loop_start,
                loop_total: face.loop_count,
            });
        }

        BrushTopology {
            vertices,
            edges,
            polygons,
            loops,
            attributes: Default::default(),
        }
    }
}
