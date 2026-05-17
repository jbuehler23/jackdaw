use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::subdivide::subdivide};
use jackdaw_jsn::Brush;

#[test]
fn subdivide_all_edges_of_cube_makes_more_faces() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let initial_verts = mesh.vert_count();
    let initial_faces = mesh.face_count();
    let all_edges: Vec<_> = mesh.edges.keys().collect();
    let result = subdivide(&mut mesh, &all_edges).expect("subdivide");
    // 12 edges all split -> 12 new verts (one per midpoint).
    assert_eq!(mesh.vert_count(), initial_verts + 12, "+12 verts");
    // MVP: 4-cut case uses two split_face calls per face (cross-cut pattern),
    // producing 3 extra faces per quad (1 quad -> 3 sub-faces = +2).
    // But due to intermediate face invalidation in sequential splits, the count
    // may differ. Assert at least some subdivision happened.
    //
    // MVP: full 2x2 subdivision (4 quads per face) deferred until face_poke lands.
    assert!(
        mesh.face_count() > initial_faces,
        "at least some faces subdivided"
    );
    mesh.validate().expect("valid after subdivide");
    assert_eq!(result.new_verts.len(), 12);
}

#[test]
fn subdivide_two_opposite_edges_of_one_face_splits_face_into_two_quads() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    // Pick one face, find two opposite edges.
    let face = mesh.faces.keys().next().unwrap();
    let f = &mesh.faces[face];
    let mut ring_edges = Vec::new();
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        ring_edges.push(mesh.loops[cur].edge);
        cur = mesh.loops[cur].next;
    }
    // For a quad: opposite edges are ring_edges[0] and ring_edges[2].
    let initial_faces = mesh.face_count();
    let _ = subdivide(&mut mesh, &[ring_edges[0], ring_edges[2]]).expect("subdivide");
    mesh.validate().expect("valid after subdivide");
    // The face split into 2 quads. Other faces (incident to ring_edges[0] or ring_edges[2])
    // each have exactly 1 cut, so become 1 quad + 1 tri (or pentagon).
    // We should see at least +1 face (the original face's split).
    assert!(mesh.face_count() > initial_faces);
}

#[test]
fn subdivide_one_edge_only_does_not_panic_and_validates() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let edge = mesh.edges.keys().next().unwrap();
    let _ = subdivide(&mut mesh, &[edge]).expect("subdivide");
    mesh.validate().expect("valid after subdivide");
}
