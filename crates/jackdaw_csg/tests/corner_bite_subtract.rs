//! Regression: subtracting a cutter from a corner of a brush should
//! leave a single connected L-shape, not multiple fragments.
//!
//! The user-facing bug ("concave angles seeping through the cut plane,
//! removal deleting more than expected") was traced to Manifold's
//! bounds-relative coincidence epsilon being tighter than the f32-to-f64
//! plane noise present in editor-saved brushes. The fix lives in
//! `crates/jackdaw_csg/src/lib.rs::brush_to_manifold` where we now call
//! `set_tolerance(MANIFOLD_TOLERANCE)` on every input manifold.
//!
//! This test mimics the geometry in `assets/pre_cut.jsn`: brush-shaped
//! sizes (a few units across), at non-trivial world translations
//! (so the manifold's automatic bbox-tolerance is computed in the same
//! ballpark as in the editor), with the cutter overlapping a single
//! corner of the target. The expected result is one solid L-shape.

use bevy::math::Vec3;
use jackdaw_csg::{CsgInput, brush_difference_split, brush_to_world};
use jackdaw_jsn::Brush;

/// Translated, rotated cuboid as the editor would produce after a draw
/// op: face planes and topology in world space.
fn world_cuboid(half_x: f32, half_y: f32, half_z: f32, center: Vec3) -> Brush {
    let brush = Brush::cuboid(half_x, half_y, half_z);
    let (world_faces, world_topo) = brush_to_world(
        &brush.faces,
        &brush.topology,
        bevy::math::Quat::IDENTITY,
        center,
    );
    Brush {
        faces: world_faces,
        topology: world_topo,
    }
}

#[test]
fn corner_bite_subtract_yields_single_connected_lshape() {
    // Target: roughly the size of the .jsn brush 2 (a 3.25 x 1.5 x 3.25
    // block). World-axis-aligned, sitting at the origin.
    let target = world_cuboid(1.625, 0.75, 1.625, Vec3::new(0.0, 0.75, 0.0));

    // Cutter: a smaller box positioned so it overlaps only one corner of
    // the target. Corners of the target sit at (+/-1.625, 0 or 1.5,
    // +/-1.625). We bite the (+x, +y, +z) corner: a 1.0 x 1.0 x 1.0
    // cutter centered just inside that corner so it removes a unit cube
    // from the corner.
    let cutter = world_cuboid(0.5, 0.5, 0.5, Vec3::new(1.125, 1.0, 1.125));

    let target_input = CsgInput::new(&target.faces, &target.topology);
    let cutter_input = CsgInput::new(&cutter.faces, &cutter.topology);

    let result = brush_difference_split(&target_input, &cutter_input)
        .expect("corner subtract should not error");

    // A corner-bite leaves a single connected L-shape. If the kernel
    // produces multiple fragments here, the tolerance / coincidence
    // epsilon was too tight and a sliver got split off.
    assert_eq!(
        result.len(),
        1,
        "corner-bite of a cube should yield 1 connected fragment, got {}",
        result.len()
    );

    let lshape = &result[0];

    // A cube minus a corner bite has 9 faces (the 6 original cube faces,
    // each clipped on one side, plus 3 new faces from the cutter sides
    // that face into the bite). If we see many more (say > 16), the
    // boundary walker is producing sliver polygons; the user-visible
    // "concave angle seeping through" maps to extra fragments here.
    assert!(
        lshape.faces.len() <= 16,
        "L-shape should have <= 16 faces (cube minus corner = 9), got {}",
        lshape.faces.len()
    );
    assert!(
        lshape.faces.len() >= 7,
        "L-shape should have >= 7 faces, got {}",
        lshape.faces.len()
    );

    // Vertex count: a cube minus a corner bite has 10 vertices
    // (8 original - 1 removed + 3 new = 10). Allow some slack for
    // boundary doubles.
    assert!(
        lshape.topology.vertices.len() >= 8 && lshape.topology.vertices.len() <= 20,
        "L-shape vertex count should be in [8, 20], got {}",
        lshape.topology.vertices.len()
    );

    // Every face must have a valid ring.
    for poly in &lshape.topology.polygons {
        assert!(
            poly.loop_total >= 3 || poly.loop_total == 0,
            "polygon ring should be valid, got loop_total = {}",
            poly.loop_total
        );
    }
}

#[test]
fn corner_bite_subtract_with_editor_scale_brushes() {
    // Same logic as above but at the world-translation scale seen in
    // `assets/pre_cut.jsn`, where target sits at (4.7, 1.8, 1.5) etc.
    // Larger translations push the kernel's bounds-relative epsilon
    // higher, so without the explicit set_tolerance call the bug shows
    // up more clearly here.
    let target = world_cuboid(1.625, 0.75, 1.625, Vec3::new(4.714, 1.786, 1.536));
    let cutter = world_cuboid(0.5, 0.5, 0.5, Vec3::new(5.839, 2.036, 2.661));

    let target_input = CsgInput::new(&target.faces, &target.topology);
    let cutter_input = CsgInput::new(&cutter.faces, &cutter.topology);

    let result = brush_difference_split(&target_input, &cutter_input)
        .expect("translated corner subtract should not error");

    assert_eq!(
        result.len(),
        1,
        "translated corner-bite should yield 1 fragment, got {}",
        result.len()
    );

    let lshape = &result[0];
    assert!(
        lshape.faces.len() >= 7 && lshape.faces.len() <= 16,
        "translated L-shape face count out of range: {}",
        lshape.faces.len()
    );
}

/// Reproduce the f32-noise on **vertex positions** seen in editor-saved
/// brushes after a round-trip through JSN serialization. The .jsn dump
/// in `assets/pre_cut.jsn` shows two vertices that should lie on the
/// same Z=-2.5357... plane but actually differ by ~2e-7 in z. Without an
/// explicit Manifold tolerance, that f32-jitter at the cut boundary
/// produces sliver triangles which the boundary walker then groups into
/// extra "faces" (the user-visible "concave angles seeping through").
fn jittered_world_cuboid(half_x: f32, half_y: f32, half_z: f32, center: Vec3) -> Brush {
    let mut b = world_cuboid(half_x, half_y, half_z, center);
    // Jitter every vertex by ~5e-7 in a deterministic direction. This
    // mimics what happens to a brush whose positions round-trip through
    // f32 serialization (the .jsn dump shows exactly this signature).
    for (i, v) in b.topology.vertices.iter_mut().enumerate() {
        let s = ((i % 4) as f32 - 1.5) * 1.5e-7;
        v.position += Vec3::splat(s);
    }
    // Also nudge the planes so they don't perfectly bound the jittered
    // vertices, closer to the editor's saved state.
    let noise = 0.9999999403953552_f32;
    for f in &mut b.faces {
        let n = f.plane.normal;
        if n == Vec3::Z {
            f.plane.normal = Vec3::new(0.0, 0.0, noise);
        } else if n == Vec3::NEG_Z {
            f.plane.normal = Vec3::new(0.0, 0.0, -noise);
        } else if n == Vec3::Y {
            f.plane.normal = Vec3::new(0.0, noise, 0.0);
        }
    }
    b
}

#[test]
fn corner_bite_with_vertex_jitter_yields_single_fragment() {
    // Simulate brush state after .jsn round-trip: vertex positions
    // jittered by f32 epsilon, plane normals barely off unit.
    let target = jittered_world_cuboid(1.625, 0.75, 1.625, Vec3::new(4.714, 1.786, 1.536));
    let cutter = jittered_world_cuboid(0.5, 0.5, 0.5, Vec3::new(5.839, 2.036, 2.661));

    let target_input = CsgInput::new(&target.faces, &target.topology);
    let cutter_input = CsgInput::new(&cutter.faces, &cutter.topology);

    let result = brush_difference_split(&target_input, &cutter_input)
        .expect("jittered corner subtract should not error");

    assert_eq!(
        result.len(),
        1,
        "jittered corner-bite should still yield 1 connected fragment, got {}",
        result.len()
    );
}

#[test]
fn chained_corner_bite_stays_single_fragment() {
    // Cut a corner off, then cut another corner off the result. The
    // intermediate brush has been rebuilt through the manifold ->
    // boundary -> topology pipeline, so its vertices and planes carry
    // both Manifold's bbox-relative epsilon and the ring-walker's
    // dedup epsilon. A second cut into it is the case most likely to
    // produce sliver fragments if the tolerance isn't set.
    let target = world_cuboid(2.0, 2.0, 2.0, Vec3::ZERO);
    let first_cutter = world_cuboid(0.7, 0.7, 0.7, Vec3::new(1.5, 1.5, 1.5));
    let result1 = brush_difference_split(
        &CsgInput::new(&target.faces, &target.topology),
        &CsgInput::new(&first_cutter.faces, &first_cutter.topology),
    )
    .expect("first subtract should succeed");
    assert_eq!(result1.len(), 1, "first cut should yield 1 fragment");

    let intermediate = &result1[0];
    let second_cutter = world_cuboid(0.7, 0.7, 0.7, Vec3::new(-1.5, -1.5, -1.5));
    let result2 = brush_difference_split(
        &CsgInput::new(&intermediate.faces, &intermediate.topology),
        &CsgInput::new(&second_cutter.faces, &second_cutter.topology),
    )
    .expect("second subtract should succeed");
    assert_eq!(
        result2.len(),
        1,
        "second cut should yield 1 fragment, got {}",
        result2.len()
    );

    let lshape = &result2[0];
    // A cube with TWO non-adjacent corner bites should have a bounded
    // face count. If the boundary walker produces sliver polygons, this
    // explodes.
    assert!(
        lshape.faces.len() <= 24,
        "double-bite face count out of range: {}",
        lshape.faces.len()
    );
}

#[test]
fn coplanar_face_cut_yields_clean_lshape() {
    // Cutter whose -X face is **coplanar** with the target's +X face,
    // mid-height bite. This is a classic precision trap: the kernel
    // has to decide whether the coincident faces are merged or not.
    // Without a deliberate tolerance, Manifold's bbox-relative epsilon
    // can either fail to merge them (leaving a zero-width sliver face)
    // or merge them too aggressively (deleting more geometry than
    // expected, the user's reported bug).
    let target = world_cuboid(1.0, 1.0, 1.0, Vec3::ZERO);
    // Cutter has half-width 0.5 in X, centered at (1.5, 0.0, 0.0) so its
    // -X plane is at x=1.0, exactly the target's +X plane.
    let cutter = world_cuboid(0.5, 0.6, 1.5, Vec3::new(1.5, 0.0, 0.0));

    let result = brush_difference_split(
        &CsgInput::new(&target.faces, &target.topology),
        &CsgInput::new(&cutter.faces, &cutter.topology),
    )
    .expect("coplanar bite should succeed");

    assert_eq!(
        result.len(),
        1,
        "coplanar face cut should yield 1 fragment, got {}",
        result.len()
    );

    let lshape = &result[0];
    assert!(
        lshape.faces.len() <= 12,
        "coplanar-cut face count out of range: {}",
        lshape.faces.len()
    );
}

#[test]
fn corner_bite_face_planes_align_to_input_planes() {
    // After a corner bite, every output face should sit on one of the
    // input planes (target's 6 or cutter's 6 faces). If we see faces
    // tilted by f32 noise (normal off by more than 1e-3 from any input
    // plane), it means the boundary walker / tris-to-quads path is
    // generating spurious faces from sliver triangles.
    let target = world_cuboid(1.625, 0.75, 1.625, Vec3::new(0.0, 0.75, 0.0));
    let cutter = world_cuboid(0.5, 0.5, 0.5, Vec3::new(1.125, 1.0, 1.125));

    let target_input = CsgInput::new(&target.faces, &target.topology);
    let cutter_input = CsgInput::new(&cutter.faces, &cutter.topology);
    let result = brush_difference_split(&target_input, &cutter_input).unwrap();

    assert_eq!(result.len(), 1);
    let lshape = &result[0];

    let mut input_planes: Vec<Vec3> = target.faces.iter().map(|f| f.plane.normal).collect();
    input_planes.extend(cutter.faces.iter().map(|f| f.plane.normal));

    for face in &lshape.faces {
        let best = input_planes
            .iter()
            .map(|n| n.dot(face.plane.normal).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            best > 1.0 - 1e-2,
            "output face normal {:?} doesn't match any input plane (best dot = {})",
            face.plane.normal,
            best
        );
    }
}
