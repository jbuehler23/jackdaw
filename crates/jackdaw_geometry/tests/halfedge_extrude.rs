use bevy_math::Vec3;
use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::extrude_face_region::extrude_face_region};
use jackdaw_jsn::Brush;

#[test]
fn extrude_one_quad_face_of_cube_adds_4_verts_8_edges_4_faces() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let initial_verts = mesh.vert_count();
    let initial_edges = mesh.edge_count();
    let initial_faces = mesh.face_count();
    let face = mesh.faces.keys().next().unwrap();
    let result = extrude_face_region(&mut mesh, face, 0.5).expect("extrude");
    assert_eq!(mesh.vert_count(), initial_verts + 4);
    assert_eq!(mesh.edge_count(), initial_edges + 8);
    assert_eq!(mesh.face_count(), initial_faces + 4);
    mesh.validate().expect("valid after extrude");
    assert_eq!(result.new_verts.len(), 4);
    assert_eq!(result.side_faces.len(), 4);
}

#[test]
fn extrude_top_face_by_1_unit_makes_cube_taller_by_1() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    // Find the +Z face.
    let top_face = mesh
        .faces
        .iter()
        .find(|(_, f)| f.normal_cache.distance(Vec3::Z) < 1e-3)
        .map(|(k, _)| k)
        .expect("top face exists");
    let result = extrude_face_region(&mut mesh, top_face, 1.0).expect("extrude");
    // The 4 new verts should be at z = 2 (was at 1, extruded by 1).
    for vk in &result.new_verts {
        let pos = mesh.verts[*vk].co;
        assert!(
            (pos.z - 2.0).abs() < 1e-4,
            "extruded vert z should be 2.0, got {pos}"
        );
    }
}
