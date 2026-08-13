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

/// Average of 8 neighbors for smoothing.
fn average_neighbors(heightmap: &Heightmap, x: u32, z: u32) -> f32 {
    let mut sum = 0.0;
    let mut count = 0.0;
    let res = heightmap.resolution;

    for dz in [-1_i32, 0, 1] {
        for dx in [-1_i32, 0, 1] {
            if dx == 0 && dz == 0 {
                continue;
            }
            let nx = x as i32 + dx;
            let nz = z as i32 + dz;
            if nx >= 0 && nx < res as i32 && nz >= 0 && nz < res as i32 {
                sum += heightmap.get_height(nx as u32, nz as u32);
                count += 1.0;
            }
        }
    }

    if count > 0.0 {
        sum / count
    } else {
        heightmap.get_height(x, z)
    }
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
    // `compute_falloff` already swallows a non-finite falloff or distance
    // via `f32::max`'s NaN-ignoring semantics, but `amount = strength * f
    // * dt` below has no such protection: a non-finite radius, strength,
    // or dt would otherwise write NaN straight into the heightmap.
    if !radius.is_finite() || radius <= 0.0 || !strength.is_finite() || !dt.is_finite() {
        return;
    }

    let res = heightmap.resolution;
    let center_height = heightmap.sample_bilinear(center.x, center.y);

    let min_x = ((center.x - radius).floor() as i32).max(0) as u32;
    let max_x = ((center.x + radius).ceil() as i32).min(res as i32 - 1) as u32;
    let min_z = ((center.y - radius).floor() as i32).max(0) as u32;
    let max_z = ((center.y + radius).ceil() as i32).min(res as i32 - 1) as u32;

    // For smoothing, we need a snapshot to avoid reading modified values
    let snapshot = if tool == SculptTool::Smooth {
        Some(heightmap.heights.clone())
    } else {
        None
    };

    for gz in min_z..=max_z {
        for gx in min_x..=max_x {
            let dist = ((gx as f32 - center.x).powi(2) + (gz as f32 - center.y).powi(2)).sqrt();
            let f = compute_falloff(dist, radius, falloff);
            if f <= 0.0 {
                continue;
            }

            let idx = (gz * res + gx) as usize;
            let h = heightmap.heights[idx];
            let amount = strength * f * dt;

            heightmap.heights[idx] = match tool {
                SculptTool::Raise => h + amount,
                SculptTool::Lower => h - amount,
                SculptTool::Flatten => lerp(h, center_height, amount.min(1.0)),
                SculptTool::Smooth => {
                    // Use snapshot for neighbor average
                    let avg = if let Some(ref snap) = snapshot {
                        average_neighbors_from_slice(snap, res, gx, gz)
                    } else {
                        average_neighbors(heightmap, gx, gz)
                    };
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
/// Same result as [`affected_chunks`] without needing the heights, so a
/// paint stroke over a channel can mark exactly the chunks a sculpt stroke
/// of the same shape would.
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

    /// A non-finite strength must not poison the heightmap.
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
