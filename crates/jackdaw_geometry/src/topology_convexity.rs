//! Helper to test whether a brush's topology represents a convex polyhedron.
//! Tests whether every vertex lies on or inside every face's plane (within EPSILON).
//! O(verts * faces). For typical brush sizes this is sub-millisecond.

use bevy::math::Vec3;

use crate::topology::BrushTopology;

pub fn is_convex_topology(topology: &BrushTopology) -> bool {
    if topology.polygons.is_empty() || topology.vertices.len() < 4 {
        return true; // degenerate; treat as convex
    }
    let positions: Vec<Vec3> = topology.vertices.iter().map(|v| v.position).collect();
    let epsilon = 1e-4_f32;
    for face_idx in 0..topology.polygons.len() {
        let normal = topology.face_normal_with(&positions, face_idx);
        if normal.length_squared() < 0.5 {
            continue; // degenerate face
        }
        let loop_start = topology.polygons[face_idx].loop_start as usize;
        let v0_idx = topology.loops[loop_start].vert as usize;
        let plane_d = positions[v0_idx].dot(normal);
        for &p in &positions {
            if p.dot(normal) > plane_d + epsilon {
                return false; // a vertex lies OUTSIDE this face's plane
            }
        }
    }
    true
}
