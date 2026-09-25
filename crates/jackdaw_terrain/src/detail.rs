//! Instance placement for a terrain's detail layers over their density channels.
//! Placement is a pure function of `(seed, layer, cell, index)`.

use bevy_math::{IVec2, Vec2, Vec3};
use bytemuck::{Pod, Zeroable};
use jackdaw_scene_types::DetailLayer;

use crate::heightmap::Heightmap;

/// Steepest ground detail grows on, in radians. A steeper cell is bare.
const MAX_SLOPE: f32 = 40.0 * std::f32::consts::PI / 180.0;

/// World units one tile of the height and tint variation spans.
const VARIATION_TILE: f32 = 9.0;

/// Share of an instance's height that is its own rather than its patch's, in `0..1`.
const HEIGHT_JITTER: f32 = 0.25;

/// Share of an instance's tint that is its own rather than its patch's, in `0..1`.
const TINT_JITTER: f32 = 0.3;

/// Lattice cells before the variation field repeats.
const VARIATION_PERIOD: f32 = 4096.0;

/// One instance, as the vertex buffer holds it. The four variation bytes are
/// normalized; the shader maps each into the range the layer declares.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct DetailInstance {
    /// Terrain-local position of the instance's foot.
    pub position: Vec3,
    /// Height, width, yaw and tint, one byte each from the low byte up.
    pub packed: u32,
    /// X and Z of the ground normal the instance stands along, or zero for one
    /// standing straight up.
    pub tilt: Vec2,
}

impl DetailInstance {
    /// Pack the four variation bytes into one word.
    pub fn pack(height: u8, width: u8, yaw: u8, tint: u8) -> u32 {
        u32::from(height) | u32::from(width) << 8 | u32::from(yaw) << 16 | u32::from(tint) << 24
    }

    /// The four variation bytes, in the order [`Self::pack`] took them.
    pub fn unpack(self) -> [u8; 4] {
        self.packed.to_le_bytes()
    }
}

/// How finely one tile of detail is seeded. A far tile's instances are a
/// subset of a near tile's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DetailLod {
    Near,
    Far,
}

impl DetailLod {
    /// Fraction of full density this level seeds at.
    pub fn density_scale(self) -> f32 {
        match self {
            Self::Near => 1.0,
            Self::Far => 0.3,
        }
    }
}

/// Half-width of the dead zone either side of the detail boundary, as a
/// fraction of the near band.
const LOD_HYSTERESIS: f32 = 0.15;

/// Distance from a viewer to the centre of a tile, in cells.
pub fn tile_centre_distance(viewer_cell: IVec2, tile: IVec2, tile_cells: u32) -> f32 {
    let side = tile_cells as f32;
    let centre = tile.as_vec2() * side + Vec2::splat(side * 0.5);
    viewer_cell.as_vec2().distance(centre)
}

/// The detail level a tile whose centre is `centre_distance` cells away takes.
/// A tile already seeded at `held` keeps it until a margin past the boundary.
pub fn detail_lod_at(centre_distance: f32, cull_cells: f32, held: Option<DetailLod>) -> DetailLod {
    let band = cull_cells * 0.5;
    let margin = band * LOD_HYSTERESIS;
    let boundary = match held {
        Some(DetailLod::Near) => band + margin,
        Some(DetailLod::Far) => band - margin,
        None => band,
    };
    if centre_distance <= boundary {
        DetailLod::Near
    } else {
        DetailLod::Far
    }
}

/// The tiles of detail around a viewer with any part inside `cull_cells`,
/// nearest first, each with the level it takes when not yet seeded.
pub fn detail_tiles_around(
    viewer_cell: IVec2,
    cull_cells: f32,
    tile_cells: u32,
) -> Vec<(IVec2, DetailLod)> {
    if tile_cells == 0 || !cull_cells.is_finite() || cull_cells <= 0.0 {
        return Vec::new();
    }
    let side = tile_cells as f32;
    let span = (cull_cells / side).ceil() as i32 + 1;
    let base = IVec2::new(
        viewer_cell.x.div_euclid(tile_cells as i32),
        viewer_cell.y.div_euclid(tile_cells as i32),
    );
    let viewer = viewer_cell.as_vec2();

    let mut tiles = Vec::new();
    for tz in -span..=span {
        for tx in -span..=span {
            let tile = base + IVec2::new(tx, tz);
            let min = tile.as_vec2() * side;
            let max = min + Vec2::splat(side);
            let nearest = viewer.clamp(min, max);
            if viewer.distance(nearest) > cull_cells {
                continue;
            }
            let centre_distance = tile_centre_distance(viewer_cell, tile, tile_cells);
            tiles.push((
                tile,
                detail_lod_at(centre_distance, cull_cells, None),
                centre_distance,
            ));
        }
    }
    tiles.sort_by(|a, b| {
        a.2.total_cmp(&b.2)
            .then_with(|| (a.0.x, a.0.y).cmp(&(b.0.x, b.0.y)))
    });
    tiles
        .into_iter()
        .map(|(tile, lod, _)| (tile, lod))
        .collect()
}

/// Which of a layer's varieties an instance draws, by a draw against their
/// weights that depends only on the instance itself: every reseed of a tile
/// gives each instance the same variety, and changing the varieties moves no
/// instance. `None` when no variety has weight.
pub fn detail_variety(instance: &DetailInstance, weights: &[f32]) -> Option<usize> {
    let total: f32 = weights.iter().map(|weight| weight.max(0.0)).sum();
    if !total.is_finite() || total <= 0.0 {
        return None;
    }
    let position = instance.position;
    let key = mix64(
        u64::from(position.x.to_bits())
            ^ mix64(u64::from(position.z.to_bits()).rotate_left(32))
            ^ mix64(u64::from(instance.packed).wrapping_add(0x5851_f42d)),
    );
    let mut roll = unit(key) * total;
    let mut last = None;
    for (index, weight) in weights.iter().enumerate() {
        let weight = weight.max(0.0);
        if weight <= 0.0 {
            continue;
        }
        last = Some(index);
        if roll < weight {
            return Some(index);
        }
        roll -= weight;
    }
    last
}

/// Seed one tile of ground with a layer's instances, in terrain-local space.
/// `density` is row-major at the heightmap's resolution, with `max` its ceiling.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is one dimension of the field, and none group"
)]
pub fn place_detail(
    density: &[u16],
    max: u16,
    heightmap: &Heightmap,
    layer: &DetailLayer,
    layer_index: usize,
    tile: IVec2,
    tile_cells: u32,
    cell_size: f32,
    density_scale: f32,
    lod_scale: f32,
    seed: u64,
) -> Vec<DetailInstance> {
    let resolution = heightmap.resolution;
    let density_per_m2 = layer.density_per_m2 * density_scale;
    if tile_cells == 0
        || max == 0
        || resolution == 0
        || !cell_size.is_finite()
        || cell_size <= 0.0
        || !density_per_m2.is_finite()
        || density_per_m2 <= 0.0
        || !lod_scale.is_finite()
        || lod_scale <= 0.0
        || density.len() != (resolution as usize) * (resolution as usize)
    {
        return Vec::new();
    }

    let per_cell = density_per_m2 * cell_size * cell_size * lod_scale;
    let origin = tile * tile_cells as i32;
    let layer_seed = seed ^ mix64(layer_index as u64);
    let mut placed = Vec::new();

    for lz in 0..tile_cells as i32 {
        for lx in 0..tile_cells as i32 {
            let gx = origin.x + lx;
            let gz = origin.y + lz;
            if gx < 0 || gz < 0 || gx >= resolution as i32 || gz >= resolution as i32 {
                continue;
            }
            let coverage = f32::from(density[gz as usize * resolution as usize + gx as usize])
                / f32::from(max);
            if coverage <= 0.0 {
                continue;
            }
            let gradient = cell_gradient(heightmap, gx, gz, cell_size);
            if gradient.length().atan() > MAX_SLOPE {
                continue;
            }
            let tilt = match layer.align_to_normal {
                true => ground_tilt(gradient),
                false => Vec2::ZERO,
            };

            let wanted = per_cell * coverage;
            let cell_seed = mix64(
                layer_seed ^ mix64(gx as i64 as u64) ^ mix64((gz as i64 as u64).rotate_left(32)),
            );
            let count = stochastic_round(wanted, cell_seed);

            for k in 0..count {
                let h = mix64(cell_seed ^ mix64(u64::from(k).wrapping_add(0x9e37_79b9)));
                let jitter = Vec2::new(unit(h), unit(h.rotate_left(17)));
                let grid = Vec2::new(gx as f32, gz as f32) + jitter;
                let local = heightmap.origin + grid * cell_size;
                let position =
                    Vec3::new(local.x, heightmap.sample_bilinear(grid.x, grid.y), local.y);
                let variation = tileable_value_noise(local / VARIATION_TILE, VARIATION_PERIOD);
                let height = to_byte(blend(variation, unit(h.rotate_left(53)), HEIGHT_JITTER));
                let width = to_byte(unit(h.rotate_left(41)));
                let yaw = to_byte(unit(h.rotate_left(5)));
                let tint = to_byte(blend(variation, unit(h.rotate_left(29)), TINT_JITTER));
                placed.push(DetailInstance {
                    position,
                    packed: DetailInstance::pack(height, width, yaw, tint),
                    tilt,
                });
            }
        }
    }
    placed
}

/// A patch value and an instance's own, mixed `amount` of the way toward the
/// instance, in `0..1`.
fn blend(patch: f32, instance: f32, amount: f32) -> f32 {
    (patch * (1.0 - amount) + instance * amount).clamp(0.0, 1.0)
}

/// Height change per world unit along X and Z at one cell.
fn cell_gradient(heightmap: &Heightmap, gx: i32, gz: i32, cell_size: f32) -> Vec2 {
    let at = |x: i32, z: i32| {
        let x = x.clamp(0, heightmap.resolution as i32 - 1) as u32;
        let z = z.clamp(0, heightmap.resolution as i32 - 1) as u32;
        heightmap.get_height(x, z)
    };
    let dx = (at(gx + 1, gz) - at(gx - 1, gz)) / (2.0 * cell_size);
    let dz = (at(gx, gz + 1) - at(gx, gz - 1)) / (2.0 * cell_size);
    Vec2::new(dx, dz)
}

/// X and Z of the unit ground normal a gradient stands under.
fn ground_tilt(gradient: Vec2) -> Vec2 {
    let normal = Vec3::new(-gradient.x, 1.0, -gradient.y).normalize_or(Vec3::Y);
    Vec2::new(normal.x, normal.z)
}

/// Value noise on a lattice that wraps every `period` units, in `0..1`.
pub fn tileable_value_noise(p: Vec2, period: f32) -> f32 {
    if !period.is_finite() || period <= 0.0 {
        return 0.5;
    }
    let x0 = p.x.floor();
    let z0 = p.y.floor();
    let fx = smoothstep(p.x - x0);
    let fz = smoothstep(p.y - z0);
    let wrap = |v: f32| v.rem_euclid(period) as i64 as u64;
    let lattice = |x: f32, z: f32| unit(mix64(wrap(x) ^ mix64(wrap(z).rotate_left(32))));

    let n00 = lattice(x0, z0);
    let n10 = lattice(x0 + 1.0, z0);
    let n01 = lattice(x0, z0 + 1.0);
    let n11 = lattice(x0 + 1.0, z0 + 1.0);
    let top = n00 + (n10 - n00) * fx;
    let bottom = n01 + (n11 - n01) * fx;
    top + (bottom - top) * fz
}

/// Hermite ease over `0..1`.
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A `0..1` value as the byte the vertex buffer carries.
fn to_byte(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// `wanted` as a whole count, its fraction spent as a draw against `seed` so
/// that a field of cells averages out to `wanted`.
fn stochastic_round(wanted: f32, seed: u64) -> u32 {
    wanted.floor() as u32 + u32::from(unit(seed) < wanted.fract())
}

/// Top 24 bits of a hash as a `0..1` float.
fn unit(h: u64) -> f32 {
    (h >> 40) as f32 / 16_777_216.0
}

/// Integer avalanche hash, identical on every platform.
fn mix64(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^= x >> 33;
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(resolution: u32) -> Heightmap {
        Heightmap::new_at(
            resolution,
            Vec2::splat((resolution - 1) as f32),
            50.0,
            Vec2::ZERO,
        )
    }

    fn full(resolution: u32) -> Vec<u16> {
        vec![255; (resolution as usize) * (resolution as usize)]
    }

    fn layer(density_per_m2: f32) -> DetailLayer {
        DetailLayer {
            density_per_m2,
            ..DetailLayer::default()
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one call shape for every placement test"
    )]
    fn place(
        density: &[u16],
        heightmap: &Heightmap,
        layer: &DetailLayer,
        layer_index: usize,
        tile: IVec2,
        tile_cells: u32,
        lod_scale: f32,
        seed: u64,
    ) -> Vec<DetailInstance> {
        place_detail(
            density,
            255,
            heightmap,
            layer,
            layer_index,
            tile,
            tile_cells,
            1.0,
            1.0,
            lod_scale,
            seed,
        )
    }

    #[test]
    fn an_instance_draws_the_same_variety_every_time_and_the_mix_follows_the_weights() {
        let heightmap = flat(64);
        let density = full(64);
        let placed = place(
            &density,
            &heightmap,
            &layer(24.0),
            0,
            IVec2::ZERO,
            32,
            1.0,
            9,
        );
        assert!(placed.len() > 2000, "enough instances to count shares");
        let weights = [3.0, 1.0, 0.0];
        let mut counts = [0usize; 3];
        for instance in &placed {
            let picked = detail_variety(instance, &weights).expect("a variety has weight");
            assert_eq!(detail_variety(instance, &weights), Some(picked), "stable");
            counts[picked] += 1;
        }
        assert_eq!(counts[2], 0, "a variety of weight zero draws nothing");
        let share = counts[0] as f32 / placed.len() as f32;
        assert!(
            (share - 0.75).abs() < 0.05,
            "three to one comes out near three quarters, not {share}"
        );
    }

    #[test]
    fn no_variety_with_weight_draws_nothing() {
        let instance = DetailInstance {
            position: Vec3::ONE,
            packed: 7,
            tilt: Vec2::ZERO,
        };
        assert_eq!(detail_variety(&instance, &[]), None);
        assert_eq!(detail_variety(&instance, &[0.0, -1.0]), None);
        assert_eq!(detail_variety(&instance, &[0.0, 2.0]), Some(1));
    }

    #[test]
    fn the_same_tile_and_seed_place_the_same_instances() {
        let heightmap = flat(32);
        let density = full(32);
        let grown = layer(12.0);
        let once = place(&density, &heightmap, &grown, 0, IVec2::new(1, 1), 8, 1.0, 7);
        let twice = place(&density, &heightmap, &grown, 0, IVec2::new(1, 1), 8, 1.0, 7);
        assert!(!once.is_empty());
        assert_eq!(once, twice);

        let elsewhere = place(&density, &heightmap, &grown, 0, IVec2::new(1, 1), 8, 1.0, 8);
        assert_ne!(once, elsewhere, "a different seed places a different field");
    }

    #[test]
    fn two_layers_of_one_terrain_place_different_fields() {
        let heightmap = flat(32);
        let density = full(32);
        let grown = layer(12.0);
        let first = place(&density, &heightmap, &grown, 0, IVec2::ZERO, 8, 1.0, 5);
        let second = place(&density, &heightmap, &grown, 1, IVec2::ZERO, 8, 1.0, 5);
        assert!(!first.is_empty());
        assert_ne!(first, second, "a second layer places different instances");
    }

    #[test]
    fn instance_count_scales_with_density_and_detail_level() {
        let heightmap = flat(32);
        let density = full(32);
        let sparse = place(&density, &heightmap, &layer(4.0), 0, IVec2::ZERO, 8, 1.0, 3);
        let dense = place(
            &density,
            &heightmap,
            &layer(16.0),
            0,
            IVec2::ZERO,
            8,
            1.0,
            3,
        );
        assert!(
            dense.len() > sparse.len() * 2,
            "{} instances at four times the density beats {}",
            dense.len(),
            sparse.len()
        );

        let far = place(
            &density,
            &heightmap,
            &layer(16.0),
            0,
            IVec2::ZERO,
            8,
            DetailLod::Far.density_scale(),
            3,
        );
        assert!(far.len() < dense.len());
        assert!(!far.is_empty());
        for instance in &far {
            assert!(
                dense.contains(instance),
                "a far tile keeps instances the near tile also placed"
            );
        }
    }

    #[test]
    fn a_cell_with_no_density_grows_nothing() {
        let heightmap = flat(16);
        let mut density = vec![0u16; 16 * 16];
        let grown = layer(20.0);
        assert!(place(&density, &heightmap, &grown, 0, IVec2::ZERO, 8, 1.0, 1).is_empty());

        density[3 * 16 + 3] = 255;
        let placed = place(&density, &heightmap, &grown, 0, IVec2::ZERO, 8, 1.0, 1);
        assert!(!placed.is_empty());
        for instance in &placed {
            assert!(
                (3.0..4.0).contains(&instance.position.x)
                    && (3.0..4.0).contains(&instance.position.z),
                "every instance stands on the one cell that has density"
            );
        }
    }

    #[test]
    fn instances_in_one_patch_do_not_stand_level() {
        let heightmap = flat(16);
        let density = full(16);
        let cells_within_one_variation_tile = 4;
        let placed = place(
            &density,
            &heightmap,
            &layer(40.0),
            0,
            IVec2::ZERO,
            cells_within_one_variation_tile,
            1.0,
            11,
        );
        assert!(placed.len() > 20);

        let heights: Vec<u8> = placed.iter().map(|blade| blade.unpack()[0]).collect();
        let tints: Vec<u8> = placed.iter().map(|blade| blade.unpack()[3]).collect();
        let spread = |bytes: &[u8]| {
            let low = *bytes.iter().min().expect("instances were placed");
            let high = *bytes.iter().max().expect("instances were placed");
            high - low
        };
        assert!(
            spread(&heights) > 20,
            "every instance stands the same height"
        );
        assert!(spread(&tints) > 20, "every instance reads the same colour");
    }

    #[test]
    fn a_steep_cell_grows_nothing_while_the_flat_ground_beside_it_does() {
        let mut heightmap = flat(16);
        for z in 0..16 {
            for x in 8..16 {
                heightmap.set_height(x, z, (x - 8) as f32 * 3.0);
            }
        }
        let density = full(16);
        let placed = place(
            &density,
            &heightmap,
            &layer(20.0),
            0,
            IVec2::ZERO,
            16,
            1.0,
            2,
        );
        assert!(!placed.is_empty());
        for instance in &placed {
            assert!(
                instance.position.x < 8.0,
                "no instance stands on the slope, but one is at {}",
                instance.position.x
            );
        }
        assert!(
            placed.iter().any(|instance| instance.position.x > 6.0),
            "the flat ground beside the slope still grows"
        );
    }

    #[test]
    fn a_layer_that_aligns_to_the_normal_leans_on_a_slope() {
        let mut heightmap = flat(16);
        for z in 0..16 {
            for x in 0..16 {
                heightmap.set_height(x, z, x as f32 * 0.5);
            }
        }
        let density = full(16);
        let upright = layer(20.0);
        let aligned = DetailLayer {
            align_to_normal: true,
            ..upright.clone()
        };

        let standing = place(&density, &heightmap, &upright, 0, IVec2::ZERO, 8, 1.0, 4);
        let leaning = place(&density, &heightmap, &aligned, 0, IVec2::ZERO, 8, 1.0, 4);
        assert!(!leaning.is_empty());
        assert!(standing.iter().all(|instance| instance.tilt == Vec2::ZERO));
        assert!(
            leaning.iter().all(|instance| instance.tilt.x < -0.1),
            "every instance leans down the rising X gradient"
        );
        assert!(leaning.iter().all(|instance| instance.tilt.y.abs() < 1e-5));
    }

    #[test]
    fn tiles_cover_the_cull_radius_and_stop_there() {
        let tiles = detail_tiles_around(IVec2::new(64, 64), 40.0, 16);
        assert!(!tiles.is_empty());
        for (tile, _) in &tiles {
            let min = tile.as_vec2() * 16.0;
            let nearest = Vec2::splat(64.0).clamp(min, min + Vec2::splat(16.0));
            assert!(
                Vec2::splat(64.0).distance(nearest) <= 40.0,
                "tile {tile} is inside the cull radius"
            );
        }
        assert!(tiles.iter().any(|(tile, _)| *tile == IVec2::new(4, 4)));
        assert!(!tiles.iter().any(|(tile, _)| *tile == IVec2::new(9, 9)));

        let wider = detail_tiles_around(IVec2::new(64, 64), 80.0, 16);
        assert!(wider.len() > tiles.len());
    }

    #[test]
    fn tiles_are_listed_nearest_first_and_coarsen_with_distance() {
        let middle_of_tile_4_4 = IVec2::new(72, 72);
        let tiles = detail_tiles_around(middle_of_tile_4_4, 64.0, 16);
        assert_eq!(tiles[0].0, IVec2::new(4, 4));
        assert_eq!(tiles[0].1, DetailLod::Near);
        assert_eq!(
            tiles.last().expect("the list is not empty").1,
            DetailLod::Far
        );
        assert!(tiles.iter().any(|(_, lod)| *lod == DetailLod::Near));
    }

    #[test]
    fn a_seeded_tile_keeps_its_level_across_the_boundary() {
        let cull = 64.0;
        let band = cull * 0.5;
        assert_eq!(detail_lod_at(band - 0.01, cull, None), DetailLod::Near);
        assert_eq!(detail_lod_at(band + 0.01, cull, None), DetailLod::Far);

        assert_eq!(
            detail_lod_at(band + 0.01, cull, Some(DetailLod::Near)),
            DetailLod::Near,
            "a near tile nudged past the boundary stays near"
        );
        assert_eq!(
            detail_lod_at(band - 0.01, cull, Some(DetailLod::Far)),
            DetailLod::Far,
            "a far tile nudged inside the boundary stays far"
        );

        let margin = band * LOD_HYSTERESIS;
        assert_eq!(
            detail_lod_at(band + margin * 2.0, cull, Some(DetailLod::Near)),
            DetailLod::Far,
            "past the margin it does change"
        );
        assert_eq!(
            detail_lod_at(band - margin * 2.0, cull, Some(DetailLod::Far)),
            DetailLod::Near
        );
    }

    #[test]
    fn a_viewer_with_no_cull_radius_has_no_tiles() {
        assert!(detail_tiles_around(IVec2::ZERO, 0.0, 16).is_empty());
        assert!(detail_tiles_around(IVec2::ZERO, 40.0, 0).is_empty());
    }

    #[test]
    fn the_variation_bytes_survive_the_packed_word() {
        let instance = DetailInstance {
            position: Vec3::new(1.0, 2.0, 3.0),
            packed: DetailInstance::pack(9, 200, 17, 255),
            tilt: Vec2::ZERO,
        };
        assert_eq!(instance.unpack(), [9, 200, 17, 255]);
        assert_eq!(size_of::<DetailInstance>(), 24);
    }

    #[test]
    fn the_value_noise_wraps_across_its_period() {
        for at in [0.0, 0.25, 1.5, 3.75] {
            let inside = tileable_value_noise(Vec2::new(at, at), 8.0);
            let wrapped = tileable_value_noise(Vec2::new(at + 8.0, at + 8.0), 8.0);
            assert!(
                (inside - wrapped).abs() < 1e-5,
                "{inside} at {at} should repeat as {wrapped}"
            );
        }
    }
}
