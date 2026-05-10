//! Topology operators for brush editing: loop cut, subdivide, knife, inset,
//! extrude, bevel, etc. Each operator wraps a BMesh op from `jackdaw_geometry::bmesh::ops`,
//! handles selection mapping, syncs `Brush::faces[i].plane` + `Brush::topology` from
//! the mutated BMesh, and fires `SetBrush` for undo.

pub mod bridge_edge_loops;
pub mod connect_verts;
pub mod dissolve_edges;
pub mod dissolve_faces;
pub mod dissolve_verts;
pub mod edge_slide;
pub mod extrude;
pub mod inset;
pub mod loop_cut;
pub mod make_edge_face;
pub mod merge_by_distance;
pub mod subdivide;
pub mod vertex_slide;
pub mod weld_selected;
