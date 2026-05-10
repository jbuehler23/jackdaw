//! Connect selected verts that share a face with new edges, splitting the face.
//! Wraps `bm_face_split`. Silently skips pairs that are adjacent in their
//! face's ring (`FaceSplitError::Adjacent`).

use std::collections::HashSet;

use crate::bmesh::ops::face_split::bm_face_split;
use crate::bmesh::types::*;

#[derive(Debug)]
pub enum ConnectError {
    NotEnoughVerts,
}

pub struct ConnectResult {
    pub new_edges: Vec<EdgeKey>,
    pub new_faces: Vec<FaceKey>,
}

/// Connect selected verts that lie in the same face with new edges, splitting
/// the face accordingly. Verts that are adjacent in the ring (already connected
/// by an edge) are skipped silently.
///
/// Returns `ConnectError::NotEnoughVerts` if fewer than 2 verts are provided.
pub fn connect_verts(
    bmesh: &mut BMesh,
    selected: &[VertKey],
) -> Result<ConnectResult, ConnectError> {
    if selected.len() < 2 {
        return Err(ConnectError::NotEnoughVerts);
    }

    let selected_set: HashSet<VertKey> = selected.iter().copied().collect();
    let mut new_edges_out: Vec<EdgeKey> = Vec::new();
    let mut new_faces_out: Vec<FaceKey> = Vec::new();

    // Snapshot face keys so we don't iterate while mutating.
    let face_keys: Vec<FaceKey> = bmesh.faces.keys().collect();

    for face in face_keys {
        // Walk the face ring to find which selected verts lie on it.
        let face_data = match bmesh.faces.get(face) {
            Some(f) => f.clone(),
            None => continue, // face was destroyed or replaced by an earlier split
        };

        let mut on_face: Vec<VertKey> = Vec::new();
        let mut cur = face_data.loop_first;
        for _ in 0..face_data.loop_count {
            let v = bmesh.loops[cur].vert;
            if selected_set.contains(&v) {
                on_face.push(v);
            }
            cur = bmesh.loops[cur].next;
        }

        if on_face.len() < 2 {
            continue;
        }

        // Connect on_face[0] -> on_face[1], on_face[1] -> on_face[2], etc.
        // After each split the original face shrinks; subsequent pairs may live
        // in either the original or the new sub-face. Re-query the correct
        // target face for each pair before calling bm_face_split.
        for window in on_face.windows(2) {
            let va = window[0];
            let vb = window[1];

            // Find which face currently contains both va and vb.
            let target_face = match find_face_with_verts(bmesh, va, vb) {
                Some(fk) => fk,
                None => continue, // no shared face exists any more; skip
            };

            let face_count_before = bmesh.faces.len();
            match bm_face_split(bmesh, target_face, va, vb) {
                Ok(new_edge) => {
                    new_edges_out.push(new_edge);
                    // Collect any faces that were inserted since before the call.
                    // bm_face_split inserts exactly one new face per successful call,
                    // but we discover it by scanning rather than relying on that detail.
                    let face_count_after = bmesh.faces.len();
                    if face_count_after > face_count_before {
                        // The new face(s) are the ones whose keys weren't in the
                        // pre-split snapshot. Accumulate them for the caller.
                        let pre_keys: HashSet<FaceKey> = bmesh
                            .faces
                            .keys()
                            .take(face_count_before)
                            .collect();
                        for (fk, _) in bmesh.faces.iter() {
                            if !pre_keys.contains(&fk) {
                                new_faces_out.push(fk);
                            }
                        }
                    }
                }
                Err(_) => {
                    // Adjacent, degenerate, or bad-verts — skip silently.
                }
            }
        }
    }

    Ok(ConnectResult {
        new_edges: new_edges_out,
        new_faces: new_faces_out,
    })
}

/// Return the key of any face whose ring contains both `va` and `vb`.
fn find_face_with_verts(bmesh: &BMesh, va: VertKey, vb: VertKey) -> Option<FaceKey> {
    for (fk, f) in bmesh.faces.iter() {
        let mut has_a = false;
        let mut has_b = false;
        let mut cur = f.loop_first;
        for _ in 0..f.loop_count {
            let v = bmesh.loops[cur].vert;
            if v == va {
                has_a = true;
            }
            if v == vb {
                has_b = true;
            }
            cur = bmesh.loops[cur].next;
        }
        if has_a && has_b {
            return Some(fk);
        }
    }
    None
}
