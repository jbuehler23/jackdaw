//! F-key behavior: dispatch on selection length to create an edge (2 verts)
//! or a face (3+ verts).

use crate::halfedge::ops::edge_create::create_edge;
use crate::halfedge::ops::face_create::create_face_from_verts;
use crate::halfedge::types::*;

#[derive(Debug)]
pub enum ContextualError {
    TooFewVerts,
}

pub enum ContextualResult {
    Edge(EdgeKey),
    Face(FaceKey),
}

pub fn contextual_create(
    mesh: &mut HalfedgeMesh,
    verts: &[VertKey],
) -> Result<ContextualResult, ContextualError> {
    match verts.len() {
        0 | 1 => Err(ContextualError::TooFewVerts),
        2 => Ok(ContextualResult::Edge(create_edge(
            mesh, verts[0], verts[1],
        ))),
        _ => {
            let face =
                create_face_from_verts(mesh, verts).map_err(|_| ContextualError::TooFewVerts)?;
            Ok(ContextualResult::Face(face))
        }
    }
}
