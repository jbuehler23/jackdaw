use jackdaw_geometry::halfedge::HalfedgeMesh;
use jackdaw_jsn::Brush;

#[test]
fn cuboid_validates() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    mesh.validate().expect("cuboid should be valid");
}

#[test]
fn empty_halfedge_validates() {
    let mesh = HalfedgeMesh::default();
    mesh.validate().expect("empty mesh is valid");
}

#[test]
fn corrupted_disk_cycle_fails_validation() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let v_key = mesh.verts.keys().next().unwrap();
    mesh.verts[v_key].edge = None;
    let result = mesh.validate();
    assert!(result.is_err());
}
