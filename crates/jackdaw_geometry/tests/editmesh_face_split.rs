use jackdaw_geometry::editmesh::{EditMesh, ops::face_split::split_face};
use jackdaw_jsn::Brush;

fn first_face_with_count_4(bmesh: &EditMesh) -> jackdaw_geometry::editmesh::FaceKey {
    bmesh.faces.iter().find(|(_, f)| f.loop_count == 4).map(|(k, _)| k).unwrap()
}

fn ring_verts(bmesh: &EditMesh, face: jackdaw_geometry::editmesh::FaceKey) -> Vec<jackdaw_geometry::editmesh::VertKey> {
    let mut out = Vec::new();
    let f = &bmesh.faces[face];
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        out.push(bmesh.loops[cur].vert);
        cur = bmesh.loops[cur].next;
    }
    out
}

#[test]
fn split_quad_face_along_diagonal_makes_two_tris() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let face = first_face_with_count_4(&bmesh);
    let ring = ring_verts(&bmesh, face);
    // Split along the diagonal between ring[0] and ring[2] (opposite verts of quad).
    let initial_faces = bmesh.face_count();
    let initial_edges = bmesh.edge_count();
    let _new_edge = split_face(&mut bmesh, face, ring[0], ring[2]).expect("split");
    assert_eq!(bmesh.face_count(), initial_faces + 1, "+1 face");
    assert_eq!(bmesh.edge_count(), initial_edges + 1, "+1 edge");
    bmesh.validate().expect("post-split valid");
    // Both faces should have loop_count == 3 (tris).
    let mut tri_count = 0;
    for (_, f) in bmesh.faces.iter() {
        if f.loop_count == 3 { tri_count += 1; }
    }
    assert!(tri_count >= 2, "at least 2 tris exist after splitting a quad along its diagonal");
}

#[test]
fn split_with_adjacent_verts_errors() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let face = first_face_with_count_4(&bmesh);
    let ring = ring_verts(&bmesh, face);
    // ring[0] and ring[1] are adjacent — already connected by an edge.
    let result = split_face(&mut bmesh, face, ring[0], ring[1]);
    assert!(result.is_err(), "splitting between adjacent verts should error");
}

#[test]
fn split_with_off_face_vert_errors() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let face = first_face_with_count_4(&bmesh);
    let ring = ring_verts(&bmesh, face);
    // Find a vert that is NOT in this face's ring.
    let off_face_vert = bmesh.verts.keys().find(|k| !ring.contains(k)).unwrap();
    let result = split_face(&mut bmesh, face, ring[0], off_face_vert);
    assert!(result.is_err(), "splitting with off-face vert should error");
}
