use jackdaw_geometry::editmesh::EditMesh;
use jackdaw_jsn::Brush;

#[test]
fn cuboid_validates() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let bmesh = EditMesh::lift_from_topology(&brush.topology);
    bmesh.validate().expect("cuboid should be valid");
}

#[test]
fn empty_bmesh_validates() {
    let bmesh = EditMesh::default();
    bmesh.validate().expect("empty mesh is valid");
}

#[test]
fn corrupted_disk_cycle_fails_validation() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let v_key = bmesh.verts.keys().next().unwrap();
    bmesh.verts[v_key].edge = None;
    let result = bmesh.validate();
    assert!(result.is_err());
}
