use jackdaw_geometry::halfedge::{
    HalfedgeMesh, VertFlag,
    select::{SelectionDelta, apply_vert_delta},
};
use jackdaw_jsn::Brush;

#[test]
fn apply_vert_delta_sets_select_bits_and_returns_inverse() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let v0 = mesh.verts.keys().next().unwrap();
    let delta = SelectionDelta {
        add: vec![v0],
        remove: vec![],
    };
    let inverse = apply_vert_delta(&mut mesh, &delta);
    assert!(mesh.verts[v0].flag.contains(VertFlag::SELECT));
    apply_vert_delta(&mut mesh, &inverse);
    assert!(!mesh.verts[v0].flag.contains(VertFlag::SELECT));
}

#[test]
fn flush_vert_to_edge_promotes_when_both_endpoints_selected() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    // Select the two endpoints of one edge.
    let any_edge = mesh.edges.keys().next().unwrap();
    let v0 = mesh.edges[any_edge].v[0];
    let v1 = mesh.edges[any_edge].v[1];
    mesh.verts[v0].flag.insert(VertFlag::SELECT);
    mesh.verts[v1].flag.insert(VertFlag::SELECT);
    jackdaw_geometry::halfedge::select::flush_vert_to_edge(&mut mesh);
    assert!(
        mesh.edges[any_edge]
            .flag
            .contains(jackdaw_geometry::halfedge::EdgeFlag::SELECT)
    );
}
