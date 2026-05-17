//! Integration test for the interactive C-key box cutter migrated to
//! `jackdaw_csg::brush_difference_split`.
//!
//! The cutter is a thin slab (the same shape the drawn-cuboid path
//! produces from `build_cutter_planes` in `src/draw_brush.rs`). The
//! target is a cube with one beveled edge. The convex-only
//! `subtract_brush` path bails on this concave target; mesh-CSG should
//! split the cube into the expected fragments.

use bevy::math::Vec3;
use jackdaw_csg::{CsgInput, brush_difference_split};
use jackdaw_geometry::halfedge::{HalfedgeMesh, ops::edge_bevel::edge_bevel};
use jackdaw_jsn::Brush;

/// Bevel one edge of a cube to produce a concave-topology brush, then
/// sync `Brush.faces` to the new polygon count, mirroring what
/// `src/brush/topology_ops/edge_bevel.rs` does after the modal op
/// commits.
fn beveled_cube(half: f32, width: f32) -> Brush {
    let brush = Brush::cuboid(half, half, half);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let edge = mesh
        .edges
        .keys()
        .next()
        .expect("cube has at least one edge");
    edge_bevel(&mut mesh, &[edge], width).expect("bevel one cube edge");
    mesh.validate()
        .expect("halfedge invariants hold after bevel");
    let topology = mesh.flatten_to_topology();

    let mut faces = brush.faces;
    let template = faces.last().cloned().unwrap_or_default();
    while faces.len() < topology.polygons.len() {
        let mut t = template.clone();
        t.uv_u_axis = Vec3::ZERO;
        t.uv_v_axis = Vec3::ZERO;
        faces.push(t);
    }
    for (face_idx, face) in faces.iter_mut().enumerate() {
        if face_idx >= topology.polygons.len() {
            continue;
        }
        let plane = topology.face_plane(face_idx);
        face.plane = plane;
        face.ensure_uv_axes();
    }
    Brush { faces, topology }
}

/// Build a slab cutter aligned to the world axes. Equivalent in shape
/// to what `build_cutter_planes` produces from a flat drawn rectangle
/// with extrude depth `depth` along the +Y axis.
fn box_cutter(center: Vec3, half_x: f32, half_z: f32, depth: f32) -> Brush {
    let mut brush = Brush::cuboid(half_x, depth.abs() / 2.0, half_z);
    for v in &mut brush.topology.vertices {
        v.position += center;
    }
    for f in &mut brush.faces {
        f.plane.distance += f.plane.normal.dot(center);
    }
    brush
}

#[test]
fn box_cutter_against_concave_target_works() {
    // Concave target: cube with one chamfered edge.
    let target = beveled_cube(1.0, 0.3);
    assert!(
        target.topology.polygons.len() >= 7,
        "expected >= 7 polygons after bevel, got {}",
        target.topology.polygons.len()
    );

    // Vertical slab cutter that slices the cube straight through along
    // the X=0 plane. Choose a wide thin Y-aligned slab centered on the
    // origin so it overlaps the whole cube.
    let cutter = box_cutter(Vec3::ZERO, 0.1, 2.0, 4.0);

    let target_input = CsgInput::new(&target.faces, &target.topology);
    let cutter_input = CsgInput::new(&cutter.faces, &cutter.topology);
    let result = brush_difference_split(&target_input, &cutter_input)
        .expect("box cutter against concave target should not error");

    // The slab splits the cube into exactly 2 pieces.
    assert_eq!(
        result.len(),
        2,
        "expected 2 fragments after splitting cube with vertical slab, got {}",
        result.len()
    );
    for piece in &result {
        assert!(
            piece.topology.vertices.len() >= 4,
            "fragment should have >= 4 verts, got {}",
            piece.topology.vertices.len()
        );
        assert!(
            piece.faces.len() >= 4,
            "fragment should have >= 4 faces, got {}",
            piece.faces.len()
        );
        // Parallel-array invariant: every face slot has a polygon ring.
        assert_eq!(
            piece.faces.len(),
            piece.topology.polygons.len(),
            "faces/polygons must be in lockstep on each fragment"
        );
        for poly in &piece.topology.polygons {
            assert!(poly.loop_total >= 3 || poly.loop_total == 0);
        }
    }
}

#[test]
fn box_cutter_misses_concave_target_returns_original() {
    // Cutter that does not intersect the target. Should yield exactly
    // one fragment whose vertex count matches the input.
    let target = beveled_cube(1.0, 0.3);
    let target_vert_count = target.topology.vertices.len();
    let cutter = box_cutter(Vec3::new(100.0, 0.0, 0.0), 0.5, 0.5, 1.0);

    let target_input = CsgInput::new(&target.faces, &target.topology);
    let cutter_input = CsgInput::new(&cutter.faces, &cutter.topology);
    let result = brush_difference_split(&target_input, &cutter_input)
        .expect("disjoint cutter should succeed");
    assert_eq!(result.len(), 1, "expected 1 fragment, got {}", result.len());
    assert_eq!(
        result[0].topology.vertices.len(),
        target_vert_count,
        "disjoint cut should preserve the input vertex count"
    );
}
