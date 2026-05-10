use jackdaw_geometry::editmesh::{EditMesh, EdgeFlag, select::linked_walk::linked_walk};
use jackdaw_jsn::Brush;

#[test]
fn linked_walk_on_connected_cube_returns_all_6_faces() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let bmesh = EditMesh::lift_from_topology(&brush.topology);
    let any_face = bmesh.faces.keys().next().unwrap();
    let result = linked_walk(&bmesh, any_face, false);
    assert_eq!(result.len(), 6, "cube has 6 connected faces");
}

#[test]
fn linked_walk_with_sharp_blockers_isolates_face() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    // Pick one face. Mark all its boundary edges as SHARP.
    let isolated_face = bmesh.faces.keys().next().unwrap();
    let face_data = bmesh.faces[isolated_face].clone();
    let mut cur = face_data.loop_first;
    let mut boundary_edges = Vec::new();
    for _ in 0..face_data.loop_count {
        boundary_edges.push(bmesh.loops[cur].edge);
        cur = bmesh.loops[cur].next;
    }
    for &e in &boundary_edges {
        bmesh.edges[e].flag.insert(EdgeFlag::SHARP);
    }
    // Walk from isolated_face with blockers ON: should only return isolated_face.
    let result = linked_walk(&bmesh, isolated_face, true);
    assert_eq!(result.len(), 1, "isolated face surrounded by SHARP edges should be alone, got {} faces", result.len());
    // Walk from isolated_face with blockers OFF: should return all 6.
    let result_no_blockers = linked_walk(&bmesh, isolated_face, false);
    assert_eq!(result_no_blockers.len(), 6);
}
