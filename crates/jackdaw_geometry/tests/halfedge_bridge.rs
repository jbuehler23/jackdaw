use bevy_math::Vec3;
use jackdaw_geometry::halfedge::ops::bridge_edge_loops::bridge_edge_loops;
use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::edge_create::create_edge};

fn build_two_squares_no_bridge() -> (
    HalfedgeMesh,
    Vec<jackdaw_geometry::halfedge::EdgeKey>,
    Vec<jackdaw_geometry::halfedge::EdgeKey>,
) {
    let mut mesh = HalfedgeMesh::default();
    // Bottom square at z=0.
    let b0 = mesh.add_vert(Vec3::new(0.0, 0.0, 0.0));
    let b1 = mesh.add_vert(Vec3::new(1.0, 0.0, 0.0));
    let b2 = mesh.add_vert(Vec3::new(1.0, 1.0, 0.0));
    let b3 = mesh.add_vert(Vec3::new(0.0, 1.0, 0.0));
    let edges_b = vec![
        create_edge(&mut mesh, b0, b1),
        create_edge(&mut mesh, b1, b2),
        create_edge(&mut mesh, b2, b3),
        create_edge(&mut mesh, b3, b0),
    ];
    // Top square at z=1.
    let t0 = mesh.add_vert(Vec3::new(0.0, 0.0, 1.0));
    let t1 = mesh.add_vert(Vec3::new(1.0, 0.0, 1.0));
    let t2 = mesh.add_vert(Vec3::new(1.0, 1.0, 1.0));
    let t3 = mesh.add_vert(Vec3::new(0.0, 1.0, 1.0));
    let edges_t = vec![
        create_edge(&mut mesh, t0, t1),
        create_edge(&mut mesh, t1, t2),
        create_edge(&mut mesh, t2, t3),
        create_edge(&mut mesh, t3, t0),
    ];
    (mesh, edges_b, edges_t)
}

#[test]
fn bridge_two_4_vert_squares_creates_4_quads() {
    let (mut mesh, edges_b, edges_t) = build_two_squares_no_bridge();
    let initial_faces = mesh.face_count();
    let initial_edges = mesh.edge_count();
    let result = bridge_edge_loops(&mut mesh, &edges_b, &edges_t).expect("bridge");
    assert_eq!(mesh.face_count(), initial_faces + 4, "+4 quad faces");
    // 4 new "spoke" edges connecting bottom verts to top verts.
    assert_eq!(mesh.edge_count(), initial_edges + 4, "+4 spoke edges");
    mesh.validate().expect("valid");
    assert_eq!(result.new_faces.len(), 4);
    assert_eq!(result.new_edges.len(), 4);
}

#[test]
fn bridge_unequal_loops_errors() {
    let mut mesh = HalfedgeMesh::default();
    let v0 = mesh.add_vert(Vec3::ZERO);
    let v1 = mesh.add_vert(Vec3::X);
    let v2 = mesh.add_vert(Vec3::Y);
    let e_a = vec![
        create_edge(&mut mesh, v0, v1),
        create_edge(&mut mesh, v1, v2),
        create_edge(&mut mesh, v2, v0),
    ];
    let v3 = mesh.add_vert(Vec3::Z);
    let v4 = mesh.add_vert(Vec3::Z + Vec3::X);
    let v5 = mesh.add_vert(Vec3::Z + Vec3::Y);
    let v6 = mesh.add_vert(Vec3::Z + Vec3::new(0.5, 1.5, 0.0));
    let e_b = vec![
        create_edge(&mut mesh, v3, v4),
        create_edge(&mut mesh, v4, v5),
        create_edge(&mut mesh, v5, v6),
        create_edge(&mut mesh, v6, v3),
    ];
    let result = bridge_edge_loops(&mut mesh, &e_a, &e_b);
    assert!(result.is_err(), "unequal loop counts should error");
}
