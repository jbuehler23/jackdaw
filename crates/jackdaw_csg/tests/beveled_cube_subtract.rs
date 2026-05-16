//! Integration test for the mesh-CSG glue: bevel one edge of a cube
//! (producing a chamfered brush), then subtract another cube from it
//! via `brush_difference_split`. Mirrors the operator path the editor
//! runs, verifies the round-trip survives without panic.

use bevy::math::Vec3;
use jackdaw_csg::{BooleanOp, CsgInput, brush_boolean, brush_difference_split};
use jackdaw_geometry::editmesh::{EditMesh, ops::edge_bevel::edge_bevel};
use jackdaw_jsn::{Brush, BrushFaceData};

/// Helper: bevel one edge of a cube to produce a chamfered brush, then
/// grow `Brush.faces` so every polygon ring has a matching face slot
/// (mirroring what the editor's modal-bevel op does after a successful
/// bevel).
fn beveled_cube(half: f32, width: f32) -> Brush {
    let brush = Brush::cuboid(half, half, half);
    let mut bmesh = EditMesh::lift_from_topology(&brush.topology);
    let edge = bmesh
        .edges
        .keys()
        .next()
        .expect("cube has at least one edge");
    edge_bevel(&mut bmesh, &[edge], width).expect("bevel one cube edge");
    bmesh
        .validate()
        .expect("editmesh invariants hold after bevel");
    let topology = bmesh.flatten_to_topology();

    // Grow faces vec to match the new polygon count and rederive each
    // face plane from its (now-current) ring. This is the same shape of
    // post-op brush sync that `src/brush/topology_ops/edge_bevel.rs`
    // performs after a successful bevel commits.
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

fn translated_cube(half: f32, offset: Vec3) -> Brush {
    let mut brush = Brush::cuboid(half, half, half);
    for v in &mut brush.topology.vertices {
        v.position += offset;
    }
    for f in &mut brush.faces {
        f.plane.distance += f.plane.normal.dot(offset);
    }
    brush
}

#[test]
fn beveled_cube_minus_cube_no_panic() {
    // Build a concave-topology brush (chamfered on one edge).
    let bevel = beveled_cube(1.0, 0.2);
    assert!(
        bevel.topology.polygons.len() >= 7,
        "expected >= 7 polygons after bevel, got {}",
        bevel.topology.polygons.len()
    );
    assert_eq!(
        bevel.faces.len(),
        bevel.topology.polygons.len(),
        "faces/polygons must be in lockstep after bevel"
    );

    // Subtract a small cube nestled into a corner of the cube.
    let cutter = translated_cube(0.3, Vec3::new(0.6, 0.6, -0.6));
    let cutter_input = CsgInput::new(&cutter.faces, &cutter.topology);
    let bevel_input = CsgInput::new(&bevel.faces, &bevel.topology);

    // Run the CSG subtract via the brush operator's underlying kernel path.
    let result = brush_difference_split(&bevel_input, &cutter_input)
        .expect("subtract should not error on concave target");

    assert!(!result.is_empty(), "expected at least one fragment");
    for piece in &result {
        assert!(
            piece.topology.vertices.len() >= 4,
            "fragment should have >= 4 vertices, got {}",
            piece.topology.vertices.len()
        );
        assert!(
            piece.faces.len() >= 4,
            "fragment should have >= 4 faces, got {}",
            piece.faces.len()
        );
        for poly in &piece.topology.polygons {
            // Either a valid ring (>= 3 verts) or an explicitly-empty
            // poly used to preserve the parallel-array invariant.
            assert!(poly.loop_total >= 3 || poly.loop_total == 0);
        }
    }
}

#[test]
fn beveled_cube_union_with_cube_is_solid() {
    // Union of a beveled cube with a translated cube. The result should
    // be a single solid containing both, with a valid mesh.
    let bevel = beveled_cube(1.0, 0.2);
    let other = translated_cube(0.5, Vec3::new(1.5, 0.0, 0.0));
    let lhs = CsgInput::new(&bevel.faces, &bevel.topology);
    let rhs = CsgInput::new(&other.faces, &other.topology);
    let result = brush_boolean(&lhs, &rhs, BooleanOp::Union).expect("union should not error");
    assert!(result.topology.vertices.len() >= 8);
    assert!(result.faces.len() >= 4);
}

#[test]
fn beveled_cube_minus_disjoint_cube_returns_original() {
    // Subtracting a far-away cube should leave the bevel essentially
    // unchanged. We check that the fragment count is 1 and the vertex
    // count matches the input's vertex count exactly.
    let bevel = beveled_cube(1.0, 0.2);
    let bevel_vert_count = bevel.topology.vertices.len();
    let cutter = translated_cube(0.5, Vec3::new(100.0, 0.0, 0.0));
    let bevel_input = CsgInput::new(&bevel.faces, &bevel.topology);
    let cutter_input = CsgInput::new(&cutter.faces, &cutter.topology);
    let result = brush_difference_split(&bevel_input, &cutter_input)
        .expect("disjoint subtract should succeed");
    assert_eq!(result.len(), 1, "disjoint subtract should yield 1 fragment");
    assert_eq!(
        result[0].topology.vertices.len(),
        bevel_vert_count,
        "disjoint subtract should preserve vertex count"
    );
}

#[test]
fn material_handles_propagate_across_bevel_subtract() {
    // Sentinel uv_scale on the original +X face of the cube should
    // survive both the bevel and the subtract on the +X plane.
    let mut bevel = beveled_cube(1.0, 0.2);
    bevel.faces[0].uv_scale = bevy::math::Vec2::new(11.0, 17.0);
    // Subtract a cube whose +X face overlaps but doesn't cover the
    // bevel's +X plane. The +X face of bevel should still exist on the
    // result with the sentinel uv_scale.
    let cutter = translated_cube(0.3, Vec3::new(0.0, 0.6, 0.0));
    let bevel_input = CsgInput::new(&bevel.faces, &bevel.topology);
    let cutter_input = CsgInput::new(&cutter.faces, &cutter.topology);
    let result =
        brush_difference_split(&bevel_input, &cutter_input).expect("subtract should succeed");
    // Find a fragment that has an +X face at distance 1.0.
    let found = result.iter().find_map(|piece| {
        piece.faces.iter().find(|f: &&BrushFaceData| {
            (f.plane.normal - Vec3::X).length() < 1e-3 && (f.plane.distance - 1.0).abs() < 1e-3
        })
    });
    let plus_x = found.expect("+X face should survive on at least one fragment");
    assert!(
        (plus_x.uv_scale - bevy::math::Vec2::new(11.0, 17.0)).length() < 1e-3,
        "sentinel uv_scale should propagate; got {:?}",
        plus_x.uv_scale
    );
}
