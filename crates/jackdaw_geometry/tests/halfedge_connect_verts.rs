use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::connect_verts::connect_verts};
use jackdaw_jsn::Brush;

#[test]
fn connect_two_opposite_verts_of_quad_face_splits_into_two_tris() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    // Pick one face, find 2 opposite verts.
    let face = mesh.faces.keys().next().unwrap();
    let f = &mesh.faces[face];
    let mut ring_verts = Vec::new();
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        ring_verts.push(mesh.loops[cur].vert);
        cur = mesh.loops[cur].next;
    }
    let v0 = ring_verts[0];
    let v2 = ring_verts[2]; // opposite in a quad
    let initial_faces = mesh.face_count();
    let initial_edges = mesh.edge_count();
    let result = connect_verts(&mut mesh, &[v0, v2]).expect("connect");
    assert_eq!(mesh.face_count(), initial_faces + 1, "+1 face");
    assert_eq!(mesh.edge_count(), initial_edges + 1, "+1 edge");
    mesh.validate().expect("valid");
    assert_eq!(result.new_edges.len(), 1);
}

#[test]
fn connect_verts_in_different_faces_splits_each_face() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    // Pick 2 faces, get 2 opposite verts in each.
    let faces: Vec<_> = mesh.faces.keys().collect();
    let face1 = faces[0];
    let face2 = faces[1];
    let ring1: Vec<_> = {
        let f = &mesh.faces[face1];
        let mut cur = f.loop_first;
        (0..f.loop_count)
            .map(|_| {
                let v = mesh.loops[cur].vert;
                cur = mesh.loops[cur].next;
                v
            })
            .collect()
    };
    let ring2: Vec<_> = {
        let f = &mesh.faces[face2];
        let mut cur = f.loop_first;
        (0..f.loop_count)
            .map(|_| {
                let v = mesh.loops[cur].vert;
                cur = mesh.loops[cur].next;
                v
            })
            .collect()
    };
    let initial_faces = mesh.face_count();
    // Pass diagonal verts of two different faces (4 total).
    let result =
        connect_verts(&mut mesh, &[ring1[0], ring1[2], ring2[0], ring2[2]]).expect("connect");
    mesh.validate().expect("valid");
    // Each face should split, so face count goes up by at least 1 (more if both faces had
    // diagonal verts that don't conflict).
    assert!(mesh.face_count() > initial_faces);
    assert!(!result.new_edges.is_empty());
}

#[test]
fn connect_one_vert_returns_empty_or_error() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let v = mesh.verts.keys().next().unwrap();
    let result = connect_verts(&mut mesh, &[v]);
    // Either error or empty result is acceptable.
    if let Ok(r) = result {
        assert_eq!(r.new_edges.len(), 0);
    }
}
