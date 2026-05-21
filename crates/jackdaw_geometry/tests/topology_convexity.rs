use bevy_math::Vec3;
use jackdaw_geometry::is_convex_topology;
use jackdaw_jsn::Brush;

#[test]
fn cube_is_convex() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    assert!(is_convex_topology(&brush.topology));
}

#[test]
fn concave_brush_with_inward_vert_not_convex() {
    let mut brush = Brush::cuboid(1.0, 1.0, 1.0);
    // Move one corner inward through the cube interior.
    if let Some(v) = brush.topology.vertices.get_mut(0) {
        v.position = Vec3::new(0.0, 0.0, 0.0); // pull corner to origin (inside)
    }
    assert!(!is_convex_topology(&brush.topology));
}

#[test]
fn empty_topology_is_treated_as_convex() {
    let topology = jackdaw_geometry::BrushTopology::default();
    assert!(is_convex_topology(&topology));
}
