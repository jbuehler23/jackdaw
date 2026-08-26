use bevy_math::{Vec2, Vec3};

/// Where a ray met the surface, in terrain-local space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceHit {
    /// Hit point relative to the terrain origin.
    pub local: Vec3,
    /// Fractional grid coordinate of [`Self::local`].
    pub grid: Vec2,
    /// Distance along the ray direction, which must be normalized for
    /// this to be a world distance.
    pub distance: f32,
}

/// Marching step as a fraction of a cell edge. A full cell per step
/// walks over a one-cell spike seen edge-on.
const STEPS_PER_CELL: f32 = 0.5;

/// Ceiling on marched steps. Past this the step coarsens instead of the
/// march running longer.
const MAX_MARCH_STEPS: u32 = 4096;

/// Bisection iterations refining a crossed interval. 24 takes a
/// cell-sized bracket below float resolution for any terrain the editor
/// can hold.
const REFINE_ITERATIONS: u32 = 24;

/// Pure heightmap data structure -- no Bevy ECS dependencies.
///
/// A dense window onto a terrain's cells: [`Self::resolution`] covers how
/// many, [`Self::origin`] where they sit. A terrain's cells live in sparse
/// regions and can reach further than any one map gathered from them.
#[derive(Clone, Debug)]
pub struct Heightmap {
    /// Vertices per edge.
    pub resolution: u32,
    /// World-space XZ dimensions.
    pub size: Vec2,
    /// Maximum height value for normalization.
    pub max_height: f32,
    /// Local-space position of grid vertex `(0, 0)`, relative to the
    /// terrain's own origin.
    pub origin: Vec2,
    /// Row-major height data, length = resolution^2.
    pub heights: Vec<f32>,
}

impl Default for Heightmap {
    fn default() -> Self {
        Self::new(256, Vec2::new(100.0, 100.0), 50.0)
    }
}

impl Heightmap {
    /// A map centred on the terrain's origin, for a caller with no stored
    /// cells to line up with. [`Self::new_at`] places one against cells
    /// that already exist.
    pub fn new(resolution: u32, size: Vec2, max_height: f32) -> Self {
        Self::new_at(resolution, size, max_height, -size / 2.0)
    }

    /// A map whose grid vertex `(0, 0)` sits at `origin`.
    pub fn new_at(resolution: u32, size: Vec2, max_height: f32, origin: Vec2) -> Self {
        Self {
            resolution,
            size,
            max_height,
            origin,
            heights: vec![0.0; (resolution * resolution) as usize],
        }
    }

    /// Get height at integer grid coordinates. Returns 0 if out of bounds.
    pub fn get_height(&self, x: u32, z: u32) -> f32 {
        if x >= self.resolution || z >= self.resolution {
            return 0.0;
        }
        self.heights[(z * self.resolution + x) as usize]
    }

    /// Set height at integer grid coordinates.
    pub fn set_height(&mut self, x: u32, z: u32, h: f32) {
        if x < self.resolution && z < self.resolution {
            self.heights[(z * self.resolution + x) as usize] = h;
        }
    }

    /// Convert a local-space position (relative to terrain origin) to fractional grid coordinates.
    pub fn world_to_grid(&self, local_pos: Vec2) -> Vec2 {
        let cell = self.cell_size();
        let offset = local_pos - self.origin;
        Vec2::new(offset.x / cell.x, offset.y / cell.y)
    }

    /// Bilinear interpolation of height at fractional grid coordinates.
    pub fn sample_bilinear(&self, gx: f32, gz: f32) -> f32 {
        let x0 = gx.floor() as i32;
        let z0 = gz.floor() as i32;
        let fx = gx - x0 as f32;
        let fz = gz - z0 as f32;

        let s = |x: i32, z: i32| -> f32 {
            let x = x.clamp(0, self.resolution as i32 - 1) as u32;
            let z = z.clamp(0, self.resolution as i32 - 1) as u32;
            self.get_height(x, z)
        };

        let h00 = s(x0, z0);
        let h10 = s(x0 + 1, z0);
        let h01 = s(x0, z0 + 1);
        let h11 = s(x0 + 1, z0 + 1);

        let h0 = h00 * (1.0 - fx) + h10 * fx;
        let h1 = h01 * (1.0 - fx) + h11 * fx;
        h0 * (1.0 - fz) + h1 * fz
    }

    /// Number of chunks along each axis given a chunk cell size.
    ///
    /// `(0, 0)` for a map covering no cells.
    pub fn chunk_count(&self, chunk_size: u32) -> (u32, u32) {
        let Some(cells) = self.cells_per_axis() else {
            return (0, 0);
        };
        let cx = cells.div_ceil(chunk_size);
        let cz = cells.div_ceil(chunk_size);
        (cx, cz)
    }

    /// World-space size of one grid cell.
    ///
    /// `Vec2::ZERO` for a map covering no cells.
    pub fn cell_size(&self) -> Vec2 {
        let Some(cells) = self.cells_per_axis() else {
            return Vec2::ZERO;
        };
        Vec2::new(self.size.x / cells as f32, self.size.y / cells as f32)
    }

    /// Cells along each axis, or `None` when the map spans no cell at
    /// all: one vertex is a point, not a cell.
    fn cells_per_axis(&self) -> Option<u32> {
        (self.resolution >= 2).then(|| self.resolution - 1)
    }

    /// Ground slope at every grid point, in radians, row-major over the
    /// grid.
    ///
    /// Central differences across one cell of the stored grid: the finest
    /// slope the map holds, and independent of how coarsely a later
    /// sampler steps. A surface meshed at a wider step reads a gentler
    /// angle, and a different one per step size.
    ///
    /// An edge point differences against itself on the missing side and
    /// divides by the one cell it spans, so a constant ramp reads the same
    /// angle along its border as in its middle.
    pub fn slope_field(&self) -> Vec<f32> {
        let res = self.resolution;
        let mut out = vec![0.0; (res as usize) * (res as usize)];
        let Some(last) = res.checked_sub(1).filter(|_| res >= 2) else {
            return out;
        };
        let cell = self.cell_size();
        for z in 0..res {
            for x in 0..res {
                let low_x = x.saturating_sub(1);
                let high_x = (x + 1).min(last);
                let low_z = z.saturating_sub(1);
                let high_z = (z + 1).min(last);
                let dx = (self.get_height(high_x, z) - self.get_height(low_x, z))
                    / ((high_x - low_x) as f32 * cell.x);
                let dz = (self.get_height(x, high_z) - self.get_height(x, low_z))
                    / ((high_z - low_z) as f32 * cell.y);
                let slope = (dx * dx + dz * dz).sqrt().atan();
                out[(z * res + x) as usize] = if slope.is_finite() { slope } else { 0.0 };
            }
        }
        out
    }

    /// Lowest and highest stored height. `(0.0, 0.0)` when there are no
    /// heights at all.
    ///
    /// A non-finite height folds in as `0.0`: both accumulators start at
    /// `0.0` and only move away from it, so `0.0` widens neither.
    pub fn height_bounds(&self) -> (f32, f32) {
        let mut low = 0.0f32;
        let mut high = 0.0f32;
        for &h in &self.heights {
            let h = if h.is_finite() { h } else { 0.0 };
            low = low.min(h);
            high = high.max(h);
        }
        (low, high)
    }

    /// First point where a ray meets the sculpted surface, in
    /// terrain-local space (the terrain's origin at `Vec3::ZERO`).
    ///
    /// A crossing in either direction counts, so a ray starting under the
    /// surface (a camera inside a hill, or below the terrain looking up)
    /// reports where it breaks out rather than reporting nothing.
    pub fn raycast(&self, origin: Vec3, dir: Vec3) -> Option<SurfaceHit> {
        self.raycast_within(origin, dir, self.height_bounds())
    }

    /// [`Self::raycast`], against a height range already scanned.
    ///
    /// Finding that range costs a pass over every stored height, once per
    /// cast. A caller holding a range it knows still matches these heights
    /// passes it here instead. A range narrower than the heights clips the
    /// march and loses the hits outside it.
    pub fn raycast_within(
        &self,
        origin: Vec3,
        dir: Vec3,
        bounds: (f32, f32),
    ) -> Option<SurfaceHit> {
        let dir = dir.normalize_or_zero();
        if dir == Vec3::ZERO {
            return None;
        }
        let (enter, exit) = self.clip_to_bounds(origin, dir, bounds)?;

        let cell = self.cell_size();
        let span = exit - enter;
        // Coarsens rather than iterating without limit when the span is
        // very wide relative to a cell.
        let step = (cell.min_element() * STEPS_PER_CELL)
            .max(span / MAX_MARCH_STEPS as f32)
            .max(f32::MIN_POSITIVE);

        let mut prev_t = enter;
        let mut prev = self.height_delta(origin, dir, prev_t);
        if prev == 0.0 {
            return Some(self.surface_hit(origin, dir, prev_t));
        }
        let mut t = enter;
        while t < exit {
            t = (t + step).min(exit);
            let delta = self.height_delta(origin, dir, t);
            if delta == 0.0 || (delta > 0.0) != (prev > 0.0) {
                let crossing = self.refine_crossing(origin, dir, prev_t, prev > 0.0, t);
                return Some(self.surface_hit(origin, dir, crossing));
            }
            prev_t = t;
            prev = delta;
        }
        None
    }

    /// Signed height of the ray above the surface at `t`.
    fn height_delta(&self, origin: Vec3, dir: Vec3, t: f32) -> f32 {
        let p = origin + dir * t;
        let grid = self.world_to_grid(Vec2::new(p.x, p.z));
        p.y - self.sample_bilinear(grid.x, grid.y)
    }

    /// Bisect a bracketed crossing. `lo_above` is which side of the
    /// surface `lo` sits on, so this refines a crossing in either
    /// direction. Returns the bracket's midpoint rather than an endpoint,
    /// so a ray meeting a cell boundary lands the same way whichever
    /// neighbouring cell the march happened to sample from.
    fn refine_crossing(
        &self,
        origin: Vec3,
        dir: Vec3,
        mut lo: f32,
        lo_above: bool,
        hi: f32,
    ) -> f32 {
        let mut hi = hi;
        for _ in 0..REFINE_ITERATIONS {
            let mid = 0.5 * (lo + hi);
            if mid <= lo || mid >= hi {
                break;
            }
            if (self.height_delta(origin, dir, mid) > 0.0) == lo_above {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        0.5 * (lo + hi)
    }

    fn surface_hit(&self, origin: Vec3, dir: Vec3, t: f32) -> SurfaceHit {
        let local = origin + dir * t;
        SurfaceHit {
            local,
            grid: self.world_to_grid(Vec2::new(local.x, local.z)),
            distance: t,
        }
    }

    /// Clip a ray to the box the surface lives in: the terrain's XZ
    /// footprint by its own height range. Bounds the march and rejects a
    /// ray that misses the terrain outright.
    fn clip_to_bounds(&self, origin: Vec3, dir: Vec3, bounds: (f32, f32)) -> Option<(f32, f32)> {
        let (low, high) = bounds;
        // Padded so a perfectly flat terrain still presents a slab with
        // depth for the march to cross, rather than a degenerate plane.
        let pad = self.cell_size().max_element().max((high - low) * 1e-3);
        let far = self.origin + self.size;
        let min = Vec3::new(self.origin.x, low - pad, self.origin.y);
        let max = Vec3::new(far.x, high + pad, far.y);

        // Starts at 0: a hit behind the camera is not something the
        // cursor is pointing at.
        let mut enter = 0.0f32;
        let mut exit = f32::INFINITY;
        for axis in 0..3 {
            if dir[axis].abs() <= f32::EPSILON {
                if origin[axis] < min[axis] || origin[axis] > max[axis] {
                    return None;
                }
                continue;
            }
            let a = (min[axis] - origin[axis]) / dir[axis];
            let b = (max[axis] - origin[axis]) / dir[axis];
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
            if enter > exit {
                return None;
            }
        }
        exit.is_finite().then_some((enter, exit))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_math::Vec3Swizzles;

    /// 64x64 world units at one unit per cell, centred on the origin, so a
    /// local coordinate and a grid coordinate differ only by the +32 shift.
    fn flat() -> Heightmap {
        Heightmap::new(65, Vec2::splat(64.0), 32.0)
    }

    /// Spacing and chunking both answer nothing rather than wrapping
    /// around on unsigned arithmetic, which would turn an empty terrain
    /// into a four-billion-iteration loop.
    #[test]
    fn a_map_over_no_cells_has_no_spacing_and_no_chunks() {
        for resolution in [0, 1] {
            let map = Heightmap::new(resolution, Vec2::ZERO, 32.0);
            assert_eq!(map.cell_size(), Vec2::ZERO, "resolution {resolution}");
            assert_eq!(map.chunk_count(32), (0, 0), "resolution {resolution}");
        }
    }

    /// A ramp of `rise` world units per world unit of +X, over a map of
    /// `resolution` grid points across `size` world units.
    fn ramp(resolution: u32, size: f32, rise: f32) -> Heightmap {
        let mut map = Heightmap::new(resolution, Vec2::splat(size), 32.0);
        let cell = map.cell_size();
        for z in 0..resolution {
            for x in 0..resolution {
                map.set_height(x, z, x as f32 * cell.x * rise);
            }
        }
        map
    }

    #[test]
    fn ground_nobody_has_sculpted_is_flat_everywhere() {
        let slopes = flat().slope_field();

        assert_eq!(slopes.len(), 65 * 65);
        assert!(slopes.iter().all(|s| *s == 0.0), "{slopes:?}");
    }

    /// Including the border: an edge point differences against itself on
    /// the missing side, and dividing by the one cell it spans keeps that
    /// from reading as half the slope.
    #[test]
    fn a_ramp_reads_its_own_angle_at_every_point_border_included() {
        let map = ramp(33, 64.0, 1.0);

        for (i, slope) in map.slope_field().iter().enumerate() {
            assert!(
                (slope - core::f32::consts::FRAC_PI_4).abs() < 1e-5,
                "point {i} read {slope}",
            );
        }
    }

    /// The same ground at two grid spacings: a point's slope is a property
    /// of the ground, not of the sampling step.
    #[test]
    fn the_same_world_ramp_reads_the_same_angle_at_two_cell_sizes() {
        let coarse = ramp(9, 64.0, 0.5).slope_field();
        let fine = ramp(129, 64.0, 0.5).slope_field();

        let expected = 0.5f32.atan();
        assert!((coarse[coarse.len() / 2] - expected).abs() < 1e-5);
        assert!((fine[fine.len() / 2] - expected).abs() < 1e-5);
    }

    /// Both axes at once, against the angle of the steepest line down a
    /// plane tilted along X and Z together.
    #[test]
    fn a_plane_tilted_on_both_axes_reads_its_steepest_line() {
        let mut map = Heightmap::new(17, Vec2::splat(16.0), 32.0);
        for z in 0..17 {
            for x in 0..17 {
                map.set_height(x, z, x as f32 * 0.75 + z as f32 * 1.0);
            }
        }

        let expected = (0.75f32 * 0.75 + 1.0).sqrt().atan();
        for (i, slope) in map.slope_field().iter().enumerate() {
            assert!((slope - expected).abs() < 1e-5, "point {i} read {slope}");
        }
    }

    /// A map over no cells has no spacing to difference across, and the
    /// field is asked for on every terrain including an empty one.
    #[test]
    fn a_map_over_no_cells_has_a_slope_for_each_point_and_no_angle() {
        for resolution in [0, 1] {
            let slopes = Heightmap::new(resolution, Vec2::ZERO, 32.0).slope_field();
            let expected = (resolution * resolution) as usize;
            assert_eq!(slopes.len(), expected, "resolution {resolution}");
            assert!(slopes.iter().all(|s| *s == 0.0), "resolution {resolution}");
        }
    }

    /// A map placed somewhere other than the middle reads its grid from
    /// where it sits: a window gathered from regions in the positive
    /// quadrant answers about those cells.
    #[test]
    fn a_placed_map_reads_its_grid_from_where_it_sits() {
        let origin = Vec2::new(100.0, -40.0);
        let map = Heightmap::new_at(65, Vec2::splat(64.0), 32.0, origin);

        assert_eq!(map.world_to_grid(origin), Vec2::ZERO);
        assert_eq!(
            map.world_to_grid(origin + Vec2::splat(64.0)),
            Vec2::splat(64.0)
        );
        assert_eq!(
            map.world_to_grid(origin + Vec2::new(3.0, 5.0)),
            Vec2::new(3.0, 5.0)
        );
    }

    /// A ray over ground that sits away from the entity still meets it.
    /// Clipping to a centred box would reject every one of these.
    #[test]
    fn a_ray_finds_ground_that_sits_away_from_the_entity() {
        let origin = Vec2::splat(200.0);
        let map = Heightmap::new_at(65, Vec2::splat(64.0), 32.0, origin);
        let centre = origin + Vec2::splat(32.0);

        let hit = map
            .raycast(Vec3::new(centre.x, 20.0, centre.y), Vec3::NEG_Y)
            .expect("the ray meets ground it is aimed straight down at");
        assert!((hit.local.x - centre.x).abs() < 1e-3);
        assert!((hit.local.z - centre.y).abs() < 1e-3);
        assert!(hit.local.y.abs() < 1e-3);
    }

    /// A cone `peak` high at the centre, falling to zero `radius` cells
    /// out.
    fn hill(peak: f32, radius: f32) -> Heightmap {
        let mut hm = flat();
        let centre = (hm.resolution - 1) as f32 / 2.0;
        for z in 0..hm.resolution {
            for x in 0..hm.resolution {
                let d = Vec2::new(x as f32 - centre, z as f32 - centre).length();
                hm.set_height(x, z, peak * (1.0 - d / radius).max(0.0));
            }
        }
        hm
    }

    /// Where the ray meets the flat base plane, which is the surface where
    /// nothing is sculpted.
    fn plane_hit(origin: Vec3, dir: Vec3) -> Vec3 {
        origin + dir * (-origin.y / dir.y)
    }

    #[test]
    fn a_ray_onto_unsculpted_ground_matches_the_plane_solution() {
        let hm = flat();
        for dir in [
            Vec3::new(0.0, -20.0, -30.0),
            Vec3::new(-11.0, -40.0, 7.0),
            Vec3::new(0.0, -1.0, 0.0),
        ] {
            let origin = Vec3::new(3.0, 25.0, 18.0);
            let dir = dir.normalize();
            let hit = hm.raycast(origin, dir).expect("the ray crosses the ground");
            let expected = plane_hit(origin, dir);
            assert!(
                hit.local.distance(expected) < 1e-2,
                "{hit:?} should match the plane solution {expected:?}",
            );
            assert!(
                hit.grid.distance(hm.world_to_grid(expected.xz())) < 1e-2,
                "grid coordinate should agree too: {hit:?}",
            );
        }
    }

    /// On sculpted ground the plane solution runs past the hill and lands
    /// beyond what the cursor is over. The march stops on the near slope.
    #[test]
    fn a_ray_at_a_hill_stops_on_the_near_slope() {
        let hm = hill(10.0, 12.0);
        let origin = Vec3::new(0.0, 12.0, 30.0);
        let dir = (Vec3::new(0.0, 0.0, 0.0) - origin).normalize();

        let hit = hm.raycast(origin, dir).expect("the ray meets the hill");

        // The near slope faces the camera, so the hit sits short of the
        // peak in z and above the base plane in y.
        assert!(
            hit.local.z > 2.0,
            "hit must be on the near slope, not at the peak or past it: {hit:?}",
        );
        assert!(hit.local.y > 1.0, "hit must be up the slope: {hit:?}");
        let expected = 10.0 * (1.0 - hit.local.xz().length() / 12.0);
        assert!(
            (hit.local.y - expected).abs() < 0.05,
            "hit must lie on the surface: {hit:?} vs {expected}",
        );
        assert!(
            hit.local.z > plane_hit(origin, dir).z + 2.0,
            "hit must sit short of where the base plane solves: {hit:?}",
        );
    }

    /// A grazing ray stops on the first slope it meets rather than
    /// carrying over the top to the far side.
    #[test]
    fn a_shallow_ray_takes_the_first_slope_not_the_far_one() {
        let hm = hill(10.0, 12.0);
        let origin = Vec3::new(0.0, 6.0, 30.0);
        let dir = Vec3::new(0.0, -0.12, -1.0).normalize();

        let hit = hm.raycast(origin, dir).expect("the ray meets the hill");

        assert!(hit.local.z > 0.0, "the far side is past the peak: {hit:?}");
    }

    #[test]
    fn a_ray_starting_under_the_surface_reports_where_it_leaves_the_ground() {
        let hm = hill(10.0, 12.0);
        // Inside the hill: the surface overhead is 10 units up.
        let origin = Vec3::new(0.0, 2.0, 0.0);
        let hit = hm
            .raycast(origin, Vec3::new(0.0, 0.4, -1.0).normalize())
            .expect("the ray leaves the ground somewhere");

        assert!(hit.local.z < 0.0, "it leaves going the way it points");
        assert!(hit.local.y > 2.0, "and above where it started: {hit:?}");
    }

    #[test]
    fn a_ray_from_below_looking_up_meets_the_underside() {
        let hm = hill(10.0, 12.0);
        let origin = Vec3::new(0.0, -20.0, 0.0);
        let hit = hm
            .raycast(origin, Vec3::Y)
            .expect("straight up through the peak");

        assert!((hit.local.y - 10.0).abs() < 0.05, "{hit:?}");
    }

    #[test]
    fn a_ray_pointing_away_from_the_terrain_hits_nothing() {
        let hm = hill(10.0, 12.0);
        assert!(hm.raycast(Vec3::new(0.0, 40.0, 0.0), Vec3::Y).is_none());
        assert!(
            hm.raycast(Vec3::new(200.0, 40.0, 200.0), Vec3::NEG_Y)
                .is_none(),
            "outside the footprint",
        );
        assert!(hm.raycast(Vec3::new(0.0, 40.0, 0.0), Vec3::ZERO).is_none());
    }

    /// The brush ring must not flicker between two cells when the cursor
    /// rests on the seam between them, so a ray onto a cell boundary has
    /// to resolve the same way every time it is cast.
    #[test]
    fn a_ray_onto_a_cell_boundary_resolves_the_same_way_every_time() {
        let hm = hill(10.0, 12.0);
        // Straight down onto the exact corner shared by four cells.
        let origin = Vec3::new(-22.0, 40.0, -22.0);
        let first = hm.raycast(origin, Vec3::NEG_Y).expect("hit");
        for _ in 0..8 {
            assert_eq!(hm.raycast(origin, Vec3::NEG_Y), Some(first));
        }
        assert!(first.grid.distance(Vec2::splat(10.0)) < 1e-3, "{first:?}");
        assert!(
            (first.local.y - hm.get_height(10, 10)).abs() < 1e-2,
            "{first:?}",
        );
    }

    /// A height the sculpt tools cannot produce but a hand-edited or
    /// corrupt sidecar can must not drag the range to infinity, or the
    /// slab the march is clipped to swallows the whole ray.
    #[test]
    fn a_non_finite_height_widens_nothing() {
        let mut hm = flat();
        hm.set_height(1, 1, 4.0);
        hm.set_height(2, 2, -3.0);
        let sane = hm.height_bounds();
        assert_eq!(sane, (-3.0, 4.0));

        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            hm.set_height(5, 5, bad);
            assert_eq!(hm.height_bounds(), sane, "{bad}");
        }
    }

    /// A caller that hands in the range this map would have scanned for
    /// itself must get the identical hit, or caching that range changes
    /// where the brush lands.
    #[test]
    fn a_precomputed_height_range_casts_the_same_as_a_scanned_one() {
        let hm = hill(10.0, 12.0);
        let bounds = hm.height_bounds();
        for origin in [
            Vec3::new(0.0, 12.0, 30.0),
            Vec3::new(-22.0, 40.0, -22.0),
            Vec3::new(0.0, 2.0, 0.0),
            Vec3::new(0.0, -20.0, 0.0),
            Vec3::new(200.0, 40.0, 200.0),
        ] {
            for dir in [
                Vec3::NEG_Y,
                Vec3::Y,
                Vec3::new(0.0, -0.12, -1.0).normalize(),
                Vec3::new(-11.0, -40.0, 7.0).normalize(),
            ] {
                assert_eq!(
                    hm.raycast_within(origin, dir, bounds),
                    hm.raycast(origin, dir),
                    "{origin:?} {dir:?}",
                );
            }
        }
    }

    /// Every column of a sculpted terrain, sampled top-down, must land on
    /// its own height -- no cell where the march steps over the surface.
    #[test]
    fn a_top_down_ray_lands_on_the_height_under_it() {
        let hm = hill(10.0, 12.0);
        for gz in [0u32, 7, 20, 32, 41, 64] {
            for gx in [0u32, 5, 19, 32, 50, 64] {
                let local = Vec3::new(gx as f32 - 32.0, 40.0, gz as f32 - 32.0);
                let hit = hm.raycast(local, Vec3::NEG_Y).expect("hit");
                assert!(
                    (hit.local.y - hm.get_height(gx, gz)).abs() < 1e-2,
                    "({gx},{gz}): {hit:?} vs {}",
                    hm.get_height(gx, gz),
                );
            }
        }
    }
}
