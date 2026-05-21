use bevy_math::Vec3;
use jackdaw_geometry::newell::newell_normal;

#[test]
fn unit_square_in_xy_plane_normal_is_z() {
    let ring = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    ];
    let n = newell_normal(&ring);
    assert!(n.distance(Vec3::Z) < 1e-5, "expected +Z, got {n}");
}

#[test]
fn reversed_winding_flips_normal() {
    let ring = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
    ];
    let n = newell_normal(&ring);
    assert!(n.distance(Vec3::NEG_Z) < 1e-5);
}

#[test]
fn slight_non_planar_pentagon_returns_close_to_average_normal() {
    let ring = [
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.3, 1.0, 0.0),
        Vec3::new(-0.8, 0.6, 0.001),
        Vec3::new(-0.8, -0.6, 0.0),
        Vec3::new(0.3, -1.0, 0.0),
    ];
    let n = newell_normal(&ring);
    assert!(n.dot(Vec3::Z) > 0.99);
}
