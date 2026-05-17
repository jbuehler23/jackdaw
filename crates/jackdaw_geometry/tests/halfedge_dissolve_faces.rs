use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::dissolve_faces::dissolve_faces};
use jackdaw_jsn::Brush;

#[test]
fn dissolve_one_face_of_cube_leaves_5_faces_with_hole() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let face = mesh.faces.keys().next().unwrap();
    let initial_verts = mesh.vert_count();
    let initial_edges = mesh.edge_count();
    let result = dissolve_faces(&mut mesh, &[face]).expect("dissolve");
    assert_eq!(mesh.face_count(), 5, "5 faces remain");
    // Verts and edges should be unchanged (edges become "wire" but don't go away).
    assert_eq!(mesh.vert_count(), initial_verts);
    assert_eq!(mesh.edge_count(), initial_edges);
    // 4 loops removed (one face had 4 loops).
    mesh.validate().expect("valid after dissolve");
    assert_eq!(result.removed_faces, 1);
}

#[test]
fn dissolve_all_faces_of_cube_leaves_no_faces_but_keeps_verts_edges() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let all_faces: Vec<_> = mesh.faces.keys().collect();
    let initial_verts = mesh.vert_count();
    let initial_edges = mesh.edge_count();
    let result = dissolve_faces(&mut mesh, &all_faces).expect("dissolve");
    assert_eq!(mesh.face_count(), 0);
    assert_eq!(mesh.loop_count(), 0);
    assert_eq!(mesh.vert_count(), initial_verts);
    assert_eq!(mesh.edge_count(), initial_edges);
    mesh.validate().expect("valid after dissolve all");
    assert_eq!(result.removed_faces, 6);
}
