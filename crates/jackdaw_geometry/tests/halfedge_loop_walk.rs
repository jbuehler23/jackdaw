use jackdaw_geometry::halfedge::{HalfedgeMesh, select::loop_walk::loop_walk};
use jackdaw_jsn::Brush;

#[test]
fn loop_walk_around_cube_returns_4_edges() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let any_edge = mesh.edges.keys().next().unwrap();
    let result = loop_walk(&mesh, any_edge);
    // A cube edge belongs to a closed parallel-edge ring of 4 edges.
    assert_eq!(
        result.len(),
        4,
        "loop walk on cube should produce 4 edges, got {}",
        result.len()
    );
    assert!(
        result.contains(&any_edge),
        "result should include start edge"
    );
}

#[test]
fn loop_walk_after_loop_cut_new_loop_edge_is_in_6_edge_ring() {
    use jackdaw_geometry::halfedge::ops::loop_cut::loop_cut;
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let any_edge = mesh.edges.keys().next().unwrap();
    let cut = loop_cut(&mut mesh, any_edge, 0.5).expect("cut");
    let new_loop_edge = cut.new_loop_edges[0];
    let result = loop_walk(&mesh, new_loop_edge);
    // After a loop cut on a cube, each new loop edge sits inside a 6-edge ring.
    // The ring interleaves 2 new loop edges with 4 old (split) edges, because
    // the new edges sit at the "seam" of each split face and their parallel-edge
    // ring crosses 2 faces per hop rather than 1.
    assert_eq!(
        result.len(),
        6,
        "loop walk on new loop edge should produce 6 edges, got {}",
        result.len()
    );
    assert!(
        result.contains(&new_loop_edge),
        "result should include start edge"
    );
}
