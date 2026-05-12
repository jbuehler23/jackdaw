//! EditMesh topology operations. Each op mutates a `EditMesh` in place and
//! preserves the half-edge invariants (disk + radial cycles, manifold faces).

pub mod bridge_edge_loops;
pub mod connect_verts;
pub mod contextual_create;
pub mod dissolve_edges;
pub mod dissolve_faces;
pub mod dissolve_verts;
pub mod edge_bevel;
pub mod edge_create;
pub mod edge_slide;
pub mod edge_split;
pub mod extrude_face_region;
pub mod face_create;
pub mod face_split;
pub mod inset_face;
pub mod loop_cut;
pub mod remove_doubles;
pub mod subdivide;
pub mod vertex_bevel;
pub mod vertex_slide;
