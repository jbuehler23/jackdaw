use jackdaw_geometry::halfedge::{HalfedgeMesh, select::ring_walk::ring_walk};
use jackdaw_jsn::Brush;

#[test]
fn ring_walk_around_cube_returns_4_edges() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let any_edge = mesh.edges.keys().next().unwrap();
    let result = ring_walk(&mesh, any_edge);
    // Cube edge: ring of 4 perpendicular edges (the verticals around a cube).
    assert_eq!(
        result.len(),
        4,
        "ring walk on cube should produce 4 edges, got {}",
        result.len()
    );
    assert!(
        result.contains(&any_edge),
        "result should include start edge"
    );
}

#[test]
fn ring_walk_returns_different_edges_than_loop_walk() {
    use jackdaw_geometry::halfedge::select::loop_walk::loop_walk;
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let any_edge = mesh.edges.keys().next().unwrap();
    let loop_result: std::collections::HashSet<_> =
        loop_walk(&mesh, any_edge).into_iter().collect();
    let ring_result: std::collections::HashSet<_> =
        ring_walk(&mesh, any_edge).into_iter().collect();
    // Both should include the start edge.
    assert!(loop_result.contains(&any_edge));
    assert!(ring_result.contains(&any_edge));
    // The two sets should differ (loop walks parallel; ring walks perpendicular).
    let intersection: std::collections::HashSet<_> =
        loop_result.intersection(&ring_result).copied().collect();
    assert_eq!(
        intersection.len(),
        1,
        "loop and ring should only share the start edge"
    );
}
