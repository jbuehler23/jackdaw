//! Topology operators for brush editing: loop cut, subdivide, knife, inset,
//! extrude, bevel, etc. Each operator wraps a EditMesh op from `jackdaw_geometry::editmesh::ops`,
//! handles selection mapping, syncs `Brush::faces[i].plane` + `Brush::topology` from
//! the mutated EditMesh, and fires `SetBrush` for undo.

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
pub mod select_invert;
pub mod select_less;
pub mod select_linked;
pub mod select_loop;
pub mod select_more;
pub mod select_ring;
pub mod uv_align_to_edge;
pub mod uv_fit_to_face;
pub mod uv_reset_axes;
pub mod uv_rotate_90;
pub mod uv_texel_density;
pub mod uv_world_aligned;
pub mod reconvexify;
pub mod weld_selected;
