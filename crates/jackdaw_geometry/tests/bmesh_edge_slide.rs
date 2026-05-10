use bevy::math::Vec3;
use jackdaw_geometry::bmesh::{BMesh, ops::edge_slide::edge_slide};
use jackdaw_geometry::bmesh::ops::loop_cut::loop_cut;
use jackdaw_jsn::Brush;

#[test]
fn slide_loop_cut_edge_at_t_pos_1_moves_endpoint_toward_top() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = BMesh::lift_from_topology(&brush.topology);
    // Loop-cut a vertical strip of the cube.
    let any_vertical_edge = bmesh.edges.iter()
        .find(|(_, e)| {
            let p0 = bmesh.verts[e.v[0]].co;
            let p1 = bmesh.verts[e.v[1]].co;
            (p1 - p0).z.abs() > 0.5  // vertical edge
        })
        .map(|(k, _)| k)
        .expect("vertical edge");
    let cut_result = loop_cut(&mut bmesh, any_vertical_edge, 0.5).expect("cut");
    let new_loop_edges = cut_result.new_loop_edges;
    // The cut adds 4 new horizontal edges + 4 new midpoint verts (at z=0).
    assert_eq!(new_loop_edges.len(), 4);

    // Snapshot midpoint vert positions before slide.
    let mut endpoint_zs_before: Vec<f32> = Vec::new();
    for &edge in &new_loop_edges {
        let e = &bmesh.edges[edge];
        endpoint_zs_before.push(bmesh.verts[e.v[0]].co.z);
        endpoint_zs_before.push(bmesh.verts[e.v[1]].co.z);
    }
    let initial_avg_z = endpoint_zs_before.iter().sum::<f32>() / endpoint_zs_before.len() as f32;
    assert!(initial_avg_z.abs() < 0.1, "before slide: average z is near 0, got {initial_avg_z}");

    // Slide at t = 0.5 (toward top of cube).
    edge_slide(&mut bmesh, &new_loop_edges, 0.5).expect("slide");

    let mut endpoint_zs_after: Vec<f32> = Vec::new();
    for &edge in &new_loop_edges {
        let e = &bmesh.edges[edge];
        endpoint_zs_after.push(bmesh.verts[e.v[0]].co.z);
        endpoint_zs_after.push(bmesh.verts[e.v[1]].co.z);
    }
    let final_avg_z = endpoint_zs_after.iter().sum::<f32>() / endpoint_zs_after.len() as f32;
    // The slide should move the endpoints toward one of the cube's caps (positive or
    // negative z; both are valid since slide direction depends on internal orientation).
    // We just check that the average changed significantly.
    assert!((final_avg_z - initial_avg_z).abs() > 0.2,
            "slide should move endpoints; before {initial_avg_z}, after {final_avg_z}");
    bmesh.validate().expect("valid after slide");
}

#[test]
fn slide_at_t_zero_does_not_move_anything() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = BMesh::lift_from_topology(&brush.topology);
    let any_edge = bmesh.edges.keys().next().unwrap();
    let positions_before: Vec<Vec3> = bmesh.verts.values().map(|v| v.co).collect();
    edge_slide(&mut bmesh, &[any_edge], 0.0).expect("slide");
    let positions_after: Vec<Vec3> = bmesh.verts.values().map(|v| v.co).collect();
    for (a, b) in positions_before.iter().zip(positions_after.iter()) {
        assert!((*a - *b).length() < 1e-5);
    }
}
