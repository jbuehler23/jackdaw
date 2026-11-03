//! Checks on closed 2D outlines shared by the outline and profile builders.

use glam::Vec2;

/// Consecutive points closer than this are one point.
pub const MERGE_DISTANCE: f32 = 1e-5;
/// An outline enclosing less than this has no area.
pub const MIN_AREA: f32 = 1e-6;
/// Two edges closer than this fraction of the longer one touch.
const TOUCH_FRACTION: f32 = 1e-4;

/// Positive when the closed ring winds counter-clockwise.
pub fn signed_area(ring: &[Vec2]) -> f32 {
    let n = ring.len();
    (0..n)
        .map(|i| ring[i].perp_dot(ring[(i + 1) % n]))
        .sum::<f32>()
        / 2.0
}

/// True if two edges of the closed ring that do not share a corner cross or
/// come within a small fraction of the longer edge's length of each other.
pub fn polygon_self_intersects(ring: &[Vec2]) -> bool {
    let n = ring.len();
    if n < 4 {
        return false;
    }
    for i in 0..n {
        let i_next = (i + 1) % n;
        for j in (i + 1)..n {
            let j_next = (j + 1) % n;
            if i == j_next || j == i_next {
                continue;
            }
            if segments_touch(ring[i], ring[i_next], ring[j], ring[j_next]) {
                return true;
            }
        }
    }
    false
}

fn segments_touch(start_a: Vec2, end_a: Vec2, start_b: Vec2, end_b: Vec2) -> bool {
    let orient = |start: Vec2, end: Vec2, point: Vec2| (end - start).perp_dot(point - start);
    let crosses = |a: f32, b: f32| (a < 0.0 && b > 0.0) || (a > 0.0 && b < 0.0);
    if crosses(
        orient(start_a, end_a, start_b),
        orient(start_a, end_a, end_b),
    ) && crosses(
        orient(start_b, end_b, start_a),
        orient(start_b, end_b, end_a),
    ) {
        return true;
    }
    let tolerance = TOUCH_FRACTION * start_a.distance(end_a).max(start_b.distance(end_b));
    distance_to_segment(start_b, start_a, end_a) <= tolerance
        || distance_to_segment(end_b, start_a, end_a) <= tolerance
        || distance_to_segment(start_a, start_b, end_b) <= tolerance
        || distance_to_segment(end_a, start_b, end_b) <= tolerance
}

fn distance_to_segment(point: Vec2, start: Vec2, end: Vec2) -> f32 {
    let segment = end - start;
    let length_squared = segment.length_squared();
    if length_squared == 0.0 {
        return point.distance(start);
    }
    let t = ((point - start).dot(segment) / length_squared).clamp(0.0, 1.0);
    point.distance(start + segment * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scaled(ring: &[Vec2], scale: f32) -> Vec<Vec2> {
        ring.iter().map(|point| *point * scale).collect()
    }

    #[test]
    fn a_square_is_simple_at_any_scale() {
        let square = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(0.0, 1.0),
        ];
        for scale in [0.001, 1.0, 1000.0] {
            assert!(!polygon_self_intersects(&scaled(&square, scale)), "{scale}");
            assert!(
                (signed_area(&scaled(&square, scale)) - scale * scale).abs() < scale * scale * 1e-5
            );
        }
    }

    #[test]
    fn a_bowtie_and_a_corner_on_an_edge_intersect_at_any_scale() {
        let bowtie = [
            Vec2::new(0.0, 0.0),
            Vec2::new(1.0, 1.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 1.0),
        ];
        let touching = [
            Vec2::new(0.0, 0.0),
            Vec2::new(2.0, 0.0),
            Vec2::new(2.0, 2.0),
            Vec2::new(1.0, 0.0),
            Vec2::new(0.0, 2.0),
        ];
        for scale in [0.001, 1.0, 1000.0] {
            assert!(polygon_self_intersects(&scaled(&bowtie, scale)), "{scale}");
            assert!(
                polygon_self_intersects(&scaled(&touching, scale)),
                "{scale}"
            );
        }
    }
}
