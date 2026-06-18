//! Ray / plane intersection.

use glam::Vec3;

/// Intersect a ray with a plane defined by a point on it and its normal.
/// Returns the forward intersection point (ray parameter `t >= 0`), or `None`
/// if the ray is parallel to the plane or the intersection lies behind the
/// origin. `dir` need not be normalized.
pub fn ray_plane_intersection(
    origin: Vec3,
    dir: Vec3,
    plane_point: Vec3,
    plane_normal: Vec3,
) -> Option<Vec3> {
    let denom = dir.dot(plane_normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (plane_point - origin).dot(plane_normal) / denom;
    if t < 0.0 {
        return None;
    }
    Some(origin + dir * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hits_plane_in_front() {
        // A ray from the origin pointing -Z hits the z = -2 plane at (0, 0, -2).
        let hit =
            ray_plane_intersection(Vec3::ZERO, Vec3::NEG_Z, Vec3::new(0.0, 0.0, -2.0), Vec3::Z);
        let p = hit.expect("a forward ray hits the plane");
        assert!((p - Vec3::new(0.0, 0.0, -2.0)).length() < 1e-6);
    }

    #[test]
    fn parallel_ray_misses() {
        // A +X ray is parallel to the z = 0 plane (normal +Z).
        let hit = ray_plane_intersection(Vec3::new(0.0, 0.0, 1.0), Vec3::X, Vec3::ZERO, Vec3::Z);
        assert!(hit.is_none());
    }

    #[test]
    fn plane_behind_origin_misses() {
        // The z = +2 plane lies behind a ray pointing -Z.
        let hit =
            ray_plane_intersection(Vec3::ZERO, Vec3::NEG_Z, Vec3::new(0.0, 0.0, 2.0), Vec3::Z);
        assert!(hit.is_none());
    }
}
