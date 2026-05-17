use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::remove_doubles::remove_doubles};
use jackdaw_jsn::Brush;
use slotmap::Key;

#[test]
fn merge_two_coincident_verts_collapses_to_one() {
    // Build a tiny scenario: a cube whose two adjacent corners are coincident
    // (impossible in practice but sufficient for a unit test).
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let initial_verts = mesh.vert_count();
    // Move one vertex onto another to create a "double".
    let mut keys: Vec<_> = mesh.verts.keys().collect();
    keys.sort_by_key(|k| k.data().as_ffi());
    let v_target = keys[0];
    let v_to_merge = keys[1];
    let target_pos = mesh.verts[v_target].co;
    mesh.verts[v_to_merge].co = target_pos;
    let result = remove_doubles(&mut mesh, 0.001).expect("merge");
    assert_eq!(mesh.vert_count(), initial_verts - 1, "one vert collapsed");
    assert!(result.merged_verts >= 1);
    mesh.validate().expect("valid after merge");
}

#[test]
fn merge_with_no_close_verts_is_noop() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let initial_verts = mesh.vert_count();
    let initial_edges = mesh.edge_count();
    let initial_faces = mesh.face_count();
    let result = remove_doubles(&mut mesh, 0.001).expect("merge");
    assert_eq!(mesh.vert_count(), initial_verts);
    assert_eq!(mesh.edge_count(), initial_edges);
    assert_eq!(mesh.face_count(), initial_faces);
    assert_eq!(result.merged_verts, 0);
}
