use bevy_math::Vec3;
use jackdaw_geometry::halfedge::{HalfedgeMesh, VertFlag};

#[test]
fn halfedge_default_is_empty() {
    let m = HalfedgeMesh::default();
    assert_eq!(m.vert_count(), 0);
    assert_eq!(m.edge_count(), 0);
    assert_eq!(m.loop_count(), 0);
    assert_eq!(m.face_count(), 0);
}

#[test]
fn halfedge_add_vert_returns_key() {
    let mut m = HalfedgeMesh::default();
    let k = m.add_vert(Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(m.vert_count(), 1);
    assert!(m.verts.contains_key(k));
    let v = &m.verts[k];
    assert_eq!(v.co, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(v.flag, VertFlag::empty());
    assert!(v.edge.is_none());
}
