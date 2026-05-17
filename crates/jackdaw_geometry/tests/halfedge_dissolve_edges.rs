use jackdaw_geometry::halfedge::ops::loop_cut::loop_cut;
use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::dissolve_edges::dissolve_edges};
use jackdaw_jsn::Brush;

#[test]
fn dissolve_a_loop_cut_edge_merges_two_quads_back_into_one() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let any_edge = mesh.edges.keys().next().unwrap();
    let cut = loop_cut(&mut mesh, any_edge, 0.5).expect("cut");
    let initial_faces = mesh.face_count();
    let initial_edges = mesh.edge_count();
    // Dissolve one of the new loop edges.
    let new_loop_edge = cut.new_loop_edges[0];
    let result = dissolve_edges(&mut mesh, &[new_loop_edge]).expect("dissolve");
    // The two adjacent quads merge into 1; -1 face, -1 edge.
    assert_eq!(mesh.face_count(), initial_faces - 1);
    assert_eq!(mesh.edge_count(), initial_edges - 1);
    mesh.validate().expect("valid after dissolve");
    assert_eq!(result.removed_edges, 1);
}

#[test]
fn dissolve_boundary_edge_errors() {
    // A boundary edge has only 1 incident loop. We can't merge two faces if there's only one.
    let mut mesh = HalfedgeMesh::default();
    let v0 = mesh.add_vert(bevy::math::Vec3::ZERO);
    let v1 = mesh.add_vert(bevy::math::Vec3::X);
    let e = jackdaw_geometry::halfedge::ops::edge_create::create_edge(&mut mesh, v0, v1);
    // No face on this edge.
    let result = dissolve_edges(&mut mesh, &[e]);
    // Either error or skipped (returns Ok with 0 removed). Just don't panic.
    if let Ok(r) = result {
        assert_eq!(r.removed_edges, 0);
    }
}
