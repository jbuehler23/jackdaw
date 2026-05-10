use jackdaw_geometry::editmesh::EditMesh;
use jackdaw_jsn::Brush;

#[test]
fn cuboid_round_trip_preserves_counts_and_positions() {
    let brush = Brush::cuboid(1.0, 2.0, 3.0);
    let original = brush.topology.clone();
    let bmesh = EditMesh::lift_from_topology(&original);
    let flattened = bmesh.flatten_to_topology();
    assert_eq!(flattened.vertices.len(), original.vertices.len());
    assert_eq!(flattened.edges.len(),    original.edges.len());
    assert_eq!(flattened.polygons.len(), original.polygons.len());
    assert_eq!(flattened.loops.len(),    original.loops.len());
    // Vertex positions preserved (compare as sorted set; order may differ).
    let mut orig_pos: Vec<_> = original.vertices.iter().map(|v| v.position).collect();
    let mut flat_pos: Vec<_> = flattened.vertices.iter().map(|v| v.position).collect();
    let cmp = |a: &bevy::math::Vec3, b: &bevy::math::Vec3| {
        a.x.partial_cmp(&b.x).unwrap()
         .then(a.y.partial_cmp(&b.y).unwrap())
         .then(a.z.partial_cmp(&b.z).unwrap())
    };
    orig_pos.sort_by(cmp);
    flat_pos.sort_by(cmp);
    for (a, b) in orig_pos.iter().zip(flat_pos.iter()) {
        assert!(a.distance(*b) < 1e-5);
    }
}
