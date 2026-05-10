use jackdaw_geometry::bmesh::{BMesh, ops::dissolve_verts::dissolve_verts};
use jackdaw_geometry::bmesh::ops::edge_split::bm_edge_split;
use jackdaw_geometry::bmesh::ops::face_create::bm_face_create_from_verts;
use jackdaw_jsn::Brush;

#[test]
fn dissolve_midpoint_vert_from_edge_split_restores_original_edge() {
    // Build a two-quad mesh (two quads sharing an edge) and split the shared edge.
    // The split produces a valence-2 midpoint vert; dissolving it restores the original.
    let mut bmesh = BMesh::default();
    use bevy::math::Vec3;
    // Quad A: (0,0,0)-(1,0,0)-(1,1,0)-(0,1,0)
    let v0 = bmesh.add_vert(Vec3::new(0.0, 0.0, 0.0));
    let v1 = bmesh.add_vert(Vec3::new(1.0, 0.0, 0.0));
    let v2 = bmesh.add_vert(Vec3::new(1.0, 1.0, 0.0));
    let v3 = bmesh.add_vert(Vec3::new(0.0, 1.0, 0.0));
    // Quad B: (0,0,0)-(0,-1,0)-(-1,-1,0)-(0,0,0) — shares edge (v0,v1) with quad A
    // Actually make it share the v0-v1 edge: B = (v1,v0, v4, v5)
    let v4 = bmesh.add_vert(Vec3::new(0.0, -1.0, 0.0));
    let v5 = bmesh.add_vert(Vec3::new(1.0, -1.0, 0.0));
    bm_face_create_from_verts(&mut bmesh, &[v0, v1, v2, v3]).expect("face A");
    bm_face_create_from_verts(&mut bmesh, &[v1, v0, v4, v5]).expect("face B");
    bmesh.validate().expect("valid before split");

    // Find the shared edge (v0, v1).
    let shared_edge = bmesh.edges.iter()
        .find(|(_, e)| (e.v[0] == v0 && e.v[1] == v1) || (e.v[0] == v1 && e.v[1] == v0))
        .map(|(k, _)| k)
        .expect("shared edge");

    let initial_verts = bmesh.vert_count(); // 6
    let initial_edges = bmesh.edge_count(); // 7 (4+4 - 1 shared)
    let initial_faces = bmesh.face_count(); // 2

    // Split the shared edge: inserts a valence-2 midpoint vert.
    let mid_vert = bm_edge_split(&mut bmesh, shared_edge, 0.5).expect("split");
    assert_eq!(bmesh.vert_count(), initial_verts + 1);
    bmesh.validate().expect("valid after split");

    // Now dissolve the midpoint vert.
    let result = dissolve_verts(&mut bmesh, &[mid_vert]).expect("dissolve");
    // Expect: -1 vert (the midpoint is removed).
    // Two edges (v0,mid) and (mid,v1) become one new edge (v0,v1): net -1 edge.
    // The two faces each lose one loop entry but are NOT merged (they still exist as quads).
    assert_eq!(bmesh.vert_count(), initial_verts, "vert count restored");
    assert_eq!(bmesh.edge_count(), initial_edges, "edge count restored");
    assert_eq!(bmesh.face_count(), initial_faces, "face count unchanged");
    bmesh.validate().expect("valid after dissolve");
    assert_eq!(result.removed_verts, 1);
}

#[test]
fn dissolve_valence_3_corner_of_cube_removes_corner_and_merges_3_faces() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = BMesh::lift_from_topology(&brush.topology);
    let initial_faces = bmesh.face_count();
    let initial_verts = bmesh.vert_count();
    let v = bmesh.verts.keys().next().unwrap(); // any cube corner is valence-3
    let result = dissolve_verts(&mut bmesh, &[v]).expect("dissolve");
    assert_eq!(result.removed_verts, 1);
    // Cube corner: 3 faces merged into 1, so face count = original - 2.
    assert_eq!(bmesh.face_count(), initial_faces - 2);
    assert_eq!(bmesh.vert_count(), initial_verts - 1);
    bmesh.validate().expect("valid after dissolve");
}
