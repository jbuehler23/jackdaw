use bevy::math::Vec3;
use jackdaw_geometry::bmesh::{BMesh, ops::vertex_slide::vertex_slide};
use jackdaw_jsn::Brush;

#[test]
fn vertex_slide_at_t_0_5_moves_vert_halfway_to_neighbor() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = BMesh::lift_from_topology(&brush.topology);
    let v = bmesh.verts.keys().next().unwrap();
    let v_start = bmesh.verts[v].co;
    // Find the first edge in this vert's disk cycle.
    let first_edge = bmesh.verts[v].edge.expect("vert has incident edge");
    let other_vert = {
        let e = &bmesh.edges[first_edge];
        if e.v[0] == v { e.v[1] } else { e.v[0] }
    };
    let target_pos = bmesh.verts[other_vert].co;
    let expected = v_start.lerp(target_pos, 0.5);

    vertex_slide(&mut bmesh, &[v], 0.5).expect("slide");
    let v_new = bmesh.verts[v].co;
    assert!((v_new - expected).length() < 1e-4,
            "vert should be at midpoint between start and neighbor: expected {expected}, got {v_new}");
    bmesh.validate().expect("valid after slide");
}

#[test]
fn vertex_slide_at_t_zero_does_nothing() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = BMesh::lift_from_topology(&brush.topology);
    let v = bmesh.verts.keys().next().unwrap();
    let v_start = bmesh.verts[v].co;
    vertex_slide(&mut bmesh, &[v], 0.0).expect("slide");
    let v_after = bmesh.verts[v].co;
    assert!((v_after - v_start).length() < 1e-6);
}

#[test]
fn vertex_slide_isolated_vert_with_no_edge_no_op() {
    let mut bmesh = BMesh::default();
    let v = bmesh.add_vert(Vec3::ZERO);
    // No edge on this vert.
    let result = vertex_slide(&mut bmesh, &[v], 0.5);
    // Either error or empty result is acceptable; just don't panic.
    let _ = result;
}
