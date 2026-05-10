use jackdaw_geometry::bmesh::{BMesh, ops::dissolve_faces::dissolve_faces};
use jackdaw_jsn::Brush;

#[test]
fn dissolve_one_face_of_cube_leaves_5_faces_with_hole() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = BMesh::lift_from_topology(&brush.topology);
    let face = bmesh.faces.keys().next().unwrap();
    let initial_verts = bmesh.vert_count();
    let initial_edges = bmesh.edge_count();
    let result = dissolve_faces(&mut bmesh, &[face]).expect("dissolve");
    assert_eq!(bmesh.face_count(), 5, "5 faces remain");
    // Verts and edges should be unchanged (edges become "wire" but don't go away).
    assert_eq!(bmesh.vert_count(), initial_verts);
    assert_eq!(bmesh.edge_count(), initial_edges);
    // 4 loops removed (one face had 4 loops).
    bmesh.validate().expect("valid after dissolve");
    assert_eq!(result.removed_faces, 1);
}

#[test]
fn dissolve_all_faces_of_cube_leaves_no_faces_but_keeps_verts_edges() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = BMesh::lift_from_topology(&brush.topology);
    let all_faces: Vec<_> = bmesh.faces.keys().collect();
    let initial_verts = bmesh.vert_count();
    let initial_edges = bmesh.edge_count();
    let result = dissolve_faces(&mut bmesh, &all_faces).expect("dissolve");
    assert_eq!(bmesh.face_count(), 0);
    assert_eq!(bmesh.loop_count(), 0);
    assert_eq!(bmesh.vert_count(), initial_verts);
    assert_eq!(bmesh.edge_count(), initial_edges);
    bmesh.validate().expect("valid after dissolve all");
    assert_eq!(result.removed_faces, 6);
}
