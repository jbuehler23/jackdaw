use jackdaw_geometry::bmesh::{BMesh, VertFlag, select::{SelectionDelta, apply_vert_delta}};
use jackdaw_jsn::Brush;

#[test]
fn apply_vert_delta_sets_select_bits_and_returns_inverse() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = BMesh::lift_from_topology(&brush.topology);
    let v0 = bmesh.verts.keys().next().unwrap();
    let delta = SelectionDelta { add: vec![v0], remove: vec![] };
    let inverse = apply_vert_delta(&mut bmesh, &delta);
    assert!(bmesh.verts[v0].flag.contains(VertFlag::SELECT));
    apply_vert_delta(&mut bmesh, &inverse);
    assert!(!bmesh.verts[v0].flag.contains(VertFlag::SELECT));
}

#[test]
fn flush_vert_to_edge_promotes_when_both_endpoints_selected() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = BMesh::lift_from_topology(&brush.topology);
    // Select the two endpoints of one edge.
    let any_edge = bmesh.edges.keys().next().unwrap();
    let v0 = bmesh.edges[any_edge].v[0];
    let v1 = bmesh.edges[any_edge].v[1];
    bmesh.verts[v0].flag.insert(VertFlag::SELECT);
    bmesh.verts[v1].flag.insert(VertFlag::SELECT);
    jackdaw_geometry::bmesh::select::flush_vert_to_edge(&mut bmesh);
    assert!(bmesh.edges[any_edge].flag.contains(jackdaw_geometry::bmesh::EdgeFlag::SELECT));
}
