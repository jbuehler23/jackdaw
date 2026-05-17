//! Dissolve faces: remove the face entirely, leaving its boundary edges as
//! "wire" (edges with no incident face). Verts and edges remain.

use crate::halfedge::cycles::radial_remove_loop;
use crate::halfedge::types::*;

#[derive(Debug)]
pub enum DissolveError {
    EmptyInput,
}

pub struct DissolveFacesResult {
    pub removed_faces: usize,
}

pub fn dissolve_faces(
    mesh: &mut HalfedgeMesh,
    faces: &[FaceKey],
) -> Result<DissolveFacesResult, DissolveError> {
    if faces.is_empty() {
        return Err(DissolveError::EmptyInput);
    }
    let mut removed = 0;
    for &face in faces {
        if dissolve_one_face(mesh, face) {
            removed += 1;
        }
    }
    Ok(DissolveFacesResult {
        removed_faces: removed,
    })
}

fn dissolve_one_face(mesh: &mut HalfedgeMesh, face: FaceKey) -> bool {
    if !mesh.faces.contains_key(face) {
        return false;
    }
    // Walk the face's ring and collect all its loops.
    let face_data = mesh.faces[face].clone();
    let mut loops_to_remove: Vec<LoopKey> = Vec::with_capacity(face_data.loop_count as usize);
    let mut cur = face_data.loop_first;
    for _ in 0..face_data.loop_count {
        loops_to_remove.push(cur);
        cur = mesh.loops[cur].next;
    }
    // Remove each loop from its edge's radial cycle, then drop it.
    for &lp in &loops_to_remove {
        radial_remove_loop(mesh, lp);
        mesh.loops.remove(lp);
    }
    // Drop the face.
    mesh.faces.remove(face);
    true
}
