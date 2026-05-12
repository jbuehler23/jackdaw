use bevy::math::Vec3;
use jackdaw_geometry::editmesh::{
    EditMesh,
    ops::edge_bevel::{BevelError, edge_bevel},
};
use jackdaw_jsn::Brush;

#[test]
fn bevel_cube_edge_creates_chamfer_quad() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let initial_verts = bmesh.vert_count();
    let initial_edges = bmesh.edge_count();
    let initial_faces = bmesh.face_count();

    let edge = bmesh
        .edges
        .keys()
        .next()
        .expect("cube has at least one edge");
    let result = edge_bevel(&mut bmesh, &[edge], 0.1).expect("bevel one cube edge");

    // 1 chamfer face was added.
    assert_eq!(
        result.new_faces.len(),
        1,
        "one chamfer quad per beveled edge"
    );
    // 2 rail edges per beveled edge.
    assert_eq!(result.new_edges.len(), 2, "two rail edges per beveled edge");

    // 4 new offset verts (2 per endpoint) - 2 old verts (v0, v1 die for the
    // standard cube case where every vert has degree 3).
    assert_eq!(
        bmesh.vert_count(),
        initial_verts + 4 - 2,
        "+4 offset verts, -2 original endpoint verts"
    );

    // Face count: +1 chamfer.
    assert_eq!(
        bmesh.face_count(),
        initial_faces + 1,
        "face count grows by one chamfer per beveled edge"
    );

    // Edge count delta: -1 original (removed) + 4 rails-and-end-caps (the
    // chamfer's 4 edges) + 0 changes to the parallel edges (those are torn
    // down and re-created with new endpoints, net zero count). The 4 chamfer
    // edges include the 2 rails plus the 2 "end-cap" edges (v0_A,v0_B) and
    // (v1_A,v1_B). For a cube vertex with degree 3, each end-cap edge is
    // shared with the rebuilt third face, so it gets created exactly once.
    let _ = initial_edges; // suppress unused if assertion changes
    bmesh
        .validate()
        .expect("EditMesh invariants hold after bevel");
}

#[test]
fn bevel_zero_width_returns_error() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let edge = bmesh.edges.keys().next().unwrap();
    let err = edge_bevel(&mut bmesh, &[edge], 0.0).expect_err("zero width is rejected");
    assert!(matches!(err, BevelError::WidthTooSmall));
}

#[test]
fn bevel_empty_returns_error() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let err = edge_bevel(&mut bmesh, &[], 0.1).expect_err("empty input is rejected");
    assert!(matches!(err, BevelError::EmptyInput));
}

#[test]
fn bevel_cube_chamfer_is_a_parallelogram() {
    // For an axis-aligned unit cube, the chamfer face produced by beveling a
    // single edge must be a clean parallelogram: opposite edges equal as
    // vectors, and the four chamfer verts coplanar.
    let brush = Brush::cuboid(2.0, 2.0, 2.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let edge = bmesh.edges.keys().next().expect("cube has edges");
    let width = 0.3_f32;
    let result = edge_bevel(&mut bmesh, &[edge], width).expect("bevel");

    let chamfer = result.new_faces[0];
    let mut ring: Vec<Vec3> = Vec::new();
    let face = &bmesh.faces[chamfer];
    let mut cur = face.loop_first;
    for _ in 0..face.loop_count {
        ring.push(bmesh.verts[bmesh.loops[cur].vert].co);
        cur = bmesh.loops[cur].next;
    }
    assert_eq!(ring.len(), 4, "chamfer should be a quad");

    // Opposite edges as vectors (with consistent winding) should sum to zero.
    // ring is [a, b, c, d] traversed in order. Vectors: a->b, b->c, c->d, d->a.
    // For a parallelogram: a->b is parallel and equal-magnitude to d->c (which is
    // -(c->d)). So (a->b) + (c->d) == 0 and (b->c) + (d->a) == 0.
    let e_ab = ring[1] - ring[0];
    let e_bc = ring[2] - ring[1];
    let e_cd = ring[3] - ring[2];
    let e_da = ring[0] - ring[3];

    let parallel_1 = (e_ab + e_cd).length();
    let parallel_2 = (e_bc + e_da).length();

    assert!(
        parallel_1 < 1e-4,
        "chamfer opposite edges (a-b vs c-d) must cancel: got |sum| = {parallel_1}, e_ab={e_ab:?}, e_cd={e_cd:?}"
    );
    assert!(
        parallel_2 < 1e-4,
        "chamfer opposite edges (b-c vs d-a) must cancel: got |sum| = {parallel_2}, e_bc={e_bc:?}, e_da={e_da:?}"
    );

    // Per-edge length consistency: both pairs of opposite edges share length.
    assert!(
        (e_ab.length() - e_cd.length()).abs() < 1e-4,
        "|a->b| ({}) should equal |c->d| ({})",
        e_ab.length(),
        e_cd.length()
    );
    assert!(
        (e_bc.length() - e_da.length()).abs() < 1e-4,
        "|b->c| ({}) should equal |d->a| ({})",
        e_bc.length(),
        e_da.length()
    );
}

#[test]
fn bevel_every_cube_edge_is_a_parallelogram() {
    // Repeat the parallelogram check for every individual edge of a cube,
    // so any per-edge orientation bug shows up.
    for edge_idx in 0..12 {
        let brush = Brush::cuboid(2.0, 2.0, 2.0);
        let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
        let edge = bmesh.edges.keys().nth(edge_idx).expect("cube has 12 edges");
        let result = edge_bevel(&mut bmesh, &[edge], 0.25).expect("bevel");
        let chamfer = result.new_faces[0];

        let mut ring: Vec<Vec3> = Vec::new();
        let face = &bmesh.faces[chamfer];
        let mut cur = face.loop_first;
        for _ in 0..face.loop_count {
            ring.push(bmesh.verts[bmesh.loops[cur].vert].co);
            cur = bmesh.loops[cur].next;
        }
        assert_eq!(ring.len(), 4, "edge {edge_idx}: chamfer must be a quad");

        let e_ab = ring[1] - ring[0];
        let e_bc = ring[2] - ring[1];
        let e_cd = ring[3] - ring[2];
        let e_da = ring[0] - ring[3];

        let p1 = (e_ab + e_cd).length();
        let p2 = (e_bc + e_da).length();
        assert!(
            p1 < 1e-4,
            "edge {edge_idx}: opposite edges (ab, cd) don't cancel: |sum| = {p1}"
        );
        assert!(
            p2 < 1e-4,
            "edge {edge_idx}: opposite edges (bc, da) don't cancel: |sum| = {p2}"
        );

        // Coplanarity: scalar triple product of three consecutive edge vectors
        // should be 0.
        let cross = e_ab.cross(e_bc);
        let coplanar = cross.dot(e_cd).abs();
        assert!(
            coplanar < 1e-4,
            "edge {edge_idx}: chamfer not coplanar: {coplanar}"
        );
    }
}

#[test]
fn bevel_preserves_face_count_plus_new_chamfer() {
    // Verify that beveling N edges adds exactly N chamfer faces (no more, no
    // less). Use 2 parallel cube edges to avoid the shared-vertex case.
    let brush = Brush::cuboid(2.0, 2.0, 2.0);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let initial_faces = bmesh.face_count();

    // Pick two edges that don't share a vertex: take the first edge and find
    // another whose vert set is disjoint from it.
    let first = bmesh.edges.keys().next().expect("at least one edge");
    let first_verts = bmesh.edges[first].v;
    let second = bmesh
        .edges
        .iter()
        .find(|(k, e)| {
            *k != first
                && e.v[0] != first_verts[0]
                && e.v[0] != first_verts[1]
                && e.v[1] != first_verts[0]
                && e.v[1] != first_verts[1]
        })
        .map(|(k, _)| k)
        .expect("a disjoint edge exists on a cube");

    let result = edge_bevel(&mut bmesh, &[first, second], 0.1).expect("bevel two cube edges");
    assert_eq!(result.new_faces.len(), 2, "one chamfer per beveled edge");
    assert_eq!(
        bmesh.face_count(),
        initial_faces + 2,
        "face count +N chamfers"
    );
    bmesh
        .validate()
        .expect("invariants hold after multi-edge bevel");
}
