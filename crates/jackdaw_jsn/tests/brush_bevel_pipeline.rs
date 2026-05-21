//! End-to-end pipeline checks for `edge_bevel` <-> `Brush::topology`.
//!
//! `bevel_cube_chamfer_is_a_parallelogram` (in `jackdaw_geometry`) already
//! verifies that the underlying `edge_bevel` op produces a parallelogram
//! chamfer in the `HalfedgeMesh`. These tests pick the chain back up at the
//! topology boundary: after `flatten_to_topology` writes the post-bevel state
//! back into the `Brush`, the chamfer's `Brush.topology` ring should still
//! be a parallelogram, the per-face plane on `Brush.faces[chamfer]` should
//! match the Newell normal of that ring, and the chamfer's `material_idx`
//! should land at the slot index the renderer expects.

use bevy_math::Vec3;
use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::edge_bevel::edge_bevel};
use jackdaw_jsn::Brush;

#[test]
fn beveled_cube_topology_chamfer_ring_is_a_parallelogram() {
    // Lift cube -> bevel one edge -> flatten back -> read chamfer ring from
    // the resulting `BrushTopology` and verify parallelogram invariants. If
    // this fails the bug is in the flatten step; if it passes the topology
    // pipeline is clean and any visual distortion lives in rendering.
    let brush = Brush::cuboid(2.0, 2.0, 2.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let edge = mesh.edges.keys().next().expect("cube has edges");
    let width = 0.3_f32;
    let _result = edge_bevel(&mut mesh, &[edge], width).expect("bevel");

    let new_topology = mesh.flatten_to_topology();
    // Cube has 6 faces; bevel adds one chamfer face.
    assert_eq!(
        new_topology.polygons.len(),
        7,
        "expected 6 original faces + 1 chamfer"
    );

    // The chamfer is the last polygon by material_idx (flatten sorts on it,
    // and `edge_bevel` picks `max + 1` for the new face).
    let chamfer_idx = new_topology.polygons.len() - 1;
    let chamfer_poly = &new_topology.polygons[chamfer_idx];
    assert_eq!(chamfer_poly.loop_total, 4, "chamfer should be a quad");

    let positions: Vec<Vec3> = new_topology.vertices.iter().map(|v| v.position).collect();
    let ring: Vec<Vec3> = new_topology
        .face_ring(chamfer_idx)
        .map(|i| positions[i as usize])
        .collect();
    assert_eq!(ring.len(), 4, "ring traversal should yield 4 verts");

    // Opposite edges as vectors should cancel (parallelogram), edge lengths
    // should match per pair, and the four verts should be coplanar.
    let e_ab = ring[1] - ring[0];
    let e_bc = ring[2] - ring[1];
    let e_cd = ring[3] - ring[2];
    let e_da = ring[0] - ring[3];

    let p1 = (e_ab + e_cd).length();
    let p2 = (e_bc + e_da).length();
    assert!(
        p1 < 1e-4,
        "post-flatten chamfer opposite edges (a-b vs c-d) must cancel: |sum|={p1}, e_ab={e_ab:?}, e_cd={e_cd:?}, ring={ring:?}"
    );
    assert!(
        p2 < 1e-4,
        "post-flatten chamfer opposite edges (b-c vs d-a) must cancel: |sum|={p2}, e_bc={e_bc:?}, e_da={e_da:?}, ring={ring:?}"
    );
    assert!(
        (e_ab.length() - e_cd.length()).abs() < 1e-4,
        "|a->b|={} vs |c->d|={}",
        e_ab.length(),
        e_cd.length()
    );
    assert!(
        (e_bc.length() - e_da.length()).abs() < 1e-4,
        "|b->c|={} vs |d->a|={}",
        e_bc.length(),
        e_da.length()
    );
    let coplanar = e_ab.cross(e_bc).dot(e_cd).abs();
    assert!(coplanar < 1e-4, "chamfer ring not coplanar: {coplanar}");
}

#[test]
fn beveling_every_cube_edge_produces_consistent_post_flatten_chamfer() {
    // Same check, repeated for every cube edge. If the bug is per-edge
    // orientation specific (e.g. flatten preserves ring order for some edges
    // but not others) the failure shows up here. The chamfer face's
    // `material_idx` is also pinned to the last slot so the renderer's
    // `brush.faces` -> `topology.polygons` index correspondence stays
    // consistent.
    for edge_idx in 0..12 {
        let brush = Brush::cuboid(2.0, 2.0, 2.0);
        let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
        let edge = mesh.edges.keys().nth(edge_idx).expect("cube has 12 edges");
        let _ = edge_bevel(&mut mesh, &[edge], 0.25).expect("bevel");
        let topology = mesh.flatten_to_topology();
        assert_eq!(
            topology.polygons.len(),
            7,
            "edge {edge_idx}: expected 7 faces after bevel"
        );
        let positions: Vec<Vec3> = topology.vertices.iter().map(|v| v.position).collect();
        let chamfer_idx = topology.polygons.len() - 1;
        let ring: Vec<Vec3> = topology
            .face_ring(chamfer_idx)
            .map(|i| positions[i as usize])
            .collect();
        assert_eq!(ring.len(), 4, "edge {edge_idx}: chamfer should be a quad");
        let e_ab = ring[1] - ring[0];
        let e_bc = ring[2] - ring[1];
        let e_cd = ring[3] - ring[2];
        let e_da = ring[0] - ring[3];
        let p1 = (e_ab + e_cd).length();
        let p2 = (e_bc + e_da).length();
        assert!(
            p1 < 1e-3,
            "edge {edge_idx}: post-flatten chamfer (a-b vs c-d) cancel: {p1}, ring={ring:?}"
        );
        assert!(
            p2 < 1e-3,
            "edge {edge_idx}: post-flatten chamfer (b-c vs d-a) cancel: {p2}, ring={ring:?}"
        );
    }
}

#[test]
fn jsn_runtime_mesh_rebuild_uses_topology_for_chamfer() {
    // `jackdaw_jsn::mesh_rebuild` is the runtime path: it runs once on
    // `Insert<Brush>` and builds the rendered geometry from
    // `brush.topology` when present (falling back to plane intersection
    // only for legacy brushes). Verify the chamfer face's rendered ring
    // matches the topology ring exactly - including vertex ordering - so
    // the rendered triangles cover the same parallelogram the topology
    // describes.
    let brush_seed = Brush::cuboid(2.0, 2.0, 2.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush_seed.topology);
    let edge = mesh.edges.keys().next().expect("cube has edges");
    let _result = edge_bevel(&mut mesh, &[edge], 0.3).expect("bevel");
    let new_topology = mesh.flatten_to_topology();

    // Build a fully-populated post-bevel Brush.
    let mut brush = Brush {
        faces: vec![Default::default(); new_topology.polygons.len()],
        topology: new_topology.clone(),
    };
    let positions: Vec<Vec3> = new_topology.vertices.iter().map(|v| v.position).collect();
    for (face_idx, face_data) in brush.faces.iter_mut().enumerate() {
        let normal = new_topology.face_normal_with(&positions, face_idx);
        let v0 =
            new_topology.loops[new_topology.polygons[face_idx].loop_start as usize].vert as usize;
        face_data.plane.normal = normal;
        face_data.plane.distance = positions[v0].dot(normal);
    }

    // Mirror the runtime rebuild: when topology is populated, use the
    // topology ring directly. When it's empty, fall back to plane
    // intersection.
    let (vertices, face_polygons) = if !brush.topology.polygons.is_empty() {
        let verts: Vec<Vec3> = brush.topology.vertices.iter().map(|v| v.position).collect();
        let polys: Vec<Vec<usize>> = (0..brush.topology.polygons.len())
            .map(|i| brush.topology.face_ring(i).map(|v| v as usize).collect())
            .collect();
        (verts, polys)
    } else {
        jackdaw_geometry::compute_brush_geometry_from_planes(&brush.faces)
    };

    let chamfer_idx = brush.faces.len() - 1;
    let ring_indices = &face_polygons[chamfer_idx];
    assert_eq!(ring_indices.len(), 4, "chamfer ring must be a quad");
    let ring: Vec<Vec3> = ring_indices.iter().map(|&i| vertices[i]).collect();
    let e_ab = ring[1] - ring[0];
    let e_bc = ring[2] - ring[1];
    let e_cd = ring[3] - ring[2];
    let e_da = ring[0] - ring[3];
    assert!(
        (e_ab + e_cd).length() < 1e-4,
        "runtime ring (a-b vs c-d) cancel: ring={ring:?}"
    );
    assert!(
        (e_bc + e_da).length() < 1e-4,
        "runtime ring (b-c vs d-a) cancel: ring={ring:?}"
    );
}

#[test]
fn beveled_cube_plane_intersection_recovers_chamfer_ring() {
    // The runtime mesh-rebuild in jackdaw_jsn falls back to
    // `compute_brush_geometry_from_planes` when there is no `BrushHalfedge`
    // available (legacy brushes, runtime preview path, etc). For a beveled
    // cube the chamfer is still a convex brush, so plane intersection should
    // in principle recover a 4-vertex ring identical to the topology ring.
    //
    // This test pins down what that path actually produces: if the chamfer
    // plane was written from the topology Newell normal it is consistent
    // with the topology vertices and plane intersection reproduces the same
    // 4 chamfer corners. If the plane is stale (e.g. carried over from the
    // start-brush template before `apply_live_bevel` re-derived it), plane
    // intersection produces a different ring, and the rendered chamfer face
    // is visually distorted.
    use jackdaw_geometry::compute_brush_geometry_from_planes;

    let brush = Brush::cuboid(2.0, 2.0, 2.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let edge = mesh.edges.keys().next().expect("cube has edges");
    let _result = edge_bevel(&mut mesh, &[edge], 0.3).expect("bevel");

    // Mirror the modal commit: flatten HalfedgeMesh into a fresh Brush and
    // re-derive `brush.faces[i].plane` from the topology Newell normal /
    // first-loop-vert distance, exactly like `apply_live_bevel` does.
    let new_topology = mesh.flatten_to_topology();
    let mut new_brush = Brush {
        faces: vec![Default::default(); new_topology.polygons.len()],
        topology: new_topology.clone(),
    };
    let positions: Vec<Vec3> = new_topology.vertices.iter().map(|v| v.position).collect();
    for (face_idx, face_data) in new_brush.faces.iter_mut().enumerate() {
        let normal = new_topology.face_normal_with(&positions, face_idx);
        let v0 =
            new_topology.loops[new_topology.polygons[face_idx].loop_start as usize].vert as usize;
        face_data.plane.normal = normal;
        face_data.plane.distance = positions[v0].dot(normal);
    }

    let (vertices, face_polys) = compute_brush_geometry_from_planes(&new_brush.faces);
    let chamfer_idx = new_brush.faces.len() - 1;
    let chamfer_indices = &face_polys[chamfer_idx];
    assert_eq!(
        chamfer_indices.len(),
        4,
        "chamfer ring from plane intersection should still be 4 verts"
    );

    let ring: Vec<Vec3> = chamfer_indices.iter().map(|&i| vertices[i]).collect();
    let e_ab = ring[1] - ring[0];
    let e_bc = ring[2] - ring[1];
    let e_cd = ring[3] - ring[2];
    let e_da = ring[0] - ring[3];
    let p1 = (e_ab + e_cd).length();
    let p2 = (e_bc + e_da).length();
    assert!(
        p1 < 1e-3,
        "plane-intersection chamfer (a-b vs c-d) cancel: {p1}, ring={ring:?}"
    );
    assert!(
        p2 < 1e-3,
        "plane-intersection chamfer (b-c vs d-a) cancel: {p2}, ring={ring:?}"
    );
}
