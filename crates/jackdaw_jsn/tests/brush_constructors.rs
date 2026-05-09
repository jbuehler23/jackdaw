use bevy::math::Vec3;
use jackdaw_jsn::Brush;

#[test]
fn cuboid_has_8_verts_12_edges_6_polys_24_loops() {
    let brush = Brush::cuboid(0.5, 0.5, 0.5);
    assert_eq!(brush.faces.len(), 6);
    assert_eq!(brush.topology.vertices.len(), 8);
    assert_eq!(brush.topology.edges.len(), 12);
    assert_eq!(brush.topology.polygons.len(), 6);
    assert_eq!(brush.topology.loops.len(), 24);
    assert_eq!(brush.faces.len(), brush.topology.polygons.len());
}

#[test]
fn prism_triangle_base_has_5_polys_5_faces() {
    let base = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.5, 1.0, 0.0),
    ];
    let brush = Brush::prism(&base, Vec3::Z, 1.0).expect("prism");
    assert_eq!(brush.faces.len(), 5); // 3 sides + top + bottom
    assert_eq!(brush.topology.polygons.len(), 5);
    assert_eq!(brush.topology.vertices.len(), 6); // 3 base + 3 top
}
