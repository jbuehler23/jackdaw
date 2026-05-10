//! F-key behavior: dispatch on selection length to create an edge (2 verts)
//! or a face (3+ verts).

use crate::bmesh::ops::edge_create::bm_edge_create;
use crate::bmesh::ops::face_create::bm_face_create_from_verts;
use crate::bmesh::types::*;

#[derive(Debug)]
pub enum ContextualError {
    TooFewVerts,
}

pub enum ContextualResult {
    Edge(EdgeKey),
    Face(FaceKey),
}

pub fn contextual_create(
    bmesh: &mut BMesh,
    verts: &[VertKey],
) -> Result<ContextualResult, ContextualError> {
    match verts.len() {
        0 | 1 => Err(ContextualError::TooFewVerts),
        2 => Ok(ContextualResult::Edge(bm_edge_create(bmesh, verts[0], verts[1]))),
        _ => {
            let face = bm_face_create_from_verts(bmesh, verts)
                .map_err(|_| ContextualError::TooFewVerts)?;
            Ok(ContextualResult::Face(face))
        }
    }
}
