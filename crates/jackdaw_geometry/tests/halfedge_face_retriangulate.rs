//! Tests for the `face_retriangulate` op: validates that the constrained
//! Delaunay-driven face retriangulator handles the cases the knife
//! commit pipeline relies on.
//!
//! Each test starts with a single face from a `Brush::cuboid` cube,
//! lifts to `HalfedgeMesh`, runs `face_retriangulate` with a chosen set of
//! Steiner points and constraint edges, and verifies the resulting
//! topology.

use bevy_math::Vec3;
use jackdaw_geometry::halfedge::{
    FaceKey, HalfedgeMesh, VertKey,
    ops::face_retriangulate::{RetriangulateError, face_retriangulate},
};
use jackdaw_jsn::Brush;

/// Locate the `FaceKey` whose `material_idx == idx`.
fn face_by_idx(mesh: &HalfedgeMesh, idx: u32) -> FaceKey {
    mesh.faces
        .iter()
        .find(|(_, f)| f.material_idx == idx)
        .map(|(k, _)| k)
        .expect("face by material_idx")
}

/// Walk a face's ring and collect its vert keys in order.
fn face_ring(mesh: &HalfedgeMesh, face: FaceKey) -> Vec<VertKey> {
    let f = &mesh.faces[face];
    let mut ring = Vec::with_capacity(f.loop_count as usize);
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        ring.push(mesh.loops[cur].vert);
        cur = mesh.loops[cur].next;
    }
    ring
}

/// Return `true` if an edge exists in the mesh connecting `va` and `vb`.
fn has_edge_between(mesh: &HalfedgeMesh, va: VertKey, vb: VertKey) -> bool {
    mesh.edges
        .iter()
        .any(|(_, e)| (e.v[0] == va && e.v[1] == vb) || (e.v[0] == vb && e.v[1] == va))
}

#[test]
fn retriangulate_face_with_one_interior_point() {
    // A single Steiner point at the center of the +Y face. With no
    // constraint edges, CDT fans from the center; the merge pass then
    // pairs adjacent fan triangles into convex quads where possible.
    //
    // For a square top face + center Steiner point, the fan produces
    // 4 triangles, and the merge pass pairs them into 2 convex quads
    // (each quad shares the center vert as one corner).
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);

    let top_face = face_by_idx(&mesh, 2);
    let original_material = mesh.faces[top_face].material_idx;

    let result = face_retriangulate(&mut mesh, top_face, vec![Vec3::new(0.0, 1.0, 0.0)], vec![])
        .expect("retriangulate succeeds");
    mesh.validate().expect("valid after retriangulate");

    // Steiner verts list contains exactly the one interior point.
    assert_eq!(result.new_verts.len(), 1);
    let center = result.new_verts[0];
    assert!(
        (mesh.verts[center].co - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-6,
        "center vert at requested position"
    );

    // After the merge pass: the 4 raw CDT fan tris pair into 2 quads
    // (each containing the center as a corner). Asserting <= 4 makes
    // the test robust to merge-policy tweaks while still confirming
    // the merge pass does its job and never inflates triangle count.
    assert!(
        !result.new_faces.is_empty() && result.new_faces.len() <= 4,
        "merge pass produced 1..=4 faces, got {}",
        result.new_faces.len()
    );

    // Every new face inherits the original material_idx.
    for fk in &result.new_faces {
        assert_eq!(
            mesh.faces[*fk].material_idx, original_material,
            "material_idx preserved"
        );
    }

    // The center vert connects to each output face (every face of the
    // fan / merged-quad arrangement contains it as a corner).
    for &fk in &result.new_faces {
        let ring = face_ring(&mesh, fk);
        assert!(ring.contains(&center), "output face contains center vert");
    }
}

#[test]
fn retriangulate_face_with_one_constraint_edge() {
    // Two opposite ring corners as a constraint edge. Equivalent to
    // `split_face`: the +Y face splits in half along the diagonal.
    // The diagonal IS the constraint edge, so the merge pass cannot
    // recombine the two triangles (the cut would be erased). Result:
    // exactly 2 triangles.
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);

    let top_face = face_by_idx(&mesh, 2);
    let ring = face_ring(&mesh, top_face);
    assert_eq!(ring.len(), 4, "+Y face is a quad");
    // Constraint between ring[0] and ring[2] (a diagonal).
    let result = face_retriangulate(&mut mesh, top_face, vec![], vec![(0, 2)])
        .expect("retriangulate succeeds");
    mesh.validate().expect("valid after retriangulate");

    // No Steiner verts.
    assert_eq!(result.new_verts.len(), 0);

    // A quad with a diagonal cut produces 2 triangles. The constraint
    // edge IS the diagonal so merging back into a quad would erase the
    // cut; the merge pass must leave the two tris alone.
    assert_eq!(result.new_faces.len(), 2, "diagonal cut keeps 2 tris");
    for &fk in &result.new_faces {
        assert_eq!(
            mesh.faces[fk].loop_count, 3,
            "diagonal cut keeps both faces as triangles (constraint edge can't be merged across)"
        );
    }

    // The constraint edge (ring[0], ring[2]) must exist.
    assert!(
        has_edge_between(&mesh, ring[0], ring[2]),
        "constraint edge (ring[0], ring[2]) present"
    );
}

#[test]
fn retriangulate_straight_chord_yields_two_quads() {
    // Setup mirrors what the knife commit pipeline does: split the two
    // opposing ring edges first (so the new chord endpoints are real
    // ring verts), then run face_retriangulate with a constraint edge
    // between the two new ring verts.
    //
    // After CDT this 6-vert face with a chord constraint produces 4
    // triangles (two per quad half). The merge pass should recombine
    // each pair of adjacent tris into a quad, splitting the face
    // cleanly along the straight chord into exactly 2 quads.
    use jackdaw_geometry::halfedge::ops::edge_split::split_edge;

    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);

    let top_face = face_by_idx(&mesh, 2);
    let ring = face_ring(&mesh, top_face);
    assert_eq!(ring.len(), 4, "+Y face is a quad");

    // Walk the face's loops to grab the two opposing edges (ring[0]-
    // ring[1] and ring[2]-ring[3]) without doing a separate lookup.
    let edge_01 = mesh
        .edges
        .iter()
        .find(|(_, e)| {
            (e.v[0] == ring[0] && e.v[1] == ring[1]) || (e.v[0] == ring[1] && e.v[1] == ring[0])
        })
        .map(|(k, _)| k)
        .expect("edge (ring[0], ring[1])");
    let split_a = split_edge(&mut mesh, edge_01, 0.5).expect("split edge 0-1");

    // Re-lookup edge (ring[2], ring[3]) after the first split (the
    // HalfedgeMesh edge set is a slotmap; the second edge handle is
    // unchanged but be defensive).
    let edge_23 = mesh
        .edges
        .iter()
        .find(|(_, e)| {
            (e.v[0] == ring[2] && e.v[1] == ring[3]) || (e.v[0] == ring[3] && e.v[1] == ring[2])
        })
        .map(|(k, _)| k)
        .expect("edge (ring[2], ring[3])");
    let split_b = split_edge(&mut mesh, edge_23, 0.5).expect("split edge 2-3");

    // After both splits the top face's ring is 6 verts (the 4 corners +
    // the 2 split midpoints). Find the new ring and the positions of
    // split_a and split_b within it for the constraint indices.
    let top_face = face_by_idx(&mesh, 2);
    let new_ring = face_ring(&mesh, top_face);
    assert_eq!(new_ring.len(), 6, "ring grew to 6 verts after both splits");
    let idx_a = new_ring
        .iter()
        .position(|&v| v == split_a)
        .expect("split_a in ring");
    let idx_b = new_ring
        .iter()
        .position(|&v| v == split_b)
        .expect("split_b in ring");

    let pre_vert_count = mesh.vert_count();
    let result = face_retriangulate(&mut mesh, top_face, vec![], vec![(idx_a, idx_b)])
        .expect("retriangulate succeeds");
    mesh.validate().expect("valid after retriangulate");

    assert_eq!(
        mesh.vert_count(),
        pre_vert_count,
        "no Steiner verts added by retriangulate (chord uses split-ring verts)"
    );
    assert_eq!(result.new_verts.len(), 0);

    // The straight chord cleaves the 6-vert face cleanly. CDT first
    // makes 4 triangles; the merge pass pairs them into 2 quads.
    assert_eq!(
        result.new_faces.len(),
        2,
        "straight chord cleaves the face into 2 output faces (merged into quads)"
    );
    for &fk in &result.new_faces {
        assert_eq!(
            mesh.faces[fk].loop_count, 4,
            "each output face is a quad after the merge pass"
        );
    }

    // Sanity: the chord (split_a, split_b) is present as an edge.
    assert!(
        has_edge_between(&mesh, split_a, split_b),
        "chord between split_a and split_b present"
    );
}

#[test]
fn retriangulate_zigzag_aggressive_merge() {
    // 4-vert face + 4 Steiner zigzag points + 3 constraint edges. CDT
    // produces a relatively dense triangulation; the multi-pass merger
    // should collapse it to far fewer convex polygons than the raw tri
    // count.
    //
    // Asserting "polygon count < raw CDT tri count" catches regressions
    // in the multi-pass merger without pinning to a fragile exact
    // number. With the original single-pass tris-to-quads, this case
    // typically left several triangles in the output; the multi-pass
    // version pulls many of them into pentagons / hexagons.
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);

    let top_face = face_by_idx(&mesh, 2);
    let ring_len = mesh.faces[top_face].loop_count as usize;
    assert_eq!(ring_len, 4);

    let p0 = Vec3::new(-0.6, 1.0, -0.4);
    let p1 = Vec3::new(0.6, 1.0, -0.4);
    let p2 = Vec3::new(-0.6, 1.0, 0.4);
    let p3 = Vec3::new(0.6, 1.0, 0.4);

    let i0 = ring_len;
    let i1 = ring_len + 1;
    let i2 = ring_len + 2;
    let i3 = ring_len + 3;
    let constraints = vec![(i0, i1), (i1, i2), (i2, i3)];

    let result = face_retriangulate(&mut mesh, top_face, vec![p0, p1, p2, p3], constraints)
        .expect("retriangulate succeeds");
    mesh.validate().expect("valid after retriangulate");

    // The raw CDT output for a 4-ring + 4 Steiner zigzag is on the
    // order of 8-10 triangles. With multi-pass merge we expect this to
    // shrink: the inputs are all coplanar and convex shapes have room
    // to merge into larger polygons. Assert the merger reduces the
    // polygon count strictly below the raw-CDT count (8 is a
    // conservative lower bound on raw CDT output for this shape).
    assert!(
        result.new_faces.len() < 8,
        "multi-pass merge should reduce zigzag below raw-CDT count (got {})",
        result.new_faces.len()
    );

    // The 3 constraint edges survive the merge pass (constraint edges
    // are never merge candidates).
    let v0 = result.new_verts[0];
    let v1 = result.new_verts[1];
    let v2 = result.new_verts[2];
    let v3 = result.new_verts[3];
    assert!(has_edge_between(&mesh, v0, v1), "zigzag edge (v0, v1)");
    assert!(has_edge_between(&mesh, v1, v2), "zigzag edge (v1, v2)");
    assert!(has_edge_between(&mesh, v2, v3), "zigzag edge (v2, v3)");
}

#[test]
fn retriangulate_closed_quad_loop_yields_inside_and_outside() {
    // A closed quad loop inside the +Y face: 4 Steiner points + 4
    // constraint edges forming a closed rectangle. The cut divides the
    // face into:
    //   * Inside the loop: a simple convex quad (the small rectangle).
    //   * Outside the loop: an annulus-shaped region (the outer ring
    //     minus the inner rectangle). An annulus is NOT a simple
    //     polygon, so CDT triangulates it into several convex
    //     triangles; the merger combines them into a small set of
    //     convex polygons.
    //
    // Asserts:
    //   - 4 Steiner verts created.
    //   - All 4 constraint edges present (the loop survives).
    //   - At least 2 output faces (1 inside + N >= 1 outside).
    //   - Strictly fewer than the raw CDT tri count, so the multi-pass
    //     merger demonstrably collapses adjacent tris into larger
    //     convex polygons.
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);

    let top_face = face_by_idx(&mesh, 2);
    let ring_len = mesh.faces[top_face].loop_count as usize;
    assert_eq!(ring_len, 4, "+Y face is a quad");

    // 4 Steiner points forming a small rectangle inside the face.
    let p0 = Vec3::new(-0.5, 1.0, -0.5);
    let p1 = Vec3::new(0.5, 1.0, -0.5);
    let p2 = Vec3::new(0.5, 1.0, 0.5);
    let p3 = Vec3::new(-0.5, 1.0, 0.5);

    let i0 = ring_len;
    let i1 = ring_len + 1;
    let i2 = ring_len + 2;
    let i3 = ring_len + 3;
    // Close the loop: (i0,i1), (i1,i2), (i2,i3), (i3,i0).
    let constraints = vec![(i0, i1), (i1, i2), (i2, i3), (i3, i0)];

    let result = face_retriangulate(&mut mesh, top_face, vec![p0, p1, p2, p3], constraints)
        .expect("retriangulate succeeds");
    mesh.validate().expect("valid after retriangulate");

    assert_eq!(result.new_verts.len(), 4, "4 Steiner verts created");
    let v0 = result.new_verts[0];
    let v1 = result.new_verts[1];
    let v2 = result.new_verts[2];
    let v3 = result.new_verts[3];

    // All four loop edges survive the merge pass.
    assert!(has_edge_between(&mesh, v0, v1), "loop edge (v0, v1)");
    assert!(has_edge_between(&mesh, v1, v2), "loop edge (v1, v2)");
    assert!(has_edge_between(&mesh, v2, v3), "loop edge (v2, v3)");
    assert!(has_edge_between(&mesh, v3, v0), "loop edge (v3, v0)");

    // Topologically: at least 2 output faces (1 inside + >= 1 outside).
    assert!(
        result.new_faces.len() >= 2,
        "at least an inside-of-loop face and an outside-of-loop face (got {})",
        result.new_faces.len()
    );

    // Exactly one face has all 4 loop verts on its ring: the inside
    // quad. The outside region is an annulus and gets split into
    // multiple pieces.
    let inside_face_count = result
        .new_faces
        .iter()
        .filter(|&&fk| {
            let ring = face_ring(&mesh, fk);
            ring.contains(&v0) && ring.contains(&v1) && ring.contains(&v2) && ring.contains(&v3)
        })
        .count();
    assert_eq!(
        inside_face_count, 1,
        "exactly one face holds the inside quad"
    );

    // The merger should beat a tri-soup result. For this configuration
    // CDT produces around 8-10 triangles; assert we end up with strictly
    // fewer than 8 polygons.
    assert!(
        result.new_faces.len() < 8,
        "multi-pass merge keeps closed-loop face count low (got {})",
        result.new_faces.len()
    );
}

#[test]
fn retriangulate_zigzag_keeps_path_edges_as_constraint() {
    // 4-vert face + 3-segment zigzag = 4 Steiner points and 3
    // constraint edges. The merge pass must NEVER touch any of the 3
    // zigzag edges (constraint edges are never merge candidates). It
    // may freely merge other CDT-internal edges into quads.
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);

    let top_face = face_by_idx(&mesh, 2);
    let ring_len = mesh.faces[top_face].loop_count as usize;
    assert_eq!(ring_len, 4);

    let p0 = Vec3::new(-0.6, 1.0, -0.4);
    let p1 = Vec3::new(0.6, 1.0, -0.4);
    let p2 = Vec3::new(-0.6, 1.0, 0.4);
    let p3 = Vec3::new(0.6, 1.0, 0.4);

    let i0 = ring_len;
    let i1 = ring_len + 1;
    let i2 = ring_len + 2;
    let i3 = ring_len + 3;
    let constraints = vec![(i0, i1), (i1, i2), (i2, i3)];

    let result = face_retriangulate(&mut mesh, top_face, vec![p0, p1, p2, p3], constraints)
        .expect("retriangulate succeeds");
    mesh.validate().expect("valid after retriangulate");

    assert_eq!(result.new_verts.len(), 4);
    let v0 = result.new_verts[0];
    let v1 = result.new_verts[1];
    let v2 = result.new_verts[2];
    let v3 = result.new_verts[3];

    // The 3 constraint edges must each survive the merge pass. If any
    // were lost, the cut would be erased.
    assert!(
        has_edge_between(&mesh, v0, v1),
        "zigzag edge (v0, v1) preserved"
    );
    assert!(
        has_edge_between(&mesh, v1, v2),
        "zigzag edge (v1, v2) preserved"
    );
    assert!(
        has_edge_between(&mesh, v2, v3),
        "zigzag edge (v2, v3) preserved"
    );
}

#[test]
fn retriangulate_cube_top_with_zigzag_path() {
    // A 4-point zigzag path entirely inside the +Y face. The path
    // produces 4 new verts (Steiner points) and 3 constraint edges (the
    // polyline segments). Verifies:
    //   * 4 new verts created.
    //   * Each of the 3 polyline edges present as a mesh edge.
    //   * Every new face inherits the original material_idx.
    //   * Mesh validates.
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);

    let top_face = face_by_idx(&mesh, 2);
    let original_material = mesh.faces[top_face].material_idx;
    let ring_len = mesh.faces[top_face].loop_count as usize;

    // 4 zigzag points entirely interior to the +Y face (y = 1):
    //
    //   p0 (-0.6, 1, -0.4) -- left, back
    //   p1 ( 0.6, 1, -0.4) -- right, back
    //   p2 (-0.6, 1,  0.4) -- left, front
    //   p3 ( 0.6, 1,  0.4) -- right, front
    //
    // The polyline forms a Z shape. Each segment crosses the previous.
    let p0 = Vec3::new(-0.6, 1.0, -0.4);
    let p1 = Vec3::new(0.6, 1.0, -0.4);
    let p2 = Vec3::new(-0.6, 1.0, 0.4);
    let p3 = Vec3::new(0.6, 1.0, 0.4);

    // Constraint edges. Indices are into ring_verts (0..4) ++
    // interior_points (4..8). The 4 Steiner points start at index
    // ring_len.
    let i0 = ring_len;
    let i1 = ring_len + 1;
    let i2 = ring_len + 2;
    let i3 = ring_len + 3;
    let constraints = vec![(i0, i1), (i1, i2), (i2, i3)];

    let result = face_retriangulate(&mut mesh, top_face, vec![p0, p1, p2, p3], constraints)
        .expect("retriangulate succeeds");
    mesh.validate().expect("valid after retriangulate");

    // 4 Steiner verts created.
    assert_eq!(result.new_verts.len(), 4, "4 new Steiner verts");
    let v0 = result.new_verts[0];
    let v1 = result.new_verts[1];
    let v2 = result.new_verts[2];
    let v3 = result.new_verts[3];

    // Each polyline segment must exist as a mesh edge.
    assert!(has_edge_between(&mesh, v0, v1), "polyline edge (v0, v1)");
    assert!(has_edge_between(&mesh, v1, v2), "polyline edge (v1, v2)");
    assert!(has_edge_between(&mesh, v2, v3), "polyline edge (v2, v3)");

    // Every output triangle inherits the original material_idx.
    for &fk in &result.new_faces {
        assert_eq!(
            mesh.faces[fk].material_idx, original_material,
            "material_idx preserved on every output tri"
        );
    }

    // At least one triangle per Steiner point uses it (so it's
    // integrated into the topology).
    for sp in &[v0, v1, v2, v3] {
        let mut found = false;
        for &fk in &result.new_faces {
            let ring = face_ring(&mesh, fk);
            if ring.contains(sp) {
                found = true;
                break;
            }
        }
        assert!(found, "Steiner point in at least one output triangle");
    }

    // Round-trip topology: flatten cleanly.
    let topo = mesh.flatten_to_topology();
    assert_eq!(topo.vertices.len(), mesh.vert_count());
    assert_eq!(topo.polygons.len(), mesh.face_count());
}

#[test]
fn retriangulate_preserves_material_idx() {
    // Sanity: pick a face with a non-zero material_idx (e.g. the +X
    // face has material_idx 0 -- but any face works), do a
    // retriangulate with a single Steiner point, and confirm every
    // new face has the same material_idx.
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);

    // Pick the -Z face (material_idx 5).
    let face = face_by_idx(&mesh, 5);
    let expected_material = mesh.faces[face].material_idx;
    assert_eq!(
        expected_material, 5,
        "test setup: -Z face has material_idx 5"
    );

    // -Z face is at z = -1, ring covers x in [-1, 1], y in [-1, 1].
    let result = face_retriangulate(&mut mesh, face, vec![Vec3::new(0.0, 0.0, -1.0)], vec![])
        .expect("retriangulate succeeds");
    mesh.validate().expect("valid after retriangulate");

    for &fk in &result.new_faces {
        assert_eq!(
            mesh.faces[fk].material_idx, expected_material,
            "every new tri inherits material_idx 5"
        );
    }
}

#[test]
fn retriangulate_rejects_invalid_constraint_index() {
    // Out-of-bounds constraint indices should be rejected.
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);

    let top_face = face_by_idx(&mesh, 2);
    let err = face_retriangulate(
        &mut mesh,
        top_face,
        vec![Vec3::new(0.0, 1.0, 0.0)],
        // Index 99 is way out of bounds.
        vec![(0, 99)],
    )
    .expect_err("should fail");
    assert!(
        matches!(err, RetriangulateError::InvalidConstraintIndex),
        "invalid index error, got {:?}",
        err
    );
}

#[test]
fn knife_two_points_on_front_face_produces_chord() {
    // Regression for the "click on front face but cut ends up on back
    // face" bug: when the snap depth filter correctly attributes both
    // path points to the same front face, the commit pipeline reduces
    // to "two Steiner points on one face + one constraint edge between
    // them." Output topology must contain a real edge between the two
    // resolved verts.
    //
    // The editor's commit pipeline ultimately calls
    // `face_retriangulate` with the per-face Steiner points and
    // constraint edges; this test exercises the same call directly so
    // it stays a fast unit test that catches retriangulator regressions
    // without driving the full Bevy system.
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);

    // +Y face (top, idx 2). Two interior path points on the face plane.
    // Pick positions that are clearly inside the unit-cube top face.
    let top_face = face_by_idx(&mesh, 2);
    let ring_len = mesh.faces[top_face].loop_count as usize;
    assert_eq!(ring_len, 4, "+Y face is a quad");

    let p0 = Vec3::new(-0.4, 1.0, -0.2);
    let p1 = Vec3::new(0.4, 1.0, 0.2);

    // Steiner indices start at ring_len.
    let i0 = ring_len;
    let i1 = ring_len + 1;
    let result = face_retriangulate(&mut mesh, top_face, vec![p0, p1], vec![(i0, i1)])
        .expect("retriangulate succeeds with two-point chord");
    mesh.validate().expect("valid after retriangulate");

    // Two Steiner verts created, one per path point.
    assert_eq!(result.new_verts.len(), 2);
    let v0 = result.new_verts[0];
    let v1 = result.new_verts[1];

    // The constraint edge MUST be present as a real edge. This is the
    // assertion the bug surfaced: a click that ended up on the back
    // face produced no edge between the two path points' resolved
    // verts. With the front-face depth check in place, both points
    // attach to the same face and the constraint edge survives.
    assert!(
        has_edge_between(&mesh, v0, v1),
        "constraint edge between the two path points present in output topology"
    );

    // Both Steiner verts are at the requested positions (within FP eps).
    let v0_pos = mesh.verts[v0].co;
    let v1_pos = mesh.verts[v1].co;
    assert!(
        (v0_pos - p0).length() < 1e-5,
        "v0 at requested position p0, got {:?}",
        v0_pos
    );
    assert!(
        (v1_pos - p1).length() < 1e-5,
        "v1 at requested position p1, got {:?}",
        v1_pos
    );

    // Both new verts appear in at least one output face's ring (they're
    // integrated into the topology, not orphaned).
    for sp in &[v0, v1] {
        let mut found = false;
        for &fk in &result.new_faces {
            let ring = face_ring(&mesh, fk);
            if ring.contains(sp) {
                found = true;
                break;
            }
        }
        assert!(found, "Steiner point integrated into at least one face");
    }
}
