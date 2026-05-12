//! Concave-capable polygon triangulation via earcutr.
//!
//! Projects the 3D ring onto its plane (via `compute_face_tangent_axes`),
//! runs earcut in 2D, returns triangles indexed back into the original ring.

use bevy::math::Vec3;

use crate::compute_face_tangent_axes;

/// Triangulate a polygon defined by a ring of vertex indices in 3D space.
///
/// Projects the 3D vertices onto the plane defined by `normal`, runs earcut
/// in 2D, and returns triangles indexed into the original ring. Handles concave
/// polygons correctly.
///
/// # Arguments
///
/// * `vertices` - All vertices in the polygon mesh
/// * `ring` - Indices into `vertices` defining the polygon's boundary
/// * `normal` - The plane normal (typically from `newell_normal`)
///
/// # Returns
///
/// Vector of triangles, each triangle is `[idx_a, idx_b, idx_c]` where each
/// index refers back to the original `ring` array.
pub fn triangulate_polygon(vertices: &[Vec3], ring: &[u32], normal: Vec3) -> Vec<[u32; 3]> {
    let n = ring.len();
    if n < 3 {
        return Vec::new();
    }
    if n == 3 {
        return vec![[ring[0], ring[1], ring[2]]];
    }

    // Compute 2D coordinate system on the plane
    let (u_axis, v_axis) = compute_face_tangent_axes(normal);

    // Project ring vertices onto 2D plane
    let mut flat: Vec<f64> = Vec::with_capacity(n * 2);
    for &idx in ring {
        let p = vertices[idx as usize];
        flat.push(p.dot(u_axis) as f64);
        flat.push(p.dot(v_axis) as f64);
    }

    // Run earcut triangulation
    let triangles = match earcutr::earcut(&flat, &[], 2) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    // Convert earcut indices back to ring indices
    let mut out = Vec::with_capacity(triangles.len() / 3);
    for chunk in triangles.chunks_exact(3) {
        out.push([ring[chunk[0]], ring[chunk[1]], ring[chunk[2]]]);
    }
    out
}

/// Triangulate a polygon defined directly by vertex positions.
///
/// Convenience function that treats the input as a simple ring (no holes).
/// Indices in the output refer directly to `positions`.
pub fn triangulate_face_polygon(positions: &[Vec3], normal: Vec3) -> Vec<[u32; 3]> {
    let ring: Vec<u32> = (0..positions.len() as u32).collect();
    triangulate_polygon(positions, &ring, normal)
}
