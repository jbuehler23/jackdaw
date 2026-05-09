//! Invariant checker for BMesh. Used in tests; optional in production.
//! Inspired by Blender's `BM_mesh_validate()` (algorithmic reference);
//! implementation original.

use crate::bmesh::cycles::{disk_walk, radial_walk};
use crate::bmesh::types::*;

#[derive(Debug)]
pub enum ValidationError {
    VertWithEdgesHasNoneEdgePtr(VertKey),
    EdgeNotInDiskCycle { edge: EdgeKey, vert: VertKey },
    LoopNotInRadialCycle { loop_key: LoopKey, edge: EdgeKey },
    FaceLoopCountMismatch { face: FaceKey, expected: u32, walked: u32 },
    LoopFaceMismatch { loop_key: LoopKey, expected_face: FaceKey, actual_face: FaceKey },
}

impl BMesh {
    pub fn validate(&self) -> Result<(), ValidationError> {
        // 1. Every vert with at least one incident edge must have edge=Some.
        for (vk, v) in self.verts.iter() {
            let has_edges = self.edges.values().any(|e| e.v[0] == vk || e.v[1] == vk);
            if has_edges && v.edge.is_none() {
                return Err(ValidationError::VertWithEdgesHasNoneEdgePtr(vk));
            }
        }

        // 2. Every edge must be reachable via disk_walk from each of its verts.
        for (ek, e) in self.edges.iter() {
            for side in 0..2 {
                let v_key = e.v[side];
                let mut found = false;
                for walked in disk_walk(self, v_key) {
                    if walked == ek {
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Err(ValidationError::EdgeNotInDiskCycle { edge: ek, vert: v_key });
                }
            }
        }

        // 3. Every loop must be reachable via radial_walk from its edge.
        for (lk, lp) in self.loops.iter() {
            let mut found = false;
            for walked in radial_walk(self, lp.edge) {
                if walked == lk {
                    found = true;
                    break;
                }
            }
            if !found {
                return Err(ValidationError::LoopNotInRadialCycle { loop_key: lk, edge: lp.edge });
            }
        }

        // 4. Every face must have exactly loop_count loops in its ring,
        //    each pointing back at the face.
        for (fk, face) in self.faces.iter() {
            let mut count = 0u32;
            let mut cur = face.loop_first;
            for _ in 0..face.loop_count + 1 {
                let lp = &self.loops[cur];
                if lp.face != fk {
                    return Err(ValidationError::LoopFaceMismatch {
                        loop_key: cur,
                        expected_face: fk,
                        actual_face: lp.face,
                    });
                }
                count += 1;
                cur = lp.next;
                if cur == face.loop_first {
                    break;
                }
            }
            if count != face.loop_count {
                return Err(ValidationError::FaceLoopCountMismatch {
                    face: fk,
                    expected: face.loop_count,
                    walked: count,
                });
            }
        }

        Ok(())
    }
}
