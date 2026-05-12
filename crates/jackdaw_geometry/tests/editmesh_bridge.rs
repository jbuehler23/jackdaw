use bevy::math::Vec3;
use jackdaw_geometry::editmesh::ops::bridge_edge_loops::bridge_edge_loops;
use jackdaw_geometry::editmesh::{EditMesh, ops::edge_create::create_edge};

fn build_two_squares_no_bridge() -> (
    EditMesh,
    Vec<jackdaw_geometry::editmesh::EdgeKey>,
    Vec<jackdaw_geometry::editmesh::EdgeKey>,
) {
    let mut bmesh = EditMesh::default();
    // Bottom square at z=0.
    let b0 = bmesh.add_vert(Vec3::new(0.0, 0.0, 0.0));
    let b1 = bmesh.add_vert(Vec3::new(1.0, 0.0, 0.0));
    let b2 = bmesh.add_vert(Vec3::new(1.0, 1.0, 0.0));
    let b3 = bmesh.add_vert(Vec3::new(0.0, 1.0, 0.0));
    let edges_b = vec![
        create_edge(&mut bmesh, b0, b1),
        create_edge(&mut bmesh, b1, b2),
        create_edge(&mut bmesh, b2, b3),
        create_edge(&mut bmesh, b3, b0),
    ];
    // Top square at z=1.
    let t0 = bmesh.add_vert(Vec3::new(0.0, 0.0, 1.0));
    let t1 = bmesh.add_vert(Vec3::new(1.0, 0.0, 1.0));
    let t2 = bmesh.add_vert(Vec3::new(1.0, 1.0, 1.0));
    let t3 = bmesh.add_vert(Vec3::new(0.0, 1.0, 1.0));
    let edges_t = vec![
        create_edge(&mut bmesh, t0, t1),
        create_edge(&mut bmesh, t1, t2),
        create_edge(&mut bmesh, t2, t3),
        create_edge(&mut bmesh, t3, t0),
    ];
    (bmesh, edges_b, edges_t)
}

#[test]
fn bridge_two_4_vert_squares_creates_4_quads() {
    let (mut bmesh, edges_b, edges_t) = build_two_squares_no_bridge();
    let initial_faces = bmesh.face_count();
    let initial_edges = bmesh.edge_count();
    let result = bridge_edge_loops(&mut bmesh, &edges_b, &edges_t).expect("bridge");
    assert_eq!(bmesh.face_count(), initial_faces + 4, "+4 quad faces");
    // 4 new "spoke" edges connecting bottom verts to top verts.
    assert_eq!(bmesh.edge_count(), initial_edges + 4, "+4 spoke edges");
    bmesh.validate().expect("valid");
    assert_eq!(result.new_faces.len(), 4);
    assert_eq!(result.new_edges.len(), 4);
}

#[test]
fn bridge_unequal_loops_errors() {
    let mut bmesh = EditMesh::default();
    let v0 = bmesh.add_vert(Vec3::ZERO);
    let v1 = bmesh.add_vert(Vec3::X);
    let v2 = bmesh.add_vert(Vec3::Y);
    let e_a = vec![
        create_edge(&mut bmesh, v0, v1),
        create_edge(&mut bmesh, v1, v2),
        create_edge(&mut bmesh, v2, v0),
    ];
    let v3 = bmesh.add_vert(Vec3::Z);
    let v4 = bmesh.add_vert(Vec3::Z + Vec3::X);
    let v5 = bmesh.add_vert(Vec3::Z + Vec3::Y);
    let v6 = bmesh.add_vert(Vec3::Z + Vec3::new(0.5, 1.5, 0.0));
    let e_b = vec![
        create_edge(&mut bmesh, v3, v4),
        create_edge(&mut bmesh, v4, v5),
        create_edge(&mut bmesh, v5, v6),
        create_edge(&mut bmesh, v6, v3),
    ];
    let result = bridge_edge_loops(&mut bmesh, &e_a, &e_b);
    assert!(result.is_err(), "unequal loop counts should error");
}
