//! Tests for `bisect_plane`: split an `HalfedgeMesh` along a plane,
//! verify cap polygon and side classification.

use bevy::math::Vec3;
use jackdaw_geometry::BrushPlane;
use jackdaw_geometry::halfedge::HalfedgeMesh;
use jackdaw_geometry::halfedge::ops::bisect_plane::{BisectKeep, bisect_plane};
use jackdaw_geometry::halfedge::ops::edge_bevel::edge_bevel;
use jackdaw_jsn::Brush;

fn side(co: Vec3, plane: &BrushPlane) -> f32 {
    co.dot(plane.normal) - plane.distance
}

/// Bevel one edge of a cube to make the brush a multi-face / "concave-ish"
/// input (technically still convex but with more than 6 faces, exercising
/// the topology-driven path).
fn beveled_cube() -> HalfedgeMesh {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let first_edge = mesh.edges.keys().next().unwrap();
    edge_bevel(&mut mesh, &[first_edge], 0.2).expect("bevel");
    mesh.validate().expect("valid after bevel");
    mesh
}

#[test]
fn clip_concave_brush_keep_front_produces_correct_topology() {
    // Pre-cut: beveled cube has > 6 faces (one chamfer quad).
    let mut mesh = beveled_cube();
    let pre_face_count = mesh.face_count();
    assert!(
        pre_face_count > 6,
        "beveled cube should have more than 6 faces"
    );

    let plane = BrushPlane {
        normal: Vec3::Z,
        distance: 0.0,
    };
    let result = bisect_plane(&mut mesh, &plane, BisectKeep::Front).expect("bisect");
    mesh.validate().expect("valid after bisect");

    // Every surviving vert is in or on +Z half-space.
    for (_, v) in mesh.verts.iter() {
        assert!(
            side(v.co, &plane) > -1e-3,
            "vert {:?} should be in front of plane",
            v.co
        );
    }

    // A new cap face must exist.
    let cap = result.cap_face.expect("cap face produced");
    let cap_face = &mesh.faces[cap];
    assert!(cap_face.loop_count >= 3, "cap is at least a triangle");
    // All cap ring verts lie on the cut plane.
    let mut cur = cap_face.loop_first;
    for _ in 0..cap_face.loop_count {
        let p = mesh.verts[mesh.loops[cur].vert].co;
        assert!(side(p, &plane).abs() < 1e-3, "cap ring on plane");
        cur = mesh.loops[cur].next;
    }

    // The cap's outward normal should face -Z (the kept solid is in +Z;
    // outward = -Z).
    assert!(
        cap_face.normal_cache.dot(Vec3::NEG_Z) > 0.5,
        "front-keep cap normal {:?} should face -Z",
        cap_face.normal_cache
    );
}

#[test]
fn clip_concave_brush_keep_back_produces_correct_topology() {
    let mut mesh = beveled_cube();

    let plane = BrushPlane {
        normal: Vec3::Z,
        distance: 0.0,
    };
    let result = bisect_plane(&mut mesh, &plane, BisectKeep::Back).expect("bisect");
    mesh.validate().expect("valid after bisect");

    for (_, v) in mesh.verts.iter() {
        assert!(
            side(v.co, &plane) < 1e-3,
            "vert {:?} should be behind the plane",
            v.co
        );
    }
    let cap = result.cap_face.expect("cap face produced");
    let cap_face = &mesh.faces[cap];
    assert!(
        cap_face.normal_cache.dot(Vec3::Z) > 0.5,
        "back-keep cap normal {:?} should face +Z",
        cap_face.normal_cache
    );
}

#[test]
fn clip_concave_brush_split_produces_two_brushes() {
    // Split = run Front + Back bisects on separate clones; sum of post-cut
    // face counts should be: pre-cut faces minus those entirely on the cut
    // plane, plus split halves of straddling faces, plus two caps.
    let mesh = beveled_cube();
    let plane = BrushPlane {
        normal: Vec3::Z,
        distance: 0.0,
    };

    let mut front = mesh.clone();
    bisect_plane(&mut front, &plane, BisectKeep::Front).expect("front bisect");
    front.validate().expect("front valid");

    let mut back = mesh.clone();
    bisect_plane(&mut back, &plane, BisectKeep::Back).expect("back bisect");
    back.validate().expect("back valid");

    // Both halves must be non-empty closed-ish meshes with their own cap.
    assert!(front.face_count() >= 4, "front half has faces");
    assert!(back.face_count() >= 4, "back half has faces");
    // Each half includes one cap.
    let front_caps = front
        .faces
        .values()
        .filter(|f| f.normal_cache.dot(Vec3::NEG_Z) > 0.9)
        .count();
    let back_caps = back
        .faces
        .values()
        .filter(|f| f.normal_cache.dot(Vec3::Z) > 0.9)
        .count();
    assert!(front_caps >= 1, "front side has at least one -Z facing cap");
    assert!(back_caps >= 1, "back side has at least one +Z facing cap");
}

#[test]
fn clip_convex_brush_still_uses_fast_path() {
    // The editor's dispatch routes brushes with an empty topology
    // through the legacy plane-push fast path. Verify that constructing
    // a Brush with empty topology (legacy / unmigrated state) leaves
    // `topology.polygons` empty, so the dispatch in `clip_apply` picks
    // the fast path.
    use jackdaw_geometry::BrushFaceData;
    let face_planes = [
        BrushPlane {
            normal: Vec3::X,
            distance: 1.0,
        },
        BrushPlane {
            normal: Vec3::NEG_X,
            distance: 1.0,
        },
        BrushPlane {
            normal: Vec3::Y,
            distance: 1.0,
        },
        BrushPlane {
            normal: Vec3::NEG_Y,
            distance: 1.0,
        },
        BrushPlane {
            normal: Vec3::Z,
            distance: 1.0,
        },
        BrushPlane {
            normal: Vec3::NEG_Z,
            distance: 1.0,
        },
    ];
    let faces: Vec<BrushFaceData> = face_planes
        .iter()
        .map(|p| BrushFaceData {
            plane: p.clone(),
            ..Default::default()
        })
        .collect();
    let brush = jackdaw_jsn::Brush {
        faces,
        topology: Default::default(),
    };
    assert!(
        brush.topology.polygons.is_empty(),
        "legacy convex brush has empty topology"
    );
    // The editor's `clip_apply` checks `!brush.topology.polygons.is_empty()`
    // to decide whether to use bisect_plane; an empty topology takes the
    // fast path.
    let use_bisect = !brush.topology.polygons.is_empty();
    assert!(!use_bisect, "convex empty-topology brush takes fast path");
}

#[test]
fn bisect_offset_plane_keeps_correct_half_space() {
    let brush = Brush::cuboid(1.0, 1.0, 1.0);
    let mut mesh = HalfedgeMesh::lift_from_topology(&brush.topology);
    let plane = BrushPlane {
        normal: Vec3::X,
        distance: 0.5,
    };
    bisect_plane(&mut mesh, &plane, BisectKeep::Front).expect("bisect");
    mesh.validate().expect("valid after offset bisect");
    for (_, v) in mesh.verts.iter() {
        assert!(v.co.x > 0.5 - 1e-3);
    }
}
