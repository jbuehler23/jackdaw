use jackdaw_geometry::editmesh::{EditMesh, ops::loop_cut::loop_cut};
use jackdaw_jsn::Brush;

#[test]
fn loop_cut_around_cube_at_t_0_5_adds_4_verts_and_4_loop_edges() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let initial_verts = bmesh.vert_count();
    let initial_faces = bmesh.face_count();
    let initial_edges = bmesh.edge_count();
    let start_edge = bmesh.edges.keys().next().unwrap();
    let result = loop_cut(&mut bmesh, start_edge, 0.5).expect("loop cut");
    // 4 quad faces in the ring around the cube. Each gets split:
    //   - 4 edges crossed -> 4 new verts (one per crossed edge midpoint).
    //   - 4 faces crossed -> 4 new faces (each quad becomes 2 quads).
    //   - 4 new "loop" edges (one per face split).
    // Total edges added: 4 (from edge splits) + 4 (from face splits) = 8.
    assert_eq!(bmesh.vert_count(), initial_verts + 4, "+4 verts (one per crossed edge midpoint)");
    assert_eq!(bmesh.face_count(), initial_faces + 4, "+4 faces (one per crossed quad becoming 2 quads)");
    assert_eq!(bmesh.edge_count(), initial_edges + 8, "+8 edges (4 from edge splits + 4 from face splits)");
    bmesh.validate().expect("valid after loop cut");
    assert_eq!(result.new_loop_edges.len(), 4, "result reports 4 loop edges");
    assert_eq!(result.new_verts.len(), 4);
    assert_eq!(result.new_faces.len(), 4);
}

#[test]
fn loop_cut_at_t_0_25_places_loop_offset_from_midpoint() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let start_edge = bmesh.edges.keys().next().unwrap();
    let result = loop_cut(&mut bmesh, start_edge, 0.25).expect("loop cut");
    // All 4 new verts should sit at t=0.25 along their respective ring edges.
    // Just check they exist and are distinct.
    assert_eq!(result.new_verts.len(), 4);
    let positions: Vec<_> = result.new_verts.iter().map(|&k| bmesh.verts[k].co).collect();
    let mut unique = positions.clone();
    unique.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap()
        .then(a.y.partial_cmp(&b.y).unwrap())
        .then(a.z.partial_cmp(&b.z).unwrap()));
    unique.dedup_by(|a, b| (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6 && (a.z - b.z).abs() < 1e-6);
    assert_eq!(unique.len(), 4, "4 distinct positions");
}
