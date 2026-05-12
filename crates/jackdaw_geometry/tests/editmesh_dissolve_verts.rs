use jackdaw_geometry::editmesh::cycles::{disk_walk, radial_walk};
use jackdaw_geometry::editmesh::ops::edge_split::split_edge;
use jackdaw_geometry::editmesh::ops::face_create::create_face_from_verts;
use jackdaw_geometry::editmesh::ops::subdivide::subdivide;
use jackdaw_geometry::editmesh::{EditMesh, ops::dissolve_verts::dissolve_verts};
use jackdaw_jsn::Brush;

#[test]
fn dissolve_midpoint_vert_from_edge_split_restores_original_edge() {
    // Build a two-quad mesh (two quads sharing an edge) and split the shared edge.
    // The split produces a valence-2 midpoint vert; dissolving it restores the original.
    let mut bmesh = EditMesh::default();
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
    create_face_from_verts(&mut bmesh, &[v0, v1, v2, v3]).expect("face A");
    create_face_from_verts(&mut bmesh, &[v1, v0, v4, v5]).expect("face B");
    bmesh.validate().expect("valid before split");

    // Find the shared edge (v0, v1).
    let shared_edge = bmesh
        .edges
        .iter()
        .find(|(_, e)| (e.v[0] == v0 && e.v[1] == v1) || (e.v[0] == v1 && e.v[1] == v0))
        .map(|(k, _)| k)
        .expect("shared edge");

    let initial_verts = bmesh.vert_count(); // 6
    let initial_edges = bmesh.edge_count(); // 7 (4+4 - 1 shared)
    let initial_faces = bmesh.face_count(); // 2

    // Split the shared edge: inserts a valence-2 midpoint vert.
    let mid_vert = split_edge(&mut bmesh, shared_edge, 0.5).expect("split");
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
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
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

#[test]
fn dissolve_corner_produces_outward_facing_merged_face() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let v = bmesh.verts.keys().next().unwrap();
    // Snapshot the corner's expected outward direction = average of 3 incident face normals.
    use jackdaw_geometry::editmesh::FaceKey;
    use std::collections::HashSet;
    let mut incident_faces: HashSet<FaceKey> = HashSet::new();
    for e in disk_walk(&bmesh, v).collect::<Vec<_>>() {
        for lp in radial_walk(&bmesh, e).collect::<Vec<_>>() {
            incident_faces.insert(bmesh.loops[lp].face);
        }
    }
    let mut expected = bevy::math::Vec3::ZERO;
    for f in &incident_faces {
        expected += bmesh.faces[*f].normal_cache;
    }
    let expected = expected.normalize_or_zero();

    // Snapshot all existing face keys before dissolve so we can find the new merged face.
    let faces_before: HashSet<FaceKey> = bmesh.faces.keys().collect();

    dissolve_verts(&mut bmesh, &[v]).expect("dissolve");

    // The merged face is the one that didn't exist before the dissolve.
    let new_face = bmesh
        .faces
        .keys()
        .find(|k| !faces_before.contains(k))
        .expect("a new merged face should have been created");
    let new_normal = bmesh.faces[new_face].normal_cache;
    assert!(
        new_normal.dot(expected) > 0.0,
        "merged face normal {new_normal} should align with expected outward {expected}",
    );
}

#[test]
fn dissolve_subdivide_midpoint_restores_original_cube() {
    // Subdivide a single edge of a cube, then dissolve the inserted midpoint vert.
    // The Blender-style algorithm should dissolve the internal "diagonal" edges
    // first (restoring the two original quads), then splice v out, giving back the
    // original cube topology exactly.
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);

    let initial_verts = bmesh.vert_count(); // 8
    let initial_edges = bmesh.edge_count(); // 12
    let initial_faces = bmesh.face_count(); // 6

    let some_edge = bmesh.edges.keys().next().unwrap();
    let result = subdivide(&mut bmesh, &[some_edge]).expect("subdivide");
    assert_eq!(result.new_verts.len(), 1, "one midpoint inserted");
    let midpoint = result.new_verts[0];
    bmesh.validate().expect("valid after subdivide");

    dissolve_verts(&mut bmesh, &[midpoint]).expect("dissolve");
    bmesh.validate().expect("valid after dissolve");

    assert_eq!(bmesh.vert_count(), initial_verts, "vert count restored");
    assert_eq!(bmesh.edge_count(), initial_edges, "edge count restored");
    assert_eq!(bmesh.face_count(), initial_faces, "face count restored");
}
