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

/// Signed distance along `axis_dir` from `axis_origin` to where the ray meets
/// a camera-facing plane that contains the axis. `cam_pos` orients that plane.
/// `axis_dir` need not be normalized. Returns `None` when the axis points at
/// the camera (the plane is edge-on) or the ray misses.
pub fn ray_axis_param(
    ray_origin: Vec3,
    ray_dir: Vec3,
    axis_origin: Vec3,
    axis_dir: Vec3,
    cam_pos: Vec3,
) -> Option<f32> {
    let axis = axis_dir.normalize_or_zero();
    if axis.length_squared() < 1e-12 {
        return None;
    }
    let view = axis_origin - cam_pos;
    let plane_normal = axis.cross(view.cross(axis)).normalize_or_zero();
    if plane_normal.length_squared() < 1e-12 {
        return None;
    }
    let hit = ray_plane_intersection(ray_origin, ray_dir, axis_origin, plane_normal)?;
    Some((hit - axis_origin).dot(axis))
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

    #[test]
    fn axis_param_projects_onto_x() {
        // Camera on +Z looking at the origin. A ray through (2, 3, 0) meets
        // the XY plane and projects to 2 along X, even though it is off-axis.
        let cam = Vec3::new(0.0, 0.0, 10.0);
        let target = Vec3::new(2.0, 3.0, 0.0);
        let t = ray_axis_param(cam, target - cam, Vec3::ZERO, Vec3::X, cam)
            .expect("ray hits the axis plane");
        assert!((t - 2.0).abs() < 1e-4);
    }

    #[test]
    fn axis_param_none_when_axis_points_at_camera() {
        let cam = Vec3::new(0.0, 0.0, 10.0);
        let hit = ray_axis_param(cam, Vec3::NEG_Z, Vec3::ZERO, Vec3::Z, cam);
        assert!(hit.is_none());
    }
}
