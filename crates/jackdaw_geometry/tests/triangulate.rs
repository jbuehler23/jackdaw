use bevy::math::Vec3;
use jackdaw_geometry::{newell_normal, triangulate::triangulate_polygon};

#[test]
fn unit_quad_produces_two_triangles() {
    let verts = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
    ];
    let ring = [0u32, 1, 2, 3];
    let n = newell_normal(&verts);
    let tris = triangulate_polygon(&verts, &ring, n);
    assert_eq!(tris.len(), 2, "quad -> 2 tris, got {}", tris.len());
    let used: std::collections::HashSet<u32> = tris.iter().flatten().copied().collect();
    assert_eq!(used, [0u32, 1, 2, 3].iter().copied().collect());
}

#[test]
fn concave_l_shape_produces_four_triangles() {
    let verts = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(2.0, 0.0, 0.0),
        Vec3::new(2.0, 1.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(1.0, 2.0, 0.0),
        Vec3::new(0.0, 2.0, 0.0),
    ];
    let ring = [0u32, 1, 2, 3, 4, 5];
    let n = newell_normal(&verts);
    let tris = triangulate_polygon(&verts, &ring, n);
    assert_eq!(tris.len(), 4, "L-shape -> 4 tris, got {}", tris.len());
}
