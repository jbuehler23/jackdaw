use bevy::math::Vec3;
use jackdaw_geometry::compute_brush_topology;
use jackdaw_jsn::{Brush, BrushTopology};

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

#[test]
fn sphere_constructor_populates_topology() {
    // Icosahedral sphere: 20 triangular faces, 12 vertices, 30 edges, 60 loops.
    let brush = Brush::sphere(1.0);
    assert_eq!(brush.faces.len(), 20);
    assert!(
        !brush.topology.polygons.is_empty(),
        "sphere topology must be populated"
    );
    assert_eq!(brush.topology.polygons.len(), 20);
    assert_eq!(brush.topology.vertices.len(), 12);
    assert_eq!(brush.topology.edges.len(), 30);
    assert_eq!(brush.topology.loops.len(), 60);
}

#[test]
fn every_primitive_constructor_populates_topology() {
    // Invariant: every primitive constructor returns a Brush with both
    // `faces` and `topology` populated, so downstream code never has
    // to branch on whether topology exists.
    let cuboid = Brush::cuboid(0.5, 0.5, 0.5);
    assert!(!cuboid.topology.polygons.is_empty(), "cuboid");

    let prism = Brush::prism(
        &[
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.5, 1.0, 0.0),
        ],
        Vec3::Z,
        1.0,
    )
    .expect("prism");
    assert!(!prism.topology.polygons.is_empty(), "prism");

    let sphere = Brush::sphere(1.0);
    assert!(!sphere.topology.polygons.is_empty(), "sphere");
}

#[test]
fn compute_brush_topology_populates_legacy_planes_only_brush() {
    // Simulate a deserialized legacy brush: faces only, empty topology.
    // The standalone helper should recover a valid topology.
    let template = Brush::cuboid(0.5, 0.5, 0.5);
    let legacy = Brush {
        faces: template.faces.clone(),
        topology: BrushTopology::default(),
    };
    assert!(
        legacy.topology.polygons.is_empty(),
        "starting point must have empty topology"
    );

    let recovered = compute_brush_topology(&legacy.faces);
    assert_eq!(
        recovered.polygons.len(),
        legacy.faces.len(),
        "topology poly count matches face count"
    );
    assert!(!recovered.vertices.is_empty());
    assert!(!recovered.edges.is_empty());
    assert!(!recovered.loops.is_empty());
}
