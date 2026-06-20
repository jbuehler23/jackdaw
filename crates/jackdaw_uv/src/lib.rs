//! Engine-agnostic UV projection math.
//!
//! Each function computes a face's UV projection from plain geometry: a face
//! normal, the face ring vertices, the current UV axes/scale, a chosen edge, or
//! a texel density. They take and return only [`glam`] vectors and plain
//! numbers, never a face record or any engine type, so the same projection math
//! drives a UV editor on any host. The editor wraps each one in a thin operator
//! that reads a face's geometry, calls the matching function, and writes the
//! result back onto the face.
//!
//! `UvProjection` is the full set of projection parameters as stored on a face:
//! the U and V axes, the scale, the per-axis offset, and the rotation. Functions
//! return only the fields they change (a [`UvAxes`] pair, a scale, an offset, or
//! a combination), leaving the rest for the caller to keep.

use glam::{Vec2, Vec3};

pub use jackdaw_geometry::compute_face_tangent_axes;

/// The U and V tangent axes of a face's UV projection, in world space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UvAxes {
    pub u: Vec3,
    pub v: Vec3,
}

impl UvAxes {
    pub fn new(u: Vec3, v: Vec3) -> Self {
        Self { u, v }
    }
}

/// New axes plus a reset offset and rotation, returned by [`reset_axes`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResetProjection {
    pub axes: UvAxes,
    pub offset: Vec2,
    pub rotation: f32,
}

/// New scale plus the offset that pins the face's minimum corner to the UV
/// origin, returned by [`fit_to_face`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitProjection {
    pub scale: Vec2,
    pub offset: Vec2,
}

/// Recompute the U and V axes from a face normal and reset the offset and
/// rotation. The scale is left to the caller (the reset operator keeps it).
///
/// The axes come from [`compute_face_tangent_axes`], the same paraxial
/// projection used when a face is first created, so a reset returns a face to
/// its default projection orientation.
pub fn reset_axes(normal: Vec3) -> ResetProjection {
    let (u, v) = compute_face_tangent_axes(normal);
    ResetProjection {
        axes: UvAxes::new(u, v),
        offset: Vec2::ZERO,
        rotation: 0.0,
    }
}

/// Rotate the U and V axes ninety degrees counter-clockwise in the face plane:
/// the new U is the old V negated and the new V is the old U. Four applications
/// return the original axes.
pub fn rotate_90(axes: UvAxes) -> UvAxes {
    UvAxes::new(-axes.v, axes.u)
}

/// Fit the scale so the face spans the full 0..1 UV range along each axis: the
/// texture covers the face exactly once. The returned offset shifts the face's
/// minimum corner to the UV origin.
///
/// `ring` is the face's ring vertices in world space and `axes` is the face's
/// current U and V axes; the extent is measured by projecting the ring onto
/// those axes. A degenerate span (a face with no extent along an axis) is
/// clamped to a tiny positive value so the scale stays finite. Returns `None`
/// when the ring is empty.
pub fn fit_to_face(ring: &[Vec3], axes: UvAxes) -> Option<FitProjection> {
    if ring.is_empty() {
        return None;
    }

    let mut min_u = f32::INFINITY;
    let mut max_u = f32::NEG_INFINITY;
    let mut min_v = f32::INFINITY;
    let mut max_v = f32::NEG_INFINITY;
    for p in ring {
        let u = p.dot(axes.u);
        let v = p.dot(axes.v);
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }

    let span_u = (max_u - min_u).max(1e-4);
    let span_v = (max_v - min_v).max(1e-4);
    let scale = Vec2::new(1.0 / span_u, 1.0 / span_v);
    // UV = world.dot(axis) * scale + offset; want (min * scale + offset) = 0.
    let offset = Vec2::new(-min_u * scale.x, -min_v * scale.y);

    Some(FitProjection { scale, offset })
}

/// Snap the U and V axes to the closest world-axis pair for the face's normal.
/// Adjacent faces with the same texture then tile continuously across shared
/// edges, the grid-aligned brushwork convention.
///
/// The dominant component of the normal selects the axis pair, and its sign
/// flips one axis so the winding stays consistent across opposite faces.
pub fn world_aligned(normal: Vec3) -> UvAxes {
    let abs = normal.abs();
    let (u, v) = if abs.x >= abs.y && abs.x >= abs.z {
        // Normal mostly along X: U = +/-Y, V = Z
        if normal.x >= 0.0 {
            (Vec3::Y, Vec3::Z)
        } else {
            (Vec3::NEG_Y, Vec3::Z)
        }
    } else if abs.y >= abs.x && abs.y >= abs.z {
        // Normal mostly along Y: U = +/-X (negated for consistent winding), V = Z
        if normal.y >= 0.0 {
            (Vec3::NEG_X, Vec3::Z)
        } else {
            (Vec3::X, Vec3::Z)
        }
    } else {
        // Normal mostly along Z: U = X, V = +/-Y
        if normal.z >= 0.0 {
            (Vec3::X, Vec3::Y)
        } else {
            (Vec3::X, Vec3::NEG_Y)
        }
    };
    UvAxes::new(u, v)
}

/// Rotate the UV axes so U follows a chosen edge of the face: the texture's
/// grain runs along that feature edge. `normal` is the face normal and
/// `edge_start` / `edge_end` are the world positions of the edge's endpoints.
///
/// The edge direction is projected onto the face plane and used as the new U;
/// V is the normal crossed with U so the pair stays orthonormal and in-plane.
/// Returns `None` when the edge projects to nearly zero in the plane (it is
/// parallel to the normal, so it gives no usable direction), leaving the
/// caller's existing axes unchanged.
pub fn align_to_edge(normal: Vec3, edge_start: Vec3, edge_end: Vec3) -> Option<UvAxes> {
    let edge_dir = edge_end - edge_start;
    let edge_dir_planar = (edge_dir - normal * edge_dir.dot(normal)).normalize_or_zero();
    if edge_dir_planar.length_squared() > 0.5 {
        let u = edge_dir_planar;
        let v = normal.cross(edge_dir_planar).normalize_or_zero();
        Some(UvAxes::new(u, v))
    } else {
        None
    }
}

/// Compute the uniform UV scale for a target texel density: `texels_per_unit`
/// texels of a `texture_pixels`-wide texture mapped onto one world unit. Holding
/// this constant across faces keeps a consistent texture resolution regardless
/// of face size.
pub fn texel_density_scale(texels_per_unit: f32, texture_pixels: f32) -> Vec2 {
    let scale = texels_per_unit / texture_pixels;
    Vec2::new(scale, scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world axis (or its negation) within tolerance.
    fn is_world_axis(v: Vec3) -> bool {
        [Vec3::X, Vec3::Y, Vec3::Z]
            .iter()
            .any(|&a| (v - a).length() < 1e-5 || (v + a).length() < 1e-5)
    }

    #[test]
    fn reset_axes_are_orthonormal_and_in_plane() {
        for normal in [
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(1.0, 2.0, 3.0).normalize(),
        ] {
            let r = reset_axes(normal);
            assert!(
                (r.axes.u.length() - 1.0).abs() < 1e-5,
                "u unit for {normal}"
            );
            assert!(
                (r.axes.v.length() - 1.0).abs() < 1e-5,
                "v unit for {normal}"
            );
            assert!(r.axes.u.dot(r.axes.v).abs() < 1e-5, "u perp v for {normal}");
            assert!(r.axes.u.dot(normal).abs() < 1e-5, "u in plane for {normal}");
            assert!(r.axes.v.dot(normal).abs() < 1e-5, "v in plane for {normal}");
            assert_eq!(r.offset, Vec2::ZERO);
            assert_eq!(r.rotation, 0.0);
        }
    }

    #[test]
    fn rotate_90_maps_u_to_v_and_v_to_neg_u() {
        let axes = UvAxes::new(Vec3::X, Vec3::Y);
        let rotated = rotate_90(axes);
        // New U is the old V negated; new V is the old U.
        assert_eq!(rotated.u, -Vec3::Y);
        assert_eq!(rotated.v, Vec3::X);
    }

    #[test]
    fn rotate_90_four_times_is_identity() {
        let axes = UvAxes::new(Vec3::X, Vec3::Y);
        let mut r = axes;
        for _ in 0..4 {
            r = rotate_90(r);
        }
        assert_eq!(r, axes);
    }

    #[test]
    fn fit_unit_square_yields_unit_scale_and_zero_offset() {
        // A unit square in the XY plane, projected on the X/Y axes, spans
        // exactly 0..1 on each axis, so the fit scale is 1 and offset is 0.
        let ring = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
        ];
        let fit = fit_to_face(&ring, UvAxes::new(Vec3::X, Vec3::Y)).unwrap();
        assert!((fit.scale.x - 1.0).abs() < 1e-5, "scale.x {}", fit.scale.x);
        assert!((fit.scale.y - 1.0).abs() < 1e-5, "scale.y {}", fit.scale.y);
        assert!(fit.offset.x.abs() < 1e-5, "offset.x {}", fit.offset.x);
        assert!(fit.offset.y.abs() < 1e-5, "offset.y {}", fit.offset.y);
    }

    #[test]
    fn fit_offset_anchors_minimum_corner_to_uv_origin() {
        // A 2x4 face offset from the origin: the fit must scale to 1/extent and
        // pin the minimum corner so (min * scale + offset) == 0 on each axis.
        let ring = [
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(4.0, 1.0, 0.0),
            Vec3::new(4.0, 5.0, 0.0),
            Vec3::new(2.0, 5.0, 0.0),
        ];
        let axes = UvAxes::new(Vec3::X, Vec3::Y);
        let fit = fit_to_face(&ring, axes).unwrap();
        assert!((fit.scale.x - 0.5).abs() < 1e-5, "scale.x {}", fit.scale.x);
        assert!((fit.scale.y - 0.25).abs() < 1e-5, "scale.y {}", fit.scale.y);
        // Minimum corner (2, 1) maps to UV (0, 0).
        let uv_min_u = 2.0 * fit.scale.x + fit.offset.x;
        let uv_min_v = 1.0 * fit.scale.y + fit.offset.y;
        assert!(uv_min_u.abs() < 1e-5, "min u maps to 0: {uv_min_u}");
        assert!(uv_min_v.abs() < 1e-5, "min v maps to 0: {uv_min_v}");
        // Maximum corner (4, 5) maps to UV (1, 1).
        let uv_max_u = 4.0 * fit.scale.x + fit.offset.x;
        let uv_max_v = 5.0 * fit.scale.y + fit.offset.y;
        assert!((uv_max_u - 1.0).abs() < 1e-5, "max u maps to 1: {uv_max_u}");
        assert!((uv_max_v - 1.0).abs() < 1e-5, "max v maps to 1: {uv_max_v}");
    }

    #[test]
    fn fit_empty_ring_returns_none() {
        assert_eq!(fit_to_face(&[], UvAxes::new(Vec3::X, Vec3::Y)), None);
    }

    #[test]
    fn world_align_snaps_axes_to_world_axes() {
        // A tilted normal still snaps both axes onto world axes.
        for normal in [
            Vec3::X,
            Vec3::NEG_Y,
            Vec3::Z,
            Vec3::new(0.9, 0.3, 0.1).normalize(),
            Vec3::new(-0.1, -0.8, 0.2).normalize(),
        ] {
            let axes = world_aligned(normal);
            assert!(is_world_axis(axes.u), "u {} not a world axis", axes.u);
            assert!(is_world_axis(axes.v), "v {} not a world axis", axes.v);
            assert!(axes.u.dot(axes.v).abs() < 1e-5, "axes perpendicular");
        }
    }

    #[test]
    fn world_align_x_face_uses_y_and_z() {
        let axes = world_aligned(Vec3::X);
        assert_eq!(axes.u, Vec3::Y);
        assert_eq!(axes.v, Vec3::Z);
    }

    #[test]
    fn align_to_edge_sets_u_along_the_edge() {
        // A face in the XY plane (normal +Z); an edge running along +X gives
        // U = +X and V = +Y (normal x U).
        let normal = Vec3::Z;
        let axes = align_to_edge(normal, Vec3::ZERO, Vec3::new(3.0, 0.0, 0.0)).unwrap();
        assert!(
            (axes.u - Vec3::X).length() < 1e-5,
            "u along edge: {}",
            axes.u
        );
        assert!(
            (axes.v - Vec3::Y).length() < 1e-5,
            "v perpendicular: {}",
            axes.v
        );
    }

    #[test]
    fn align_to_edge_parallel_to_normal_returns_none() {
        // An edge running along the normal has no in-plane direction.
        let normal = Vec3::Z;
        assert_eq!(
            align_to_edge(normal, Vec3::ZERO, Vec3::new(0.0, 0.0, 2.0)),
            None
        );
    }

    #[test]
    fn texel_density_scale_is_uniform_ratio() {
        // 64 texels per unit on a 1024 px texture is a uniform 1/16 scale.
        let scale = texel_density_scale(64.0, 1024.0);
        assert!((scale.x - 0.0625).abs() < 1e-6);
        assert_eq!(scale.x, scale.y, "scale is uniform across both axes");
    }
}
