use glam::Vec3;
use jackdaw_csg::{BooleanOp, CsgInput, brush_boolean};
use jackdaw_geometry::{compute_brush_topology, cuboid_faces};

#[test]
fn box_minus_tunnel_through_manifold() {
    let outer_faces = cuboid_faces(Vec3::splat(4.0));
    // A 2x2 bar longer than the box on Z punches a tunnel (a concave result).
    let cutter_faces = cuboid_faces(Vec3::new(2.0, 2.0, 8.0));
    let outer_topo = compute_brush_topology(&outer_faces);
    let cutter_topo = compute_brush_topology(&cutter_faces);
    let a = CsgInput::new(&outer_faces, &outer_topo);
    let b = CsgInput::new(&cutter_faces, &cutter_topo);

    let result = brush_boolean(&a, &b, BooleanOp::Difference)
        .expect("manifold difference of two valid boxes should succeed");
    assert!(!result.faces.is_empty(), "difference should produce faces");
    assert!(
        !result.topology.polygons.is_empty(),
        "result should carry topology"
    );
}
