use jackdaw_geometry::editmesh::EditMesh;
use jackdaw_jsn::Brush;

#[test]
fn cuboid_lifts_to_8_verts_12_edges_24_loops_6_faces() {
    let brush = Brush::cuboid(0.5, 0.5, 0.5);
    let bmesh = EditMesh::lift_from_topology(&brush.topology);
    assert_eq!(bmesh.vert_count(), 8);
    assert_eq!(bmesh.edge_count(), 12);
    assert_eq!(bmesh.loop_count(), 24);
    assert_eq!(bmesh.face_count(), 6);
}

#[test]
fn cuboid_lift_each_face_has_axis_aligned_normal_cache() {
    let brush = Brush::cuboid(0.5, 0.5, 0.5);
    let bmesh = EditMesh::lift_from_topology(&brush.topology);
    let mut found_pos_z = false;
    let mut found_neg_z = false;
    let mut found_pos_x = false;
    let mut found_neg_x = false;
    let mut found_pos_y = false;
    let mut found_neg_y = false;
    for (_, face) in bmesh.faces.iter() {
        if face.normal_cache.distance(bevy::math::Vec3::Z) < 1e-3 {
            found_pos_z = true;
        }
        if face.normal_cache.distance(bevy::math::Vec3::NEG_Z) < 1e-3 {
            found_neg_z = true;
        }
        if face.normal_cache.distance(bevy::math::Vec3::X) < 1e-3 {
            found_pos_x = true;
        }
        if face.normal_cache.distance(bevy::math::Vec3::NEG_X) < 1e-3 {
            found_neg_x = true;
        }
        if face.normal_cache.distance(bevy::math::Vec3::Y) < 1e-3 {
            found_pos_y = true;
        }
        if face.normal_cache.distance(bevy::math::Vec3::NEG_Y) < 1e-3 {
            found_neg_y = true;
        }
    }
    assert!(found_pos_z && found_neg_z && found_pos_x && found_neg_x && found_pos_y && found_neg_y);
}
