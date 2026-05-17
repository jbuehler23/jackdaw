use bevy::math::Vec3;
use jackdaw_geometry::halfedge::{
    HalfedgeMesh, FaceKey,
    ops::face_poke::{PokeError, face_poke},
};
use jackdaw_jsn::Brush;

/// Find the cuboid face whose cached normal matches `target`.
/// Cuboid faces are 6 axis-aligned quads, so this uniquely identifies one.
fn face_with_normal(mesh: &HalfedgeMesh, target: Vec3) -> FaceKey {
    mesh
        .faces
        .iter()
        .find(|(_, f)| f.normal_cache.distance(target) < 1e-3)
        .map(|(k, _)| k)
        .expect("face with target normal exists on cuboid")
}

fn ring_verts(mesh: &HalfedgeMesh, face: FaceKey) -> Vec<jackdaw_geometry::halfedge::VertKey> {
    let mut out = Vec::new();
    let f = &mesh.faces[face];
    let mut cur = f.loop_first;
    for _ in 0..f.loop_count {
        out.push(mesh.loops[cur].vert);
        cur = mesh.loops[cur].next;
    }
    out
}

#[test]
fn poke_cube_top_face_creates_4_triangles() {
    // Unit cuboid with half-extents = 1.0; the +Y face has centroid (0, 1, 0).
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let initial_verts = mesh.vert_count();
    let initial_edges = mesh.edge_count();
    let initial_faces = mesh.face_count();
    assert_eq!(initial_verts, 8);
    assert_eq!(initial_edges, 12);
    assert_eq!(initial_faces, 6);

    let top = face_with_normal(&mesh, Vec3::Y);
    let original_ring = ring_verts(&mesh, top);
    assert_eq!(original_ring.len(), 4);

    let result = face_poke(&mut mesh, top, Vec3::new(0.0, 1.0, 0.0)).expect("poke");

    // The poked face is gone; 4 fan triangles replace it.
    // Net face count: 6 - 1 + 4 = 9.
    assert_eq!(mesh.face_count(), initial_faces + 3, "9 faces total");
    // 4 fan triangles created.
    assert_eq!(result.new_faces.len(), 4, "4 fan triangles");
    // 4 new spoke edges from center vert to each ring vert.
    assert_eq!(result.new_edges.len(), 4, "4 new spoke edges");
    // Original 8 verts + 1 new center vert.
    assert_eq!(mesh.vert_count(), initial_verts + 1, "9 vertices");
    // Original 12 edges + 4 new spokes.
    assert_eq!(mesh.edge_count(), initial_edges + 4, "16 edges");

    // Center vert is at the poke point.
    assert!(mesh.verts[result.center_vert].co.distance(Vec3::Y) < 1e-5);

    // Each new face is a triangle.
    for &f in &result.new_faces {
        assert_eq!(mesh.faces[f].loop_count, 3, "fan face is a triangle");
    }

    // Each new edge connects the center vert to a ring vert.
    for &e in &result.new_edges {
        let ed = &mesh.edges[e];
        let touches_center = ed.v[0] == result.center_vert || ed.v[1] == result.center_vert;
        assert!(touches_center, "spoke edge must touch center vert");
        let other = if ed.v[0] == result.center_vert {
            ed.v[1]
        } else {
            ed.v[0]
        };
        assert!(
            original_ring.contains(&other),
            "spoke edge other endpoint must be a ring vert"
        );
    }

    mesh.validate().expect("post-poke valid");
}

#[test]
fn poke_off_plane_errors() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let top = face_with_normal(&mesh, Vec3::Y);
    // The +Y face plane is y = 1. Poke at y = 5 is way off the plane.
    let result = face_poke(&mut mesh, top, Vec3::new(0.0, 5.0, 0.0));
    assert!(
        matches!(result, Err(PokeError::PointNotInFacePlane)),
        "expected PointNotInFacePlane, got {result:?}"
    );
}

#[test]
fn poke_degenerate_face_errors() {
    // Synthesize a degenerate face with loop_count == 2. The face won't be a
    // real well-formed face, but we only need the early-out check to fire.
    let mut mesh = HalfedgeMesh::default();
    let v0 = mesh.add_vert(Vec3::ZERO);
    let v1 = mesh.add_vert(Vec3::X);
    // Insert a "fake" face with loop_count = 2; loop_first need not point at a
    // real loop because the op rejects before walking.
    let face = mesh.faces.insert(jackdaw_geometry::halfedge::HalfedgeFace {
        flag: jackdaw_geometry::halfedge::FaceFlag::empty(),
        material_idx: 0,
        loop_first: jackdaw_geometry::halfedge::LoopKey::default(),
        loop_count: 2,
        normal_cache: Vec3::Z,
    });
    // Silence unused-var warnings; we never read these in this test.
    let _ = (v0, v1);

    let result = face_poke(&mut mesh, face, Vec3::ZERO);
    assert!(
        matches!(result, Err(PokeError::Degenerate)),
        "expected Degenerate, got {result:?}"
    );
}

#[test]
fn poke_preserves_face_material_idx() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let top = face_with_normal(&mesh, Vec3::Y);
    let original_material_idx = mesh.faces[top].material_idx;

    let result = face_poke(&mut mesh, top, Vec3::new(0.0, 1.0, 0.0)).expect("poke");

    // All N fan triangles inherit the original face's material_idx.
    for &f in &result.new_faces {
        assert_eq!(
            mesh.faces[f].material_idx, original_material_idx,
            "fan tri inherits poked face material_idx"
        );
    }

    // After flatten_to_topology, faces sort by material_idx; the 4 fan tris
    // share the same material_idx so they end up as 4 adjacent polygon slots
    // (one extra over the original 6 - 1 + 4 = 9 polys, with 4 in a row
    // matching the original material_idx slot).
    let topology = mesh.flatten_to_topology();
    assert_eq!(topology.polygons.len(), 9);

    // Count polygons whose post-flatten material grouping matches the poked
    // face. We don't store material_idx on `MeshPoly` directly; instead we
    // verify the fan tris sit in a contiguous block after sorting by checking
    // that exactly 4 tris (loop_total == 3) exist and the remaining 5 are
    // quads (loop_total == 4 from the other 5 original faces).
    let tri_count = topology
        .polygons
        .iter()
        .filter(|p| p.loop_total == 3)
        .count();
    let quad_count = topology
        .polygons
        .iter()
        .filter(|p| p.loop_total == 4)
        .count();
    assert_eq!(tri_count, 4, "4 fan triangles in flattened topology");
    assert_eq!(quad_count, 5, "5 surviving original quad faces");
}

#[test]
fn poke_preserves_invariants() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let top = face_with_normal(&mesh, Vec3::Y);
    face_poke(&mut mesh, top, Vec3::new(0.0, 1.0, 0.0)).expect("poke");
    mesh.validate().expect("post-poke validate ok");
}
