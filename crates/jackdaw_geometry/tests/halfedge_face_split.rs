use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::face_split::split_face};
use jackdaw_jsn::Brush;

fn first_face_with_count_4(mesh: &HalfedgeMesh) -> jackdaw_geometry::halfedge::FaceKey {
    mesh
        .faces
        .iter()
        .find(|(_, f)| f.loop_count == 4)
        .map(|(k, _)| k)
        .unwrap()
}

fn ring_verts(
    mesh: &HalfedgeMesh,
    face: jackdaw_geometry::halfedge::FaceKey,
) -> Vec<jackdaw_geometry::halfedge::VertKey> {
    let mut out = Vec::new();
    let f = &mesh.faces[face];
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        out.push(mesh.loops[cur].vert);
        cur = mesh.loops[cur].next;
    }
    out
}

#[test]
fn split_quad_face_along_diagonal_makes_two_tris() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let face = first_face_with_count_4(&mesh);
    let ring = ring_verts(&mesh, face);
    // Split along the diagonal between ring[0] and ring[2] (opposite verts of quad).
    let initial_faces = mesh.face_count();
    let initial_edges = mesh.edge_count();
    let _new_edge = split_face(&mut mesh, face, ring[0], ring[2]).expect("split");
    assert_eq!(mesh.face_count(), initial_faces + 1, "+1 face");
    assert_eq!(mesh.edge_count(), initial_edges + 1, "+1 edge");
    mesh.validate().expect("post-split valid");
    // Both faces should have loop_count == 3 (tris).
    let mut tri_count = 0;
    for (_, f) in mesh.faces.iter() {
        if f.loop_count == 3 {
            tri_count += 1;
        }
    }
    assert!(
        tri_count >= 2,
        "at least 2 tris exist after splitting a quad along its diagonal"
    );
}

#[test]
fn split_with_adjacent_verts_errors() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let face = first_face_with_count_4(&mesh);
    let ring = ring_verts(&mesh, face);
    // ring[0] and ring[1] are adjacent - already connected by an edge.
    let result = split_face(&mut mesh, face, ring[0], ring[1]);
    assert!(
        result.is_err(),
        "splitting between adjacent verts should error"
    );
}

#[test]
fn split_with_off_face_vert_errors() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let face = first_face_with_count_4(&mesh);
    let ring = ring_verts(&mesh, face);
    // Find a vert that is NOT in this face's ring.
    let off_face_vert = mesh.verts.keys().find(|k| !ring.contains(k)).unwrap();
    let result = split_face(&mut mesh, face, ring[0], off_face_vert);
    assert!(result.is_err(), "splitting with off-face vert should error");
}
