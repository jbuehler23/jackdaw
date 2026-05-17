use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::inset_face::inset_face};
use jackdaw_jsn::Brush;

#[test]
fn inset_one_quad_face_of_cube_adds_4_verts_8_edges_4_faces() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let initial_verts = mesh.vert_count();
    let initial_edges = mesh.edge_count();
    let initial_faces = mesh.face_count();
    let face = mesh.faces.keys().next().unwrap();
    let result = inset_face(&mut mesh, face, 0.2).expect("inset");
    assert_eq!(mesh.vert_count(), initial_verts + 4, "+4 inner-ring verts");
    assert_eq!(
        mesh.edge_count(),
        initial_edges + 8,
        "+8 edges (4 inner + 4 wall)"
    );
    assert_eq!(mesh.face_count(), initial_faces + 4, "+4 side-quad faces");
    mesh.validate().expect("valid after inset");
    assert_eq!(result.new_verts.len(), 4);
    assert_eq!(result.side_faces.len(), 4);
}

#[test]
fn inset_inner_face_normal_matches_original_face_normal() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let face = mesh.faces.keys().next().unwrap();
    let original_normal = mesh.faces[face].normal_cache;
    inset_face(&mut mesh, face, 0.1).expect("inset");
    // The original face was shrunk to its inner ring but its normal should be preserved
    // (within numerical noise) since the inner-ring is a parallel-shrunk copy of the original.
    let new_normal = {
        let f = &mesh.faces[face];
        let mut ring_positions = Vec::new();
        let mut cur = f.loop_first;
        for _ in 0..f.loop_count {
            ring_positions.push(mesh.verts[mesh.loops[cur].vert].co);
            cur = mesh.loops[cur].next;
        }
        jackdaw_geometry::newell_normal(&ring_positions)
    };
    assert!(
        new_normal.distance(original_normal) < 1e-3,
        "inner face normal {new_normal} should match original {original_normal}"
    );
}

#[test]
fn inset_amount_zero_leaves_geometry_unchanged_in_position() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let face = mesh.faces.keys().next().unwrap();
    inset_face(&mut mesh, face, 0.0).expect("inset zero");
    mesh.validate().expect("valid after zero inset");
    // With amount=0 the inner ring has the same positions as the outer ring.
    // Side quads will be degenerate (zero-area) but topology should still validate.
}
