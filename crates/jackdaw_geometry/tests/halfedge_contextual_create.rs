use bevy::math::Vec3;
use jackdaw_geometry::halfedge::{
    HalfedgeMesh,
    ops::contextual_create::{ContextualResult, contextual_create},
};

#[test]
fn contextual_create_with_2_unconnected_verts_creates_edge() {
    let mut mesh = HalfedgeMesh::default();
    let v0 = mesh.add_vert(Vec3::ZERO);
    let v1 = mesh.add_vert(Vec3::X);
    let result = contextual_create(&mut mesh, &[v0, v1]).expect("create");
    let ContextualResult::Edge(_) = result else {
        panic!("expected edge result")
    };
    assert_eq!(mesh.edge_count(), 1);
    assert_eq!(mesh.face_count(), 0);
}

#[test]
fn contextual_create_with_3_verts_creates_triangle_face() {
    let mut mesh = HalfedgeMesh::default();
    let v0 = mesh.add_vert(Vec3::ZERO);
    let v1 = mesh.add_vert(Vec3::X);
    let v2 = mesh.add_vert(Vec3::Y);
    let result = contextual_create(&mut mesh, &[v0, v1, v2]).expect("create");
    let ContextualResult::Face(_) = result else {
        panic!("expected face result")
    };
    assert_eq!(mesh.vert_count(), 3);
    assert_eq!(mesh.edge_count(), 3);
    assert_eq!(mesh.face_count(), 1);
    mesh.validate().expect("valid");
}

#[test]
fn contextual_create_with_4_verts_creates_quad_face() {
    let mut mesh = HalfedgeMesh::default();
    let v0 = mesh.add_vert(Vec3::new(0.0, 0.0, 0.0));
    let v1 = mesh.add_vert(Vec3::new(1.0, 0.0, 0.0));
    let v2 = mesh.add_vert(Vec3::new(1.0, 1.0, 0.0));
    let v3 = mesh.add_vert(Vec3::new(0.0, 1.0, 0.0));
    let result = contextual_create(&mut mesh, &[v0, v1, v2, v3]).expect("create");
    let ContextualResult::Face(face) = result else {
        panic!("expected face result")
    };
    assert_eq!(mesh.face_count(), 1);
    assert_eq!(mesh.faces[face].loop_count, 4);
    mesh.validate().expect("valid");
}

#[test]
fn contextual_create_with_2_verts_already_connected_returns_existing_edge() {
    let mut mesh = HalfedgeMesh::default();
    let v0 = mesh.add_vert(Vec3::ZERO);
    let v1 = mesh.add_vert(Vec3::X);
    let result1 = contextual_create(&mut mesh, &[v0, v1]).expect("create");
    let ContextualResult::Edge(e1) = result1 else {
        panic!("expected edge")
    };
    let result2 = contextual_create(&mut mesh, &[v0, v1]).expect("create idempotent");
    let ContextualResult::Edge(e2) = result2 else {
        panic!("expected edge")
    };
    assert_eq!(e1, e2, "should return existing edge, not create duplicate");
    assert_eq!(mesh.edge_count(), 1);
}
