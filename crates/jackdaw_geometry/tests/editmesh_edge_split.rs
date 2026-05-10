use jackdaw_geometry::editmesh::{EditMesh, ops::edge_split::split_edge};
use jackdaw_jsn::Brush;

#[test]
fn split_edge_of_cube_adds_one_vert_one_edge_one_loop_per_adjacent_face() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let initial_verts = bmesh.vert_count();
    let initial_edges = bmesh.edge_count();
    let initial_loops = bmesh.loop_count();
    let initial_faces = bmesh.face_count();
    let edge_to_split = bmesh.edges.keys().next().unwrap();
    // Each cube edge is shared by exactly 2 faces.
    let new_vert = split_edge(&mut bmesh, edge_to_split, 0.5).expect("split");
    assert_eq!(bmesh.vert_count(), initial_verts + 1, "+1 vert");
    assert_eq!(bmesh.edge_count(), initial_edges + 1, "+1 edge");
    assert_eq!(bmesh.face_count(), initial_faces, "no new faces");
    // 2 incident faces, each gains one loop.
    assert_eq!(bmesh.loop_count(), initial_loops + 2, "+2 loops (one per adjacent face)");
    bmesh.validate().expect("valid after split");
    // The new vert is at midpoint.
    let new_pos = bmesh.verts[new_vert].co;
    assert!(new_pos.length() > 0.0 && new_pos.length() < 2.0,
            "new vert at midpoint should be inside cube envelope, got {new_pos}");
}

#[test]
fn split_edge_at_t_quarter_places_vert_at_quarter_position() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let edge_to_split = bmesh.edges.keys().next().unwrap();
    let v0 = bmesh.edges[edge_to_split].v[0];
    let v1 = bmesh.edges[edge_to_split].v[1];
    let p0 = bmesh.verts[v0].co;
    let p1 = bmesh.verts[v1].co;
    let new_vert = split_edge(&mut bmesh, edge_to_split, 0.25).expect("split");
    let expected = p0.lerp(p1, 0.25);
    assert!((bmesh.verts[new_vert].co - expected).length() < 1e-5);
}

#[test]
fn split_edge_then_validate_round_trip_preserves_topology_via_flatten() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let edge_to_split = bmesh.edges.keys().next().unwrap();
    split_edge(&mut bmesh, edge_to_split, 0.5).unwrap();
    bmesh.validate().expect("post-split valid");
    let topology = bmesh.flatten_to_topology();
    assert_eq!(topology.vertices.len(), 9);
    assert_eq!(topology.edges.len(),    13);
    assert_eq!(topology.polygons.len(), 6);
    assert_eq!(topology.loops.len(),    26);
}
