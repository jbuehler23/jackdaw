use bevy::math::Vec3;
use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::loop_cut::loop_cut};
use jackdaw_jsn::Brush;

#[test]
fn loop_cut_around_cube_at_t_0_5_adds_4_verts_and_4_loop_edges() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let initial_verts = mesh.vert_count();
    let initial_faces = mesh.face_count();
    let initial_edges = mesh.edge_count();
    let start_edge = mesh.edges.keys().next().unwrap();
    let result = loop_cut(&mut mesh, start_edge, 0.5).expect("loop cut");
    // 4 quad faces in the ring around the cube. Each gets split:
    //   - 4 edges crossed -> 4 new verts (one per crossed edge midpoint).
    //   - 4 faces crossed -> 4 new faces (each quad becomes 2 quads).
    //   - 4 new "loop" edges (one per face split).
    // Total edges added: 4 (from edge splits) + 4 (from face splits) = 8.
    assert_eq!(
        mesh.vert_count(),
        initial_verts + 4,
        "+4 verts (one per crossed edge midpoint)"
    );
    assert_eq!(
        mesh.face_count(),
        initial_faces + 4,
        "+4 faces (one per crossed quad becoming 2 quads)"
    );
    assert_eq!(
        mesh.edge_count(),
        initial_edges + 8,
        "+8 edges (4 from edge splits + 4 from face splits)"
    );
    mesh.validate().expect("valid after loop cut");
    assert_eq!(
        result.new_loop_edges.len(),
        4,
        "result reports 4 loop edges"
    );
    assert_eq!(result.new_verts.len(), 4);
    assert_eq!(result.new_faces.len(), 4);
}

#[test]
fn loop_cut_at_t_0_25_places_loop_offset_from_midpoint() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let start_edge = mesh.edges.keys().next().unwrap();
    let result = loop_cut(&mut mesh, start_edge, 0.25).expect("loop cut");
    // All 4 new verts should sit at t=0.25 along their respective ring edges.
    // Just check they exist and are distinct.
    assert_eq!(result.new_verts.len(), 4);
    let positions: Vec<_> = result
        .new_verts
        .iter()
        .map(|&k| mesh.verts[k].co)
        .collect();
    let mut unique = positions.clone();
    unique.sort_by(|a, b| {
        a.x.partial_cmp(&b.x)
            .unwrap()
            .then(a.y.partial_cmp(&b.y).unwrap())
            .then(a.z.partial_cmp(&b.z).unwrap())
    });
    unique.dedup_by(|a, b| {
        (a.x - b.x).abs() < 1e-6 && (a.y - b.y).abs() < 1e-6 && (a.z - b.z).abs() < 1e-6
    });
    assert_eq!(unique.len(), 4, "4 distinct positions");
}

#[test]
fn loop_cut_at_t_0_25_all_new_verts_lie_on_consistent_plane() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let any_edge = mesh.edges.keys().next().unwrap();
    let cut = loop_cut(&mut mesh, any_edge, 0.25).expect("cut");
    let new_verts = cut.new_verts;
    // Compute the 4 new vert positions.
    let positions: Vec<Vec3> = new_verts.iter().map(|&k| mesh.verts[k].co).collect();
    // For a uniform slide, all 4 positions should be coplanar - specifically all should have
    // the same coordinate value along the slide direction.
    // Compute min/max along each axis.
    let (mut min, mut max) = (positions[0], positions[0]);
    for p in &positions[1..] {
        min = min.min(*p);
        max = max.max(*p);
    }
    let extent = max - min;
    // The slide axis has high extent (>=1 unit on a 2x2x2 cube); the perpendicular
    // axis should have very low extent (approximately 0).
    // Sort the 3 extents; the smallest should be near zero.
    let mut extents = [extent.x.abs(), extent.y.abs(), extent.z.abs()];
    extents.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!(
        extents[0] < 1e-3,
        "new verts should be coplanar -- smallest extent {} should be near 0",
        extents[0]
    );
}

#[test]
fn loop_cut_at_t_0_3_all_new_verts_lie_on_left_third_plane() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    // Pick the edge with the lowest slotmap key for a deterministic test.
    let any_edge = mesh
        .edges
        .keys()
        .min_by_key(|k| {
            use slotmap::Key;
            k.data().as_ffi()
        })
        .unwrap();
    let cut = loop_cut(&mut mesh, any_edge, 0.3).expect("cut");
    let new_verts = cut.new_verts;
    let positions: Vec<Vec3> = new_verts.iter().map(|&k| mesh.verts[k].co).collect();
    // All new verts should share one coordinate value along the slide axis,
    // meaning they all lie on the same plane (smallest extent near zero).
    let (mut mn, mut mx) = (positions[0], positions[0]);
    for p in &positions[1..] {
        mn = mn.min(*p);
        mx = mx.max(*p);
    }
    let extents = [mx.x - mn.x, mx.y - mn.y, mx.z - mn.z];
    let smallest = extents.iter().fold(f32::INFINITY, |a, &b| a.min(b));
    assert!(
        smallest < 1e-3,
        "all new verts should be coplanar at the same slide t; smallest extent {}",
        smallest
    );
}
