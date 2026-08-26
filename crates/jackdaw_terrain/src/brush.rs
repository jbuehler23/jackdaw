use crate::heightmap::Heightmap;
use bevy_math::Vec2;

/// Sculpting tool types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SculptTool {
    Raise,
    Lower,
    Flatten,
    Smooth,
    Noise,
}

/// Compute brush falloff: `(1 - (dist/radius)^power).max(0)`.
pub fn compute_falloff(dist: f32, radius: f32, power: f32) -> f32 {
    if dist >= radius {
        return 0.0;
    }
    (1.0 - (dist / radius).powf(power)).max(0.0)
}

/// Apply a brush stroke to the heightmap.
///
/// `center`: grid-space center of the brush.
/// `radius`: brush radius in grid cells.
/// `strength`: brush strength per second.
/// `falloff`: falloff power (1.0 = linear, 2.0 = quadratic, etc.).
/// `dt`: frame delta time.
/// `noise_fn`: optional noise function for the Noise tool; takes (`grid_x`, `grid_z`).
pub fn apply_brush(
    heightmap: &mut Heightmap,
    tool: SculptTool,
    center: Vec2,
    radius: f32,
    strength: f32,
    falloff: f32,
    dt: f32,
    noise_fn: Option<&dyn Fn(f32, f32) -> f32>,
) {
    let resolution = heightmap.resolution;
    apply_brush_at(
        &mut heightmap.heights,
        resolution,
        tool,
        center,
        radius,
        strength,
        falloff,
        dt,
        noise_fn,
    );
}

/// [`apply_brush`] against a dense height grid the caller already holds.
///
/// Writes in place, so a stroke over a terrain document's region needs no
/// round trip through a [`Heightmap`].
pub fn apply_brush_at(
    heights: &mut [f32],
    resolution: u32,
    tool: SculptTool,
    center: Vec2,
    radius: f32,
    strength: f32,
    falloff: f32,
    dt: f32,
    noise_fn: Option<&dyn Fn(f32, f32) -> f32>,
) {
    // `compute_falloff` swallows a non-finite falloff or distance through
    // `f32::max`, but `amount = strength * f * dt` does not: a non-finite
    // radius, strength or dt writes NaN into the heightmap.
    if resolution == 0
        || !radius.is_finite()
        || radius <= 0.0
        || !strength.is_finite()
        || !dt.is_finite()
    {
        return;
    }

    let res = resolution as i32;
    let center_height = sample_bilinear_in(heights, resolution, center.x, center.y);

    // Clamped at both ends: a footprint entirely off the low edge gives a
    // negative `ceil`, which as an unsigned bound spans four billion cells.
    let min_x = ((center.x - radius).floor() as i32).clamp(0, res - 1);
    let max_x = ((center.x + radius).ceil() as i32).clamp(0, res - 1);
    let min_z = ((center.y - radius).floor() as i32).clamp(0, res - 1);
    let max_z = ((center.y + radius).ceil() as i32).clamp(0, res - 1);

    // Smoothing averages the heights as they were at the start of the
    // call, so one cell's new value never feeds the next cell's average.
    let snapshot = (tool == SculptTool::Smooth).then(|| heights.to_vec());

    for gz in min_z..=max_z {
        for gx in min_x..=max_x {
            let dist = ((gx as f32 - center.x).powi(2) + (gz as f32 - center.y).powi(2)).sqrt();
            let f = compute_falloff(dist, radius, falloff);
            if f <= 0.0 {
                continue;
            }

            let idx = (gz * res + gx) as usize;
            let h = heights[idx];
            let amount = strength * f * dt;

            heights[idx] = match tool {
                SculptTool::Raise => h + amount,
                SculptTool::Lower => h - amount,
                SculptTool::Flatten => lerp(h, center_height, amount.min(1.0)),
                SculptTool::Smooth => {
                    let avg = snapshot.as_ref().map_or(h, |snap| {
                        average_neighbors_from_slice(snap, resolution, gx as u32, gz as u32)
                    });
                    lerp(h, avg, amount.min(1.0))
                }
                SculptTool::Noise => {
                    if let Some(nf) = noise_fn {
                        h + nf(gx as f32, gz as f32) * amount
                    } else {
                        h
                    }
                }
            };
        }
    }
}

/// [`Heightmap::sample_bilinear`] against a bare grid.
fn sample_bilinear_in(heights: &[f32], resolution: u32, gx: f32, gz: f32) -> f32 {
    let x0 = gx.floor() as i32;
    let z0 = gz.floor() as i32;
    let fx = gx - x0 as f32;
    let fz = gz - z0 as f32;

    let s = |x: i32, z: i32| -> f32 {
        if resolution == 0 {
            return 0.0;
        }
        let x = x.clamp(0, resolution as i32 - 1) as u32;
        let z = z.clamp(0, resolution as i32 - 1) as u32;
        heights
            .get((z * resolution + x) as usize)
            .copied()
            .unwrap_or(0.0)
    };

    let h00 = s(x0, z0);
    let h10 = s(x0 + 1, z0);
    let h01 = s(x0, z0 + 1);
    let h11 = s(x0 + 1, z0 + 1);

    let h0 = h00 * (1.0 - fx) + h10 * fx;
    let h1 = h01 * (1.0 - fx) + h11 * fx;
    h0 * (1.0 - fz) + h1 * fz
}

fn average_neighbors_from_slice(heights: &[f32], res: u32, x: u32, z: u32) -> f32 {
    let mut sum = 0.0;
    let mut count = 0.0;

    for dz in [-1_i32, 0, 1] {
        for dx in [-1_i32, 0, 1] {
            if dx == 0 && dz == 0 {
                continue;
            }
            let nx = x as i32 + dx;
            let nz = z as i32 + dz;
            if nx >= 0 && nx < res as i32 && nz >= 0 && nz < res as i32 {
                sum += heights[(nz as u32 * res + nx as u32) as usize];
                count += 1.0;
            }
        }
    }

    if count > 0.0 {
        sum / count
    } else {
        heights[(z * res + x) as usize]
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Determine which chunks are affected by a brush stroke.
pub fn affected_chunks(
    heightmap: &Heightmap,
    center: Vec2,
    radius: f32,
    chunk_size: u32,
) -> Vec<(u32, u32)> {
    affected_chunks_at(heightmap.resolution, center, radius, chunk_size)
}

/// Determine which chunks a brush stroke touches, given only the grid
/// resolution.
///
/// Same result as [`affected_chunks`] without needing the heights.
pub fn affected_chunks_at(
    resolution: u32,
    center: Vec2,
    radius: f32,
    chunk_size: u32,
) -> Vec<(u32, u32)> {
    if resolution < 2 || chunk_size == 0 {
        return Vec::new();
    }
    let cells = resolution - 1;
    let cx_count = cells.div_ceil(chunk_size);
    let cz_count = cells.div_ceil(chunk_size);

    let min_gx = (center.x - radius).floor().max(0.0) as u32;
    let max_gx = (center.x + radius).ceil().min(resolution as f32 - 1.0) as u32;
    let min_gz = (center.y - radius).floor().max(0.0) as u32;
    let max_gz = (center.y + radius).ceil().min(resolution as f32 - 1.0) as u32;

    let min_cx = min_gx / chunk_size;
    let max_cx = (max_gx / chunk_size).min(cx_count - 1);
    let min_cz = min_gz / chunk_size;
    let max_cz = (max_gz / chunk_size).min(cz_count - 1);

    let mut chunks = Vec::new();
    for cz in min_cz..=max_cz {
        for cx in min_cx..=max_cx {
            chunks.push((cx, cz));
        }
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A footprint entirely off one edge writes nothing, without walking
    /// the four billion cells a negative `ceil` spans when cast unsigned.
    #[test]
    fn a_brush_entirely_off_the_grid_writes_nothing() {
        let resolution = 16;
        for center in [
            Vec2::new(-400.0, 8.0),
            Vec2::new(400.0, 8.0),
            Vec2::new(8.0, -400.0),
            Vec2::new(8.0, 400.0),
        ] {
            let mut heights = vec![1.0f32; (resolution * resolution) as usize];
            apply_brush_at(
                &mut heights,
                resolution,
                SculptTool::Raise,
                center,
                4.0,
                10.0,
                2.0,
                1.0 / 60.0,
                None,
            );
            assert!(heights.iter().all(|h| *h == 1.0), "{center:?} wrote a cell");
        }
    }

    /// A grid with no cells has nothing to clamp a bound between.
    #[test]
    fn a_zero_resolution_grid_is_left_alone() {
        let mut heights: Vec<f32> = Vec::new();
        apply_brush_at(
            &mut heights,
            0,
            SculptTool::Raise,
            Vec2::ZERO,
            4.0,
            10.0,
            2.0,
            1.0 / 60.0,
            None,
        );
        assert!(heights.is_empty());
    }

    /// Both forms have to write the same heights bit for bit.
    #[test]
    fn brushing_in_place_matches_brushing_a_heightmap() {
        for tool in [
            SculptTool::Raise,
            SculptTool::Lower,
            SculptTool::Flatten,
            SculptTool::Smooth,
        ] {
            let resolution = 24;
            let seeded: Vec<f32> = (0..resolution * resolution)
                .map(|i| (i as f32 * 0.37).sin() * 4.0)
                .collect();

            let mut owned = Heightmap::new(resolution, Vec2::splat(23.0), 10.0);
            owned.heights.clone_from(&seeded);
            let mut in_place = seeded.clone();

            for center in [Vec2::new(12.0, 12.0), Vec2::new(3.5, 20.5)] {
                apply_brush(&mut owned, tool, center, 4.0, 7.0, 2.0, 1.0 / 60.0, None);
                apply_brush_at(
                    &mut in_place,
                    resolution,
                    tool,
                    center,
                    4.0,
                    7.0,
                    2.0,
                    1.0 / 60.0,
                    None,
                );
            }

            assert_eq!(
                owned
                    .heights
                    .iter()
                    .map(|h| h.to_bits())
                    .collect::<Vec<_>>(),
                in_place.iter().map(|h| h.to_bits()).collect::<Vec<_>>(),
                "{tool:?} disagreed between the two forms",
            );
        }
    }

    #[test]
    fn chunk_lookup_agrees_with_the_heightmap_form() {
        let hm = Heightmap::new(65, Vec2::splat(64.0), 10.0);
        let center = Vec2::new(40.0, 20.0);
        assert_eq!(
            affected_chunks(&hm, center, 5.0, 32),
            affected_chunks_at(65, center, 5.0, 32),
        );
    }

    #[test]
    fn a_degenerate_grid_touches_no_chunks_instead_of_panicking() {
        assert!(affected_chunks_at(0, Vec2::ZERO, 4.0, 32).is_empty());
        assert!(affected_chunks_at(1, Vec2::ZERO, 4.0, 32).is_empty());
        assert!(affected_chunks_at(65, Vec2::ZERO, 4.0, 0).is_empty());
    }

    #[test]
    fn a_stroke_spanning_a_chunk_seam_marks_both_chunks() {
        let touched = affected_chunks_at(65, Vec2::new(32.0, 32.0), 3.0, 32);
        assert_eq!(touched.len(), 4, "got {touched:?}");
    }

    #[test]
    fn a_non_finite_strength_does_not_poison_the_heightmap() {
        let mut hm = Heightmap::new(5, Vec2::splat(4.0), 10.0);
        hm.heights = vec![1.0; 25];

        apply_brush(
            &mut hm,
            SculptTool::Raise,
            Vec2::new(2.0, 2.0),
            2.0,
            f32::NAN,
            1.0,
            1.0 / 60.0,
            None,
        );

        assert!(
            hm.heights.iter().all(|h| *h == 1.0),
            "a non-finite strength must leave the heightmap untouched: {:?}",
            hm.heights
        );
    }

    #[test]
    fn a_non_finite_radius_does_not_poison_the_heightmap() {
        let mut hm = Heightmap::new(5, Vec2::splat(4.0), 10.0);
        hm.heights = vec![1.0; 25];

        apply_brush(
            &mut hm,
            SculptTool::Raise,
            Vec2::new(2.0, 2.0),
            f32::INFINITY,
            1.0,
            1.0,
            1.0 / 60.0,
            None,
        );

        assert!(hm.heights.iter().all(|h| *h == 1.0));
    }
}
