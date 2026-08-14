//! Snapping terrain heights onto a game's own elevation lattice.
//!
//! A quantized terrain is one whose every stored height is an exact
//! multiple of a step. That is what makes an integer export lossless and
//! what gives grid-based and tactical games the readable, terraced ground
//! they want. The arithmetic lives here, away from the ECS, because three
//! callers need it at three different granularities: a sculpt stroke
//! snaps only the cells it touched, generate and erode snap a whole fresh
//! array, and the apply operator snaps an array that already exists.
//!
//! A step of zero, a negative step, or a non-finite one is a no-op rather
//! than an error. The setting ships off with `height_step: 0.0`, so the
//! disabled case is the common case and it has to be free.

use bevy_math::Vec2;

/// Snap one height onto the lattice of multiples of `step`.
///
/// Returns `h` unchanged when the step is not a usable interval. Ties
/// round away from zero, which is `f32::round`'s rule; the choice is
/// arbitrary but has to be the same everywhere or two paths would
/// disagree on a height sitting exactly between terraces.
pub fn quantize_height(h: f32, step: f32) -> f32 {
    if !step.is_finite() || step <= 0.0 {
        return h;
    }
    (h / step).round() * step
}

/// Snap a whole height array in place.
pub fn quantize_heights(heights: &mut [f32], step: f32) {
    if !step.is_finite() || step <= 0.0 {
        return;
    }
    for h in heights.iter_mut() {
        *h = (*h / step).round() * step;
    }
}

/// Snap only the cells a brush of this shape touched.
///
/// A sculpt stroke fires every frame while the button is held. Rewriting
/// a 512-resolution array -- 262,144 floats -- on each of those frames to
/// snap the few hundred cells under the brush is the difference between
/// terraces forming as the user drags and the editor stuttering. The
/// bounds are computed the same way [`crate::apply_brush`] computes them,
/// so the two agree on which cells a stroke owns.
///
/// `center` and `radius` are in grid cells, matching the brush.
pub fn quantize_region(heights: &mut [f32], resolution: u32, center: Vec2, radius: f32, step: f32) {
    if !step.is_finite() || step <= 0.0 || resolution == 0 || !radius.is_finite() || radius <= 0.0 {
        return;
    }
    let res = resolution;
    let min_x = ((center.x - radius).floor() as i32).max(0) as u32;
    let max_x = ((center.x + radius).ceil() as i32).min(res as i32 - 1);
    let min_z = ((center.y - radius).floor() as i32).max(0) as u32;
    let max_z = ((center.y + radius).ceil() as i32).min(res as i32 - 1);
    if max_x < 0 || max_z < 0 {
        return;
    }
    let (max_x, max_z) = (max_x as u32, max_z as u32);
    let radius_squared = radius * radius;

    for gz in min_z..=max_z {
        for gx in min_x..=max_x {
            let delta = Vec2::new(gx as f32, gz as f32) - center;
            if delta.length_squared() >= radius_squared {
                continue;
            }
            let idx = (gz * res + gx) as usize;
            if let Some(h) = heights.get_mut(idx) {
                *h = (*h / step).round() * step;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_multiples_are_left_where_they_are() {
        for h in [0.0, 0.25, 0.5, 12.75, -3.5] {
            assert_eq!(quantize_height(h, 0.25), h, "h = {h}");
        }
    }

    #[test]
    fn heights_land_on_the_nearest_step() {
        assert_eq!(quantize_height(0.3, 0.25), 0.25);
        assert_eq!(quantize_height(0.4, 0.25), 0.5);
        assert_eq!(quantize_height(1.2, 1.0), 1.0);
        assert_eq!(quantize_height(1.7, 1.0), 2.0);
    }

    #[test]
    fn negative_heights_snap_the_same_way() {
        assert_eq!(quantize_height(-0.3, 0.25), -0.25);
        assert_eq!(quantize_height(-0.4, 0.25), -0.5);
        assert_eq!(quantize_height(-1.7, 1.0), -2.0);
    }

    #[test]
    fn a_step_of_zero_or_less_is_a_no_op() {
        assert_eq!(quantize_height(0.37, 0.0), 0.37);
        assert_eq!(quantize_height(0.37, -0.25), 0.37);

        let mut heights = vec![0.37, 1.42];
        quantize_heights(&mut heights, 0.0);
        assert_eq!(heights, vec![0.37, 1.42]);
    }

    #[test]
    fn a_non_finite_step_is_a_no_op() {
        assert_eq!(quantize_height(0.37, f32::NAN), 0.37);
        assert_eq!(quantize_height(0.37, f32::INFINITY), 0.37);

        let mut heights = vec![0.37, 1.42];
        quantize_heights(&mut heights, f32::NAN);
        assert_eq!(heights, vec![0.37, 1.42]);
        quantize_heights(&mut heights, f32::INFINITY);
        assert_eq!(heights, vec![0.37, 1.42]);
    }

    #[test]
    fn every_height_in_the_array_snaps() {
        let mut heights = vec![0.3, 0.4, -0.3, 1.0];
        quantize_heights(&mut heights, 0.25);
        assert_eq!(heights, vec![0.25, 0.5, -0.25, 1.0]);
    }

    #[test]
    fn a_region_touches_only_the_cells_under_the_brush() {
        // 5x5 grid, every cell off-lattice by the same amount.
        let mut heights = vec![0.3_f32; 25];
        let center = Vec2::new(2.0, 2.0);
        let radius = 1.5;
        quantize_region(&mut heights, 5, center, radius, 0.25);

        // Quantization follows the same circular footprint as sculpting,
        // not the square used only to bound the iteration.
        for gz in 0..5_usize {
            for gx in 0..5_usize {
                let sample = Vec2::new(gx as f32, gz as f32);
                let inside = sample.distance(center) < radius;
                let h = heights[gz * 5 + gx];
                if inside {
                    assert_eq!(h, 0.25, "cell ({gx},{gz}) should have snapped");
                } else {
                    assert_eq!(h, 0.3, "cell ({gx},{gz}) should have been left alone");
                }
            }
        }
    }

    #[test]
    fn a_region_clamps_to_the_grid_instead_of_wrapping() {
        let mut heights = vec![0.3_f32; 25];
        // Centre off the near corner: only (0,0) lies inside the circular
        // footprint after the iteration bounds clamp to the grid.
        quantize_region(&mut heights, 5, Vec2::new(-0.5, -0.5), 1.0, 0.25);
        assert_eq!(heights[0], 0.25);
        assert_eq!(heights[1], 0.3);
        assert_eq!(heights[5], 0.3);
        assert_eq!(heights[5 + 1], 0.3);
        assert_eq!(heights[2], 0.3);
        assert_eq!(heights[24], 0.3);
    }

    #[test]
    fn a_region_wholly_off_the_grid_changes_nothing() {
        let mut heights = vec![0.3_f32; 25];
        quantize_region(&mut heights, 5, Vec2::new(-40.0, -40.0), 1.0, 0.25);
        assert!(heights.iter().all(|h| *h == 0.3));
    }

    #[test]
    fn a_region_with_a_dead_step_or_radius_is_a_no_op() {
        let mut heights = vec![0.3_f32; 25];
        quantize_region(&mut heights, 5, Vec2::splat(2.0), 2.0, 0.0);
        quantize_region(&mut heights, 5, Vec2::splat(2.0), 0.0, 0.25);
        quantize_region(&mut heights, 0, Vec2::splat(2.0), 2.0, 0.25);
        assert!(heights.iter().all(|h| *h == 0.3));
    }
}
