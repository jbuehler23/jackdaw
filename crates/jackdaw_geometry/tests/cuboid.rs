use glam::Vec3;
use jackdaw_geometry::{compute_brush_topology, cuboid_faces};

#[test]
fn cuboid_builds_a_closed_box() {
    let faces = cuboid_faces(Vec3::new(2.0, 2.0, 2.0));
    assert_eq!(faces.len(), 6, "a box has six faces");

    let topo = compute_brush_topology(&faces);
    assert_eq!(topo.vertices.len(), 8, "a box has eight corners");
    assert_eq!(topo.polygons.len(), 6, "one polygon per face");
    assert_eq!(topo.edges.len(), 12, "a box has twelve edges");
}
