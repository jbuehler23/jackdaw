//! F-key behavior: dispatch on selection length to create an edge (2 verts)
//! or a face (3+ verts).

use crate::editmesh::ops::edge_create::create_edge;
use crate::editmesh::ops::face_create::create_face_from_verts;
use crate::editmesh::types::*;

#[derive(Debug)]
pub enum ContextualError {
    TooFewVerts,
}

pub enum ContextualResult {
    Edge(EdgeKey),
    Face(FaceKey),
}

pub fn contextual_create(
    bmesh: &mut EditMesh,
    verts: &[VertKey],
) -> Result<ContextualResult, ContextualError> {
    match verts.len() {
        0 | 1 => Err(ContextualError::TooFewVerts),
        2 => Ok(ContextualResult::Edge(create_edge(bmesh, verts[0], verts[1]))),
        _ => {
            let face = create_face_from_verts(bmesh, verts)
                .map_err(|_| ContextualError::TooFewVerts)?;
            Ok(ContextualResult::Face(face))
        }
    }
}
