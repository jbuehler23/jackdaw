use crate::default_style;
use crate::draw_brush::{DrawBrushGizmoGroup, DrawPlane};
use crate::snapping::SnapSettings;
use bevy::prelude::*;
use jackdaw_scene_types::BrushTopology;

/// AABB overlap test on topology vertices. Replaces the convex-only plane
/// `brushes_intersect` for paths where either brush may be concave; the
/// half-space separating-plane test isn't sound for concave brushes because a
/// face plane can pass through the brush's own interior.
pub(crate) fn topology_aabbs_overlap(a: &BrushTopology, b: &BrushTopology) -> bool {
    if a.vertices.is_empty() || b.vertices.is_empty() {
        return false;
    }
    let mut a_min = Vec3::MAX;
    let mut a_max = Vec3::MIN;
    for v in &a.vertices {
        a_min = a_min.min(v.position);
        a_max = a_max.max(v.position);
    }
    let mut b_min = Vec3::MAX;
    let mut b_max = Vec3::MIN;
    for v in &b.vertices {
        b_min = b_min.min(v.position);
        b_max = b_max.max(v.position);
    }
    const E: f32 = 1e-4;
    a_min.x <= b_max.x + E
        && a_max.x >= b_min.x - E
        && a_min.y <= b_max.y + E
        && a_max.y >= b_min.y - E
        && a_min.z <= b_max.z + E
        && a_max.z >= b_min.z - E
}

/// Intersect a ray with a plane defined by a point and normal.
pub(crate) fn ray_plane_intersection(
    ray: Ray3d,
    plane_point: Vec3,
    plane_normal: Vec3,
) -> Option<Vec3> {
    jackdaw_geometry::ray_plane_intersection(ray.origin, *ray.direction, plane_point, plane_normal)
}

/// Draw a grid of small crosses on the drawing plane, centered near `center`.
/// Grid points are world-aligned (fixed at world-space multiples of `inc`),
/// so only the visible window moves with the cursor. Individual crosses stay put.
pub(crate) fn draw_plane_grid(
    gizmos: &mut Gizmos<DrawBrushGizmoGroup>,
    plane: &DrawPlane,
    center: Vec3,
    snap_settings: &SnapSettings,
) {
    let inc = snap_settings.grid_size();
    let cross_size = inc * 0.1;
    let range = 10_i32;
    let fade_radius = range as f32 * inc;

    // World-aligned: project center directly onto axes (not relative to plane.origin)
    let u_center = (center.dot(plane.axis_u) / inc).round() as i32;
    let v_center = (center.dot(plane.axis_v) / inc).round() as i32;

    // Distance of the plane from the world origin along its normal
    let plane_d = plane.origin.dot(plane.normal);

    for du in -range..=range {
        for dv in -range..=range {
            let u = (u_center + du) as f32 * inc;
            let v = (v_center + dv) as f32 * inc;
            let pt = plane.axis_u * u + plane.axis_v * v + plane.normal * plane_d;

            // Distance-based alpha fade from cursor
            let dist = (pt - center).length();
            let alpha = (1.0 - dist / fade_radius).clamp(0.0, 0.3);
            if alpha <= 0.0 {
                continue;
            }
            let grid_color = default_style::DRAW_PLANE_GRID.with_alpha(alpha);

            gizmos.line(
                pt - plane.axis_u * cross_size,
                pt + plane.axis_u * cross_size,
                grid_color,
            );
            gizmos.line(
                pt - plane.axis_v * cross_size,
                pt + plane.axis_v * cross_size,
                grid_color,
            );
        }
    }
}

/// Snap a world-space hit point to a world-aligned grid on the drawing plane.
pub(crate) fn snap_to_plane_grid(
    hit: Vec3,
    plane: &DrawPlane,
    snap_settings: &SnapSettings,
    ctrl: bool,
) -> Vec3 {
    if !snap_settings.translate_active(ctrl) || snap_settings.translate_increment <= 0.0 {
        return hit;
    }
    let inc = snap_settings.translate_increment;
    // World-aligned: snap using world-space projections onto axes
    let u = hit.dot(plane.axis_u);
    let v = hit.dot(plane.axis_v);
    let snapped_u = (u / inc).round() * inc;
    let snapped_v = (v / inc).round() * inc;
    let plane_d = plane.origin.dot(plane.normal);
    plane.axis_u * snapped_u + plane.axis_v * snapped_v + plane.normal * plane_d
}

/// Constrain a hit point to the nearest 45-degree angle from an origin on the drawing plane.
pub(crate) fn snap_to_diagonal(hit: Vec3, origin: Vec3, plane: &DrawPlane) -> Vec3 {
    let delta_u = hit.dot(plane.axis_u) - origin.dot(plane.axis_u);
    let delta_v = hit.dot(plane.axis_v) - origin.dot(plane.axis_v);
    let angle = delta_v.atan2(delta_u);
    let snapped_angle = (angle / std::f32::consts::FRAC_PI_4).round() * std::f32::consts::FRAC_PI_4;
    let distance = (delta_u * delta_u + delta_v * delta_v).sqrt();
    let snapped_u = origin.dot(plane.axis_u) + distance * snapped_angle.cos();
    let snapped_v = origin.dot(plane.axis_v) + distance * snapped_angle.sin();
    let plane_d = plane.origin.dot(plane.normal);
    plane.axis_u * snapped_u + plane.axis_v * snapped_v + plane.normal * plane_d
}

/// True if the ring self-intersects when projected onto the drawing plane.
pub(crate) fn polygon_self_intersects_on_plane(points: &[Vec3], plane: &DrawPlane) -> bool {
    let n = points.len();
    if n < 4 {
        return false;
    }
    let projected: Vec<Vec2> = points
        .iter()
        .map(|&point| {
            Vec2::new(
                (point - plane.origin).dot(plane.axis_u),
                (point - plane.origin).dot(plane.axis_v),
            )
        })
        .collect();
    for i in 0..n {
        let i_next = (i + 1) % n;
        for j in (i + 1)..n {
            let j_next = (j + 1) % n;
            if i == j_next || j == i_next {
                continue;
            }
            if segments_intersect(
                projected[i],
                projected[i_next],
                projected[j],
                projected[j_next],
            ) {
                return true;
            }
        }
    }
    false
}

fn segments_intersect(start_a: Vec2, end_a: Vec2, start_b: Vec2, end_b: Vec2) -> bool {
    let orientation_start_b = orient(start_a, end_a, start_b);
    let orientation_end_b = orient(start_a, end_a, end_b);
    let orientation_start_a = orient(start_b, end_b, start_a);
    let orientation_end_a = orient(start_b, end_b, end_a);
    if orientation_start_b * orientation_end_b < 0.0
        && orientation_start_a * orientation_end_a < 0.0
    {
        return true;
    }
    const EPS: f32 = 1e-5;
    (orientation_start_b.abs() <= EPS && point_on_segment(start_b, start_a, end_a))
        || (orientation_end_b.abs() <= EPS && point_on_segment(end_b, start_a, end_a))
        || (orientation_start_a.abs() <= EPS && point_on_segment(start_a, start_b, end_b))
        || (orientation_end_a.abs() <= EPS && point_on_segment(end_a, start_b, end_b))
}

fn orient(start: Vec2, end: Vec2, point: Vec2) -> f32 {
    (end - start).perp_dot(point - start)
}

fn point_on_segment(point: Vec2, segment_start: Vec2, segment_end: Vec2) -> bool {
    const EPS: f32 = 1e-5;
    let segment = segment_end - segment_start;
    let to_point = point - segment_start;
    let length_squared = segment.length_squared();
    if length_squared <= EPS * EPS {
        return to_point.length_squared() <= EPS * EPS;
    }
    let parameter = to_point.dot(segment) / length_squared;
    (-EPS..=1.0 + EPS).contains(&parameter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::draw_brush::DrawPlane;
    use bevy::prelude::Vec3;

    fn xz_plane() -> DrawPlane {
        DrawPlane {
            origin: Vec3::ZERO,
            normal: Vec3::Y,
            axis_u: Vec3::X,
            axis_v: Vec3::Z,
        }
    }

    #[test]
    fn chevron_ring_is_not_self_intersecting() {
        let chevron = [
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(3.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(0.5, 0.0, 0.0),
        ];
        assert!(!polygon_self_intersects_on_plane(&chevron, &xz_plane()));
    }

    #[test]
    fn bowtie_ring_is_self_intersecting() {
        let bowtie = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
        ];
        assert!(polygon_self_intersects_on_plane(&bowtie, &xz_plane()));
    }
}
