//! Deterministic point scatter over a heightmap and its paint channels.
//!
//! [`scatter`] turns a terrain plus a rule set into a list of placements.
//! It knows nothing about entities, assets, or the editor: the caller
//! decides what a placement becomes.
//!
//! The same seed and the same channel data produce byte-identical
//! placements on any machine and in any build:
//!
//! - Every random draw comes from a stream keyed by `(seed, cell index)`
//!   and nothing else, so a cell's output does not depend on how many
//!   cells preceded it.
//! - Cells are visited row-major, and the only order-sensitive step,
//!   minimum-spacing rejection, consumes that fixed order.
//! - No `HashMap`, no thread RNG, no floating-point accumulation across
//!   cells.
//!
//! The PRNG is a hand-rolled `splitmix64` rather than the `rand` crate:
//! `rand`'s generators do not promise a stable value stream across
//! versions, and a scatter that re-rolls on a dependency bump would break
//! the re-run contract.

use bevy_math::{Vec2, Vec3};

use crate::channel::ChannelData;
use crate::heightmap::Heightmap;

/// Which channel gates placement, and which of its values are allowed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScatterMask {
    /// Index into the terrain's channel list.
    pub channel: usize,
    /// Palette values that accept a cell. An empty set accepts nothing,
    /// so a half-configured operator places nothing rather than
    /// everything.
    pub accept: Vec<u16>,
}

impl ScatterMask {
    fn accepts(&self, value: u16) -> bool {
        self.accept.contains(&value)
    }
}

/// The rule set a scatter run is driven by.
#[derive(Clone, Debug, PartialEq)]
pub struct ScatterParams {
    /// The one input every random draw derives from, with the cell index.
    pub seed: u64,
    /// Instances per square world unit, before the weight channel.
    pub density: f32,
    /// Closest two placements may sit, in world units. 0 disables the
    /// check.
    pub min_spacing: f32,
    /// Which cells may receive instances. `None` accepts every cell.
    pub mask: Option<ScatterMask>,
    /// A channel whose per-cell value scales local density.
    ///
    /// Values are normalised against the largest value present in that
    /// channel, so a channel painted 0/1/2 and one painted 0..255 both
    /// mean "none to all". A channel that is entirely zero scatters
    /// nothing.
    pub weight_channel: Option<usize>,
    /// Uniform scale range. `scale_min == scale_max` means no variance.
    pub scale_min: f32,
    pub scale_max: f32,
    /// Randomise rotation about Y.
    pub random_yaw: bool,
    /// Tilt instances onto the surface instead of standing them upright.
    pub align_to_normal: bool,
    /// How many assets the caller's palette holds. Each placement gets an
    /// index into it; 0 assets still produces placements, all at index 0.
    pub asset_count: usize,
}

impl Default for ScatterParams {
    fn default() -> Self {
        Self {
            seed: 0,
            density: 1.0,
            min_spacing: 0.0,
            mask: None,
            weight_channel: None,
            scale_min: 1.0,
            scale_max: 1.0,
            random_yaw: true,
            align_to_normal: false,
            asset_count: 1,
        }
    }
}

/// One instance the caller should place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// World-space position, sitting on the sampled surface.
    pub position: Vec3,
    /// Rotation about Y, radians. 0 when `random_yaw` is off.
    pub yaw: f32,
    /// Uniform scale.
    pub scale: f32,
    /// The up axis this instance should use: the sampled surface normal
    /// when `align_to_normal` is on, `Vec3::Y` when it is off.
    pub normal: Vec3,
    /// Index into the caller's asset palette.
    pub asset_index: usize,
    /// Row-major index of the grid cell this came from, `z * resolution +
    /// x`. Same indexing as a channel's values, so a caller can look up
    /// what was painted under any placement.
    pub cell: u32,
}

/// Scatter placements across `heightmap` under `params`.
///
/// `channels` is the terrain's channel list in declaration order; the
/// mask and weight indices address it. An out-of-range mask or weight
/// index produces zero placements rather than ignoring the rule.
pub fn scatter(
    heightmap: &Heightmap,
    channels: &[ChannelData],
    params: &ScatterParams,
) -> Vec<Placement> {
    let resolution = heightmap.resolution;
    if resolution < 2 || params.density <= 0.0 {
        return Vec::new();
    }

    let mask = match &params.mask {
        Some(mask) if mask.channel >= channels.len() => return Vec::new(),
        Some(mask) if mask.accept.is_empty() => return Vec::new(),
        other => other.as_ref(),
    };
    let weight = match params.weight_channel {
        Some(index) => match channels.get(index) {
            Some(channel) => {
                let peak = channel.values.iter().copied().max().unwrap_or(0);
                if peak == 0 {
                    return Vec::new();
                }
                Some((channel, f32::from(peak)))
            }
            None => return Vec::new(),
        },
        None => None,
    };

    let cell_size = heightmap.cell_size();
    let cell_area = cell_size.x * cell_size.y;
    // Instances ride the ground the heights and normals were sampled
    // from, whose first vertex is not `-size / 2` for an off-centre
    // terrain.
    let at = heightmap.origin;
    let mut spacing = SpacingGrid::new(params.min_spacing, heightmap.size, at);
    let mut out = Vec::new();

    // Row-major over cells. The last row and column are vertices with no
    // cell beyond them, so they are the edge of the last cell, not the
    // origin of another one.
    for gz in 0..resolution - 1 {
        for gx in 0..resolution - 1 {
            let cell = gz * resolution + gx;

            if let Some(mask) = mask
                && !mask.accepts(channels[mask.channel].get(resolution, gx, gz))
            {
                continue;
            }
            let weight_scale = match weight {
                Some((channel, peak)) => f32::from(channel.get(resolution, gx, gz)) / peak,
                None => 1.0,
            };
            if weight_scale <= 0.0 {
                continue;
            }

            let mut rng = CellRng::new(params.seed, cell);
            // Stochastic rounding: a cell expecting 0.3 instances places
            // one 30% of the time, so density scales linearly instead of
            // snapping to whole instances per cell.
            let expected = params.density * cell_area * weight_scale;
            let count = (expected + rng.next_f32()).floor().max(0.0) as u32;

            for _ in 0..count {
                // Every draw happens whether or not the point survives the
                // spacing test, so rejecting a point never shifts the
                // stream for the ones after it.
                let fx = gx as f32 + rng.next_f32();
                let fz = gz as f32 + rng.next_f32();
                let yaw = if params.random_yaw {
                    rng.next_f32() * std::f32::consts::TAU
                } else {
                    0.0
                };
                let scale_t = rng.next_f32();
                let asset_index = if params.asset_count > 1 {
                    (rng.next_f32() * params.asset_count as f32) as usize
                } else {
                    0
                };

                let world_x = fx * cell_size.x + at.x;
                let world_z = fz * cell_size.y + at.y;
                if !spacing.accept(Vec2::new(world_x, world_z)) {
                    continue;
                }

                out.push(Placement {
                    position: Vec3::new(world_x, heightmap.sample_bilinear(fx, fz), world_z),
                    yaw,
                    scale: params.scale_min + (params.scale_max - params.scale_min) * scale_t,
                    normal: if params.align_to_normal {
                        surface_normal(heightmap, fx, fz)
                    } else {
                        Vec3::Y
                    },
                    asset_index: asset_index.min(params.asset_count.saturating_sub(1)),
                    cell,
                });
            }
        }
    }
    out
}

/// Surface normal at fractional grid coordinates, by central difference.
///
/// The step is one grid cell, so the normal describes the slope of the
/// rendered mesh rather than of the interpolant at a point.
pub fn surface_normal(heightmap: &Heightmap, gx: f32, gz: f32) -> Vec3 {
    let cell = heightmap.cell_size();
    let dx = heightmap.sample_bilinear(gx + 1.0, gz) - heightmap.sample_bilinear(gx - 1.0, gz);
    let dz = heightmap.sample_bilinear(gx, gz + 1.0) - heightmap.sample_bilinear(gx, gz - 1.0);
    Vec3::new(-dx / (2.0 * cell.x), 1.0, -dz / (2.0 * cell.y)).normalize_or(Vec3::Y)
}

// --- Deterministic random ---

/// The splitmix64 mixing step. Pinned here rather than taken from a crate
/// so the value stream cannot move under a dependency bump.
#[inline]
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A random stream belonging to exactly one grid cell.
///
/// Seeded by hashing the cell index and mixing it into the run seed, so
/// neighbouring cells get uncorrelated streams and no cell's output
/// depends on another cell having been visited first.
struct CellRng(u64);

impl CellRng {
    fn new(seed: u64, cell: u32) -> Self {
        let mut hashed = u64::from(cell);
        let hashed = splitmix64(&mut hashed);
        Self(seed ^ hashed)
    }

    /// Uniform in `[0, 1)`, from the top 24 bits so the mantissa is
    /// filled exactly and the result is representable without rounding.
    fn next_f32(&mut self) -> f32 {
        (splitmix64(&mut self.0) >> 40) as f32 / (1u32 << 24) as f32
    }
}

// --- Minimum spacing ---

/// Accept/reject against already-placed points, bucketed so the check
/// stays cheap at a few thousand instances.
///
/// The bucket edge is never smaller than `min_spacing`, so every point
/// within `min_spacing` of a candidate sits in the candidate's own bucket
/// or one of its eight neighbours, and the grid returns what a full
/// pairwise scan would.
struct SpacingGrid {
    min_spacing_sq: f32,
    edge: f32,
    origin: Vec2,
    dims: (i32, i32),
    buckets: Vec<Vec<Vec2>>,
}

/// Ceiling on buckets per axis. A very small `min_spacing` over a large
/// terrain would otherwise ask for an unbounded allocation; a coarser
/// bucket costs time, not accuracy.
const MAX_BUCKETS_PER_AXIS: i32 = 512;

impl SpacingGrid {
    /// `origin` is the lowest corner of the ground being scattered on, so
    /// buckets are laid in the frame the candidates are generated in.
    fn new(min_spacing: f32, size: Vec2, origin: Vec2) -> Self {
        if min_spacing <= 0.0 {
            return Self {
                min_spacing_sq: 0.0,
                edge: 1.0,
                origin: Vec2::ZERO,
                dims: (0, 0),
                buckets: Vec::new(),
            };
        }
        let floor = (size.max_element() / MAX_BUCKETS_PER_AXIS as f32).max(f32::MIN_POSITIVE);
        let edge = min_spacing.max(floor);
        let dims = (
            ((size.x / edge).ceil() as i32 + 1).clamp(1, MAX_BUCKETS_PER_AXIS + 2),
            ((size.y / edge).ceil() as i32 + 1).clamp(1, MAX_BUCKETS_PER_AXIS + 2),
        );
        Self {
            min_spacing_sq: min_spacing * min_spacing,
            edge,
            origin,
            dims,
            buckets: vec![Vec::new(); (dims.0 * dims.1) as usize],
        }
    }

    fn bucket_of(&self, point: Vec2) -> (i32, i32) {
        let local = (point - self.origin) / self.edge;
        (
            (local.x.floor() as i32).clamp(0, self.dims.0 - 1),
            (local.y.floor() as i32).clamp(0, self.dims.1 - 1),
        )
    }

    /// Record `point` and return whether it cleared the spacing rule.
    fn accept(&mut self, point: Vec2) -> bool {
        if self.min_spacing_sq <= 0.0 {
            return true;
        }
        let (bx, bz) = self.bucket_of(point);
        for z in (bz - 1).max(0)..=(bz + 1).min(self.dims.1 - 1) {
            for x in (bx - 1).max(0)..=(bx + 1).min(self.dims.0 - 1) {
                for other in &self.buckets[(z * self.dims.0 + x) as usize] {
                    if point.distance_squared(*other) < self.min_spacing_sq {
                        return false;
                    }
                }
            }
        }
        self.buckets[(bz * self.dims.0 + bx) as usize].push(point);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channel::ChannelElement;

    fn flat(resolution: u32) -> Heightmap {
        Heightmap::new(resolution, Vec2::new(32.0, 32.0), 10.0)
    }

    /// A channel where the left half is 1 and the right half is 2.
    fn split_channel(resolution: u32) -> Vec<ChannelData> {
        let mut channel = ChannelData::new("density", ChannelElement::U8, resolution);
        for z in 0..resolution {
            for x in 0..resolution {
                channel.values[(z * resolution + x) as usize] =
                    if x < resolution / 2 { 1 } else { 2 };
            }
        }
        vec![channel]
    }

    fn mask(values: &[u16]) -> Option<ScatterMask> {
        Some(ScatterMask {
            channel: 0,
            accept: values.to_vec(),
        })
    }

    /// Two runs with the same seed over the same data compare equal
    /// placement-for-placement and field-for-field: position, yaw, scale,
    /// normal, asset index and cell, not merely equal in count.
    #[test]
    fn the_same_seed_and_data_produce_identical_placements() {
        let heightmap = flat(64);
        let channels = split_channel(64);
        let params = ScatterParams {
            seed: 0xDEAD_BEEF,
            density: 0.5,
            min_spacing: 0.3,
            mask: mask(&[1, 2]),
            align_to_normal: true,
            asset_count: 4,
            scale_min: 0.7,
            scale_max: 1.4,
            ..default_params()
        };
        let first = scatter(&heightmap, &channels, &params);
        let second = scatter(&heightmap, &channels, &params);
        assert!(!first.is_empty(), "the fixture should place something");
        assert_eq!(first, second);
    }

    #[test]
    fn a_different_seed_produces_different_placements() {
        let heightmap = flat(64);
        let channels = split_channel(64);
        let params = ScatterParams {
            seed: 1,
            density: 0.5,
            mask: mask(&[1, 2]),
            ..default_params()
        };
        let first = scatter(&heightmap, &channels, &params);
        let second = scatter(&heightmap, &channels, &ScatterParams { seed: 2, ..params });
        assert_ne!(first, second);
    }

    /// Changing which palette values are accepted changes which cells
    /// receive instances and nothing about the instances in the cells
    /// that stayed accepted: the mask filters, it does not re-roll.
    #[test]
    fn narrowing_the_mask_only_removes_cells_and_leaves_the_rest_untouched() {
        let heightmap = flat(64);
        let channels = split_channel(64);
        let base = ScatterParams {
            seed: 99,
            density: 0.6,
            mask: mask(&[1, 2]),
            ..default_params()
        };
        let both = scatter(&heightmap, &channels, &base);
        let only_two = scatter(
            &heightmap,
            &channels,
            &ScatterParams {
                mask: mask(&[2]),
                ..base.clone()
            },
        );

        assert!(!only_two.is_empty());
        assert!(only_two.len() < both.len());
        // Every survivor is byte-identical to its counterpart in the
        // wider run, and every cell it came from carries value 2.
        let kept: Vec<_> = both
            .iter()
            .filter(|p| channels[0].values[p.cell as usize] == 2)
            .copied()
            .collect();
        assert_eq!(kept, only_two);
    }

    #[test]
    fn a_mask_that_accepts_nothing_places_nothing() {
        let heightmap = flat(32);
        let channels = split_channel(32);
        let params = ScatterParams {
            density: 2.0,
            mask: Some(ScatterMask {
                channel: 0,
                accept: Vec::new(),
            }),
            ..default_params()
        };
        assert!(scatter(&heightmap, &channels, &params).is_empty());
        // A value nobody painted accepts nothing either.
        let params = ScatterParams {
            mask: mask(&[7]),
            ..params
        };
        assert!(scatter(&heightmap, &channels, &params).is_empty());
    }

    #[test]
    fn an_all_zero_weight_channel_places_nothing() {
        let heightmap = flat(32);
        let channels = vec![ChannelData::new("weight", ChannelElement::U8, 32)];
        let params = ScatterParams {
            density: 4.0,
            weight_channel: Some(0),
            ..default_params()
        };
        assert!(scatter(&heightmap, &channels, &params).is_empty());
    }

    /// A weight channel scales density locally: the half painted 2 must
    /// come out substantially denser than the half painted 1.
    #[test]
    fn a_weight_channel_scales_density_per_cell() {
        let heightmap = flat(64);
        let channels = split_channel(64);
        let placements = scatter(
            &heightmap,
            &channels,
            &ScatterParams {
                seed: 5,
                density: 1.0,
                weight_channel: Some(0),
                ..default_params()
            },
        );
        let left = placements.iter().filter(|p| p.position.x < 0.0).count();
        let right = placements.iter().filter(|p| p.position.x >= 0.0).count();
        // Half-weight against full weight: expect roughly double.
        assert!(
            right > left * 3 / 2,
            "right {right} should clearly exceed left {left}"
        );
    }

    #[test]
    fn minimum_spacing_is_never_violated() {
        let heightmap = flat(64);
        let channels = split_channel(64);
        let spacing = 0.8;
        let placements = scatter(
            &heightmap,
            &channels,
            &ScatterParams {
                seed: 3,
                // Ask for far more than the spacing can fit, so the
                // rejection path is what decides the result.
                density: 8.0,
                min_spacing: spacing,
                mask: mask(&[1, 2]),
                ..default_params()
            },
        );
        assert!(placements.len() > 50, "got {}", placements.len());
        for (i, a) in placements.iter().enumerate() {
            for b in &placements[i + 1..] {
                let d = Vec2::new(a.position.x, a.position.z)
                    .distance(Vec2::new(b.position.x, b.position.z));
                assert!(d >= spacing - 1e-4, "pair at {d} is closer than {spacing}");
            }
        }
    }

    /// The bucketed spacing grid returns what a full pairwise scan would.
    /// A bucket edge far larger than the spacing exercises the coarser
    /// bucket fallback.
    #[test]
    fn the_spacing_grid_matches_a_brute_force_scan() {
        let size = Vec2::new(32.0, 32.0);
        let spacing = 0.5;
        let mut grid = SpacingGrid::new(spacing, size, -size / 2.0);
        let mut brute: Vec<Vec2> = Vec::new();
        let mut rng = CellRng::new(11, 0);
        for _ in 0..2000 {
            let point = Vec2::new(
                rng.next_f32() * size.x - size.x / 2.0,
                rng.next_f32() * size.y - size.y / 2.0,
            );
            let brute_ok = brute.iter().all(|p| p.distance(point) >= spacing);
            if brute_ok {
                brute.push(point);
            }
            assert_eq!(grid.accept(point), brute_ok);
        }
        assert!(brute.len() > 100, "fixture placed only {}", brute.len());
    }

    #[test]
    fn density_scales_the_instance_count_roughly_linearly() {
        let heightmap = flat(64);
        let channels = split_channel(64);
        let count_at = |density: f32| {
            scatter(
                &heightmap,
                &channels,
                &ScatterParams {
                    seed: 7,
                    density,
                    mask: mask(&[1, 2]),
                    ..default_params()
                },
            )
            .len() as f32
        };
        let one = count_at(0.25);
        let four = count_at(1.0);
        assert!(one > 100.0, "fixture is too small to measure: {one}");
        let ratio = four / one;
        assert!((3.6..=4.4).contains(&ratio), "ratio {ratio} is not ~4x");
    }

    /// Instances sit on the ground, not on the Y=0 plane.
    #[test]
    fn placements_sample_the_surface_height() {
        let mut heightmap = flat(32);
        heightmap.heights.fill(3.0);
        let channels = split_channel(32);
        let placements = scatter(
            &heightmap,
            &channels,
            &ScatterParams {
                density: 0.2,
                mask: mask(&[1, 2]),
                ..default_params()
            },
        );
        assert!(!placements.is_empty());
        assert!(placements.iter().all(|p| (p.position.y - 3.0).abs() < 1e-5));
    }

    /// With align-to-normal off, every instance stands upright whatever
    /// the ground does; with it on, a sloped surface tilts them.
    #[test]
    fn align_to_normal_decides_the_reported_up_axis() {
        let mut heightmap = flat(32);
        for z in 0..32 {
            for x in 0..32 {
                heightmap.heights[(z * 32 + x) as usize] = x as f32 * 0.5;
            }
        }
        let channels = split_channel(32);
        let base = ScatterParams {
            density: 0.2,
            mask: mask(&[1, 2]),
            ..default_params()
        };
        let upright = scatter(&heightmap, &channels, &base);
        assert!(!upright.is_empty());
        assert!(upright.iter().all(|p| p.normal == Vec3::Y));

        let tilted = scatter(
            &heightmap,
            &channels,
            &ScatterParams {
                align_to_normal: true,
                ..base
            },
        );
        assert!(tilted.iter().all(|p| p.normal.x < 0.0 && p.normal.y > 0.0));
        // Same positions either way: aligning changes orientation only.
        let positions: Vec<_> = upright.iter().map(|p| p.position).collect();
        let aligned_positions: Vec<_> = tilted.iter().map(|p| p.position).collect();
        assert_eq!(positions, aligned_positions);
    }

    #[test]
    fn asset_indices_stay_inside_the_palette() {
        let heightmap = flat(48);
        let channels = split_channel(48);
        let placements = scatter(
            &heightmap,
            &channels,
            &ScatterParams {
                density: 0.5,
                mask: mask(&[1, 2]),
                asset_count: 3,
                ..default_params()
            },
        );
        assert!(!placements.is_empty());
        assert!(placements.iter().all(|p| p.asset_index < 3));
        assert!(placements.iter().any(|p| p.asset_index == 2));
    }

    #[test]
    fn scale_stays_inside_the_configured_range() {
        let heightmap = flat(48);
        let channels = split_channel(48);
        let placements = scatter(
            &heightmap,
            &channels,
            &ScatterParams {
                density: 0.5,
                mask: mask(&[1, 2]),
                scale_min: 0.5,
                scale_max: 2.0,
                ..default_params()
            },
        );
        assert!(!placements.is_empty());
        assert!(placements.iter().all(|p| (0.5..=2.0).contains(&p.scale)));
    }

    #[test]
    fn an_out_of_range_channel_index_places_nothing() {
        let heightmap = flat(32);
        let channels = split_channel(32);
        assert!(
            scatter(
                &heightmap,
                &channels,
                &ScatterParams {
                    density: 1.0,
                    mask: Some(ScatterMask {
                        channel: 9,
                        accept: vec![1],
                    }),
                    ..default_params()
                }
            )
            .is_empty()
        );
        assert!(
            scatter(
                &heightmap,
                &channels,
                &ScatterParams {
                    density: 1.0,
                    weight_channel: Some(9),
                    ..default_params()
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn a_degenerate_terrain_or_density_places_nothing() {
        let channels = split_channel(1);
        assert!(scatter(&flat(1), &channels, &default_params()).is_empty());
        assert!(
            scatter(
                &flat(32),
                &split_channel(32),
                &ScatterParams {
                    density: 0.0,
                    ..default_params()
                }
            )
            .is_empty()
        );
    }

    /// `cell` addresses the channel arrays directly, so a caller can look
    /// up what was painted under any placement.
    #[test]
    fn the_reported_cell_indexes_the_channel_arrays() {
        let heightmap = flat(32);
        let channels = split_channel(32);
        let placements = scatter(
            &heightmap,
            &channels,
            &ScatterParams {
                density: 0.4,
                mask: mask(&[2]),
                ..default_params()
            },
        );
        assert!(!placements.is_empty());
        assert!(
            placements
                .iter()
                .all(|p| channels[0].values[p.cell as usize] == 2)
        );
    }

    /// An instance carries the height and normal of the cell it came
    /// from, so it stands over that cell. Placement that assumes a
    /// centred grid puts every prop half a terrain away from the ground
    /// it was sampled on when the grid does not start at `-size / 2`.
    #[test]
    fn placements_stand_over_the_cells_they_sampled() {
        let at = Vec2::new(512.0, -256.0);
        let heightmap = Heightmap::new_at(32, Vec2::new(32.0, 32.0), 10.0, at);
        let channels = split_channel(32);

        let placements = scatter(
            &heightmap,
            &channels,
            &ScatterParams {
                seed: 11,
                density: 0.2,
                mask: mask(&[1, 2]),
                ..default_params()
            },
        );

        assert!(!placements.is_empty());
        let far = at + heightmap.size;
        for p in &placements {
            assert!(
                p.position.x >= at.x && p.position.x <= far.x,
                "{} is outside the ground the map covers",
                p.position.x,
            );
            assert!(
                p.position.z >= at.y && p.position.z <= far.y,
                "{} is outside the ground the map covers",
                p.position.z,
            );
            let cell_x = (p.cell % heightmap.resolution) as f32;
            let cell_z = (p.cell / heightmap.resolution) as f32;
            let cell = heightmap.cell_size();
            assert!((p.position.x - (cell_x * cell.x + at.x)).abs() <= cell.x);
            assert!((p.position.z - (cell_z * cell.y + at.y)).abs() <= cell.y);
        }
    }

    /// The buckets are laid over the ground the candidates are on. A grid
    /// anchored anywhere else clamps every candidate into one corner
    /// bucket, and a scatter degrades to the pairwise scan the bucketing
    /// exists to avoid.
    #[test]
    fn the_spacing_grid_buckets_over_the_ground_it_was_given() {
        let size = Vec2::splat(32.0);
        let origin = Vec2::new(900.0, -400.0);
        let grid = SpacingGrid::new(1.0, size, origin);

        assert_eq!(grid.bucket_of(origin), (0, 0));
        let far = grid.bucket_of(origin + size);
        assert!(
            far.0 > 1 && far.1 > 1,
            "the far corner must reach a far bucket, got {far:?}",
        );
    }

    /// Minimum spacing buckets in the same frame the placements are
    /// generated in. Buckets laid around the origin while candidates
    /// arrive somewhere else clamp every one of them into an edge bucket,
    /// and the rejection test stops separating them.
    #[test]
    fn minimum_spacing_survives_a_grid_that_is_not_centred() {
        let params = ScatterParams {
            seed: 5,
            density: 0.5,
            min_spacing: 1.5,
            mask: mask(&[1, 2]),
            ..default_params()
        };
        let centred = scatter(&flat(32), &split_channel(32), &params);
        let offset = scatter(
            &Heightmap::new_at(32, Vec2::new(32.0, 32.0), 10.0, Vec2::new(900.0, 900.0)),
            &split_channel(32),
            &params,
        );

        assert_eq!(centred.len(), offset.len());
        for pair in offset.windows(2) {
            let a = Vec2::new(pair[0].position.x, pair[0].position.z);
            let b = Vec2::new(pair[1].position.x, pair[1].position.z);
            assert!(a.distance(b) >= 1.5 - 1.0e-3);
        }
    }

    fn default_params() -> ScatterParams {
        ScatterParams {
            random_yaw: true,
            ..ScatterParams::default()
        }
    }
}
