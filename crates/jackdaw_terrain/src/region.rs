//! Sparse region storage for a terrain's heights, control map, and color.
//!
//! A terrain is a sparse grid of fixed-size square [`Region`]s, addressed by
//! integer [`RegionCoord`]. A region springs into existence the first time
//! an edit writes a non-default value into one of its cells -- a terrain
//! that has only ever been edited near the origin costs nothing for the
//! rest of the world, however far it nominally extends. This mirrors
//! `Terrain3D`'s region model.
//!
//! Existence, once granted, is an AUTHORED fact, not a derived one (T1
//! review ruling, 2026-08-12): a region sculpted flat back to every default
//! value is still a region a person chose to allocate, and it stays
//! allocated, round-trips through a sidecar, and re-encodes exactly as it
//! was. The only way a region goes away is [`TerrainRegions::remove_region`]
//! -- an explicit, deliberate removal, not a side effect of ordinary
//! editing. This also means no setter ever scans a whole region to decide
//! whether to keep it: allocating is an O(1) "does this one write differ
//! from default" check, not an O(side^2) region-wide scan.
//!
//! Heights and the control map (see [`crate::control`]) always exist per
//! region; the color layer is optional and, once allocated the first time a
//! cell in that region is painted, stays allocated for the same
//! authored-presence reason -- it does not collapse back to absent just
//! because every pixel happens to read as [`DEFAULT_COLOR`] again.
//!
//! Cell coordinates are signed: a region's world position can be negative
//! in either axis, since edits are not constrained to start at the origin.
//!
//! # Seam rule (T4 builds on this; documented here since this is where
//! region addressing lives)
//!
//! Cells are single-owner: a cell belongs to exactly one region, and
//! storage never duplicates an edge row into its neighbor. A mesher
//! building seamless geometry across a region boundary reads the
//! neighboring region directly (via [`TerrainRegions::region`]) for the
//! extra vertex row/column it needs; if that neighbor is not allocated, the
//! mesher clamps to the current region's own edge rather than treating the
//! absent neighbor as an error or fabricating a duplicate row for it.
//!
//! # A note on `f32` equality
//!
//! [`Region`], [`TerrainRegions`] and [`crate::sidecar::RegionTerrainData`]
//! all derive `PartialEq` down to raw `f32` heights. IEEE 754 `NaN` is not
//! equal to itself, so a height array containing `NaN` makes its own struct
//! not equal to a bit-for-bit copy of itself under `==`/`assert_eq!`; a test
//! that needs to compare data that may contain `NaN` must compare
//! `to_bits()` of the individual floats instead.

use std::collections::HashMap;

use crate::control::Control;

/// A cell's tint when no color layer has been painted there. Opaque white:
/// "no tint," matching the convention that an unpainted region looks
/// exactly like the splat material alone would render it.
pub const DEFAULT_COLOR: [u8; 4] = [255, 255, 255, 255];

/// Integer coordinate of a region on the terrain's region grid. Not a cell
/// coordinate -- multiply by the terrain's region size to get the world
/// cell of the region's minimum corner.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RegionCoord {
    pub x: i32,
    pub z: i32,
}

impl RegionCoord {
    pub const ORIGIN: RegionCoord = RegionCoord { x: 0, z: 0 };

    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }
}

impl core::fmt::Display for RegionCoord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "({}, {})", self.x, self.z)
    }
}

/// Why a region size could not be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionSizeError {
    /// A region cannot have zero cells per side.
    Zero,
    /// Region size must be a power of two, so the mesher and LOD clipmap
    /// (later tasks) can assume clean subdivision. This is a real,
    /// always-enforced invariant, not just a rule for newly created
    /// terrains: a sidecar that declares a non-power-of-two region size is
    /// rejected the same as one that declares zero.
    NotPowerOfTwo,
}

impl core::fmt::Display for RegionSizeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Zero => write!(f, "region size must be at least 1 cell per side"),
            Self::NotPowerOfTwo => write!(f, "region size must be a power of two"),
        }
    }
}

impl core::error::Error for RegionSizeError {}

/// Cells per edge of every region in a terrain. Immutable once a terrain
/// exists: changing it would require re-bucketing every region. Every
/// `RegionSize` in existence, however it was built, is guaranteed a
/// nonzero power of two -- there is no unchecked or "trusted caller"
/// constructor. A v1 sidecar's resolution is not assumed to satisfy this,
/// so migrating one is fallible; see
/// [`crate::sidecar::RegionTerrainData::from_legacy_v1`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegionSize(u32);

impl RegionSize {
    /// Region side length new terrains use unless the author picks another
    /// power of two at creation time.
    pub const DEFAULT: RegionSize = RegionSize(256);

    /// Validated constructor: `side` must be a nonzero power of two. Used
    /// for terrains created going forward and for every sidecar v3 decode
    /// -- a declared region size is only ever trusted once it has passed
    /// through here.
    pub fn new(side: u32) -> Result<Self, RegionSizeError> {
        if side == 0 {
            return Err(RegionSizeError::Zero);
        }
        if !side.is_power_of_two() {
            return Err(RegionSizeError::NotPowerOfTwo);
        }
        Ok(Self(side))
    }

    pub fn get(self) -> u32 {
        self.0
    }
}

/// One region's heights, control map, and optional color layer, each
/// `side * side` cells, row-major (z-major, matching this crate's other
/// grid data).
#[derive(Clone, Debug, PartialEq)]
pub struct Region {
    side: u32,
    heights: Vec<f32>,
    control: Vec<Control>,
    color: Option<Vec<[u8; 4]>>,
}

impl Region {
    fn empty(side: u32) -> Self {
        let n = (side as usize) * (side as usize);
        Self {
            side,
            heights: vec![0.0; n],
            control: vec![Control::default(); n],
            color: None,
        }
    }

    /// Build a region directly from already-sized layers. Used by sidecar
    /// decode and the v1 migration bridge, both of which validate lengths
    /// against `side` themselves before calling this.
    pub(crate) fn from_parts(
        side: u32,
        heights: Vec<f32>,
        control: Vec<Control>,
        color: Option<Vec<[u8; 4]>>,
    ) -> Self {
        let n = (side as usize) * (side as usize);
        debug_assert_eq!(heights.len(), n);
        debug_assert_eq!(control.len(), n);
        if let Some(c) = &color {
            debug_assert_eq!(c.len(), n);
        }
        Self {
            side,
            heights,
            control,
            color,
        }
    }

    pub fn side(&self) -> u32 {
        self.side
    }

    /// Row-major heights, length `side * side`.
    pub fn heights(&self) -> &[f32] {
        &self.heights
    }

    /// Row-major control words, length `side * side`.
    pub fn control_words(&self) -> &[Control] {
        &self.control
    }

    /// Row-major color, if this region has ever been painted.
    pub fn color(&self) -> Option<&[[u8; 4]]> {
        self.color.as_deref()
    }

    fn idx(&self, lx: u32, lz: u32) -> usize {
        (lz as usize) * (self.side as usize) + lx as usize
    }

    fn height(&self, lx: u32, lz: u32) -> f32 {
        self.heights[self.idx(lx, lz)]
    }

    fn set_height(&mut self, lx: u32, lz: u32, value: f32) {
        let i = self.idx(lx, lz);
        self.heights[i] = value;
    }

    fn control(&self, lx: u32, lz: u32) -> Control {
        self.control[self.idx(lx, lz)]
    }

    fn set_control(&mut self, lx: u32, lz: u32, value: Control) {
        let i = self.idx(lx, lz);
        self.control[i] = value;
    }

    fn color_at(&self, lx: u32, lz: u32) -> [u8; 4] {
        let i = self.idx(lx, lz);
        match &self.color {
            Some(layer) => layer[i],
            None => DEFAULT_COLOR,
        }
    }

    fn set_color(&mut self, lx: u32, lz: u32, value: [u8; 4]) {
        let n = self.heights.len();
        let i = self.idx(lx, lz);
        let layer = self.color.get_or_insert_with(|| vec![DEFAULT_COLOR; n]);
        layer[i] = value;
    }
}

/// Collapse `-0.0` to `+0.0` so a height that reads as "the default value"
/// always has the one canonical bit pattern, in memory and on disk. Without
/// this, sculpting a cell to exactly `-0.0` would compare equal to a
/// never-touched default cell (`-0.0 == 0.0` is `true`) while carrying a
/// different bit pattern, which is exactly the kind of surprise a "bit-level
/// round trip" sidecar format should not have.
fn canonicalize_zero(value: f32) -> f32 {
    if value == 0.0 { 0.0 } else { value }
}

/// A terrain's sparse height/control/color storage.
///
/// Existing paint channels (scatter masks) are not part of this: they stay
/// dense, whole-terrain, and unrelated to regions, per the design's split
/// between gameplay masks and visual splat data.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainRegions {
    region_size: RegionSize,
    regions: HashMap<RegionCoord, Region>,
}

impl TerrainRegions {
    pub fn new(region_size: RegionSize) -> Self {
        Self {
            region_size,
            regions: HashMap::new(),
        }
    }

    pub fn region_size(&self) -> RegionSize {
        self.region_size
    }

    /// Number of currently allocated regions.
    pub fn region_count(&self) -> usize {
        self.regions.len()
    }

    pub fn region(&self, coord: RegionCoord) -> Option<&Region> {
        self.regions.get(&coord)
    }

    /// Insert an already-built region as an authored, present region.
    /// Unlike the per-cell setters this never checks the region's content:
    /// a region a sidecar's region table lists, or that migration builds
    /// from a legacy heightmap, is present because it says so, default
    /// content and all. Used by sidecar decode and the v1 migration
    /// bridge; both are responsible for rejecting a duplicate coordinate
    /// themselves before calling this, since a second call for the same
    /// coordinate silently overwrites the first.
    pub(crate) fn insert_region(&mut self, coord: RegionCoord, region: Region) {
        debug_assert_eq!(region.side(), self.region_size.get());
        self.regions.insert(coord, region);
    }

    /// Explicitly deallocate a region regardless of its content. This is
    /// the only way a region goes away once it exists -- ordinary editing,
    /// even sculpting every cell back to default, never does this on its
    /// own. Returns the removed region, if there was one.
    pub fn remove_region(&mut self, coord: RegionCoord) -> Option<Region> {
        self.regions.remove(&coord)
    }

    /// Allocated regions in a deterministic, coordinate-sorted order.
    /// Serializing code depends on this: `HashMap` iteration order is not
    /// stable, and encoding must be a pure function of the data regardless
    /// of the order edits happened to allocate regions in.
    pub fn iter_sorted(&self) -> impl Iterator<Item = (RegionCoord, &Region)> {
        let mut entries: Vec<_> = self.regions.iter().map(|(c, r)| (*c, r)).collect();
        entries.sort_by_key(|(coord, _)| *coord);
        entries.into_iter()
    }

    fn locate(&self, x: i32, z: i32) -> (RegionCoord, u32, u32) {
        let side = self.region_size.get() as i32;
        let rx = x.div_euclid(side);
        let rz = z.div_euclid(side);
        let lx = x.rem_euclid(side) as u32;
        let lz = z.rem_euclid(side) as u32;
        (RegionCoord::new(rx, rz), lx, lz)
    }

    pub fn height_at(&self, x: i32, z: i32) -> f32 {
        let (coord, lx, lz) = self.locate(x, z);
        self.regions
            .get(&coord)
            .map_or(0.0, |region| region.height(lx, lz))
    }

    /// Write a height, allocating the owning region if this is its first
    /// non-default write. Once allocated, a region never deallocates
    /// itself as a side effect of a write -- see the module docs.
    pub fn set_height(&mut self, x: i32, z: i32, value: f32) {
        let value = canonicalize_zero(value);
        let (coord, lx, lz) = self.locate(x, z);
        match self.regions.get_mut(&coord) {
            Some(region) => region.set_height(lx, lz, value),
            None if value != 0.0 => {
                let side = self.region_size.get();
                self.regions
                    .entry(coord)
                    .or_insert_with(|| Region::empty(side))
                    .set_height(lx, lz, value);
            }
            None => {}
        }
    }

    pub fn control_at(&self, x: i32, z: i32) -> Control {
        let (coord, lx, lz) = self.locate(x, z);
        self.regions
            .get(&coord)
            .map_or(Control::default(), |region| region.control(lx, lz))
    }

    pub fn set_control(&mut self, x: i32, z: i32, value: Control) {
        let (coord, lx, lz) = self.locate(x, z);
        match self.regions.get_mut(&coord) {
            Some(region) => region.set_control(lx, lz, value),
            None if value != Control::default() => {
                let side = self.region_size.get();
                self.regions
                    .entry(coord)
                    .or_insert_with(|| Region::empty(side))
                    .set_control(lx, lz, value);
            }
            None => {}
        }
    }

    pub fn color_at(&self, x: i32, z: i32) -> [u8; 4] {
        let (coord, lx, lz) = self.locate(x, z);
        self.regions
            .get(&coord)
            .map_or(DEFAULT_COLOR, |region| region.color_at(lx, lz))
    }

    /// Paint a color, allocating the region's color layer on first use.
    pub fn set_color(&mut self, x: i32, z: i32, value: [u8; 4]) {
        let (coord, lx, lz) = self.locate(x, z);
        match self.regions.get_mut(&coord) {
            Some(region) => region.set_color(lx, lz, value),
            None if value != DEFAULT_COLOR => {
                let side = self.region_size.get();
                self.regions
                    .entry(coord)
                    .or_insert_with(|| Region::empty(side))
                    .set_color(lx, lz, value);
            }
            None => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_size_rejects_zero_and_non_power_of_two() {
        assert_eq!(RegionSize::new(0), Err(RegionSizeError::Zero));
        assert_eq!(RegionSize::new(3), Err(RegionSizeError::NotPowerOfTwo));
        assert_eq!(RegionSize::new(255), Err(RegionSizeError::NotPowerOfTwo));
        assert!(RegionSize::new(256).is_ok());
        assert!(RegionSize::new(1).is_ok());
        assert_eq!(RegionSize::DEFAULT.get(), 256);
    }

    #[test]
    fn a_fresh_terrain_has_no_regions() {
        let t = TerrainRegions::new(RegionSize::new(4).unwrap());
        assert_eq!(t.region_count(), 0);
        assert_eq!(t.height_at(0, 0), 0.0);
        assert_eq!(t.control_at(0, 0), Control::default());
        assert_eq!(t.color_at(0, 0), DEFAULT_COLOR);
    }

    #[test]
    fn writing_a_height_allocates_its_region() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        assert_eq!(t.region_count(), 0);
        t.set_height(1, 1, 5.0);
        assert_eq!(t.region_count(), 1);
        assert_eq!(t.height_at(1, 1), 5.0);
        assert!(t.region(RegionCoord::ORIGIN).is_some());
    }

    #[test]
    fn writing_default_value_to_an_unallocated_cell_allocates_nothing() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        t.set_height(1, 1, 0.0);
        assert_eq!(t.region_count(), 0);
        t.set_control(1, 1, Control::default());
        assert_eq!(t.region_count(), 0);
        t.set_color(1, 1, DEFAULT_COLOR);
        assert_eq!(t.region_count(), 0);
    }

    #[test]
    fn writing_negative_zero_to_an_unallocated_cell_allocates_nothing() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        t.set_height(1, 1, -0.0);
        assert_eq!(t.region_count(), 0);
        assert_eq!(t.height_at(1, 1).to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn negative_zero_is_canonicalized_to_positive_zero_bit_pattern() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        // Force an allocation first, then sculpt the same cell to -0.0.
        t.set_height(1, 1, 5.0);
        t.set_height(1, 1, -0.0);
        assert_eq!(t.height_at(1, 1).to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn a_nan_height_still_allocates_and_reads_back_as_nan() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        t.set_height(0, 0, f32::NAN);
        assert_eq!(t.region_count(), 1);
        assert!(t.height_at(0, 0).is_nan());
    }

    #[test]
    fn sculpting_every_cell_back_to_default_does_not_deallocate_the_region() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        t.set_height(0, 0, 3.0);
        t.set_height(1, 2, 4.0);
        assert_eq!(t.region_count(), 1);
        t.set_height(0, 0, 0.0);
        t.set_height(1, 2, 0.0);
        assert_eq!(
            t.region_count(),
            1,
            "an authored region persists even when every cell reads as default"
        );
        assert_eq!(t.height_at(0, 0), 0.0);
        assert_eq!(t.height_at(1, 2), 0.0);
    }

    #[test]
    fn control_writes_allocate_but_never_self_deallocate() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        let painted = Control::default().with_base_id(2);
        t.set_control(0, 0, painted);
        assert_eq!(t.region_count(), 1);
        assert_eq!(t.control_at(0, 0), painted);
        t.set_control(0, 0, Control::default());
        assert_eq!(t.region_count(), 1);
        assert_eq!(t.control_at(0, 0), Control::default());
    }

    #[test]
    fn color_layer_allocates_on_first_paint_and_stays_allocated() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        assert_eq!(t.color_at(2, 2), DEFAULT_COLOR);
        t.set_color(2, 2, [10, 20, 30, 255]);
        assert!(t.region(RegionCoord::ORIGIN).unwrap().color().is_some());
        assert_eq!(t.color_at(2, 2), [10, 20, 30, 255]);
        // Other cells in the same region are still the default tint even
        // though the color layer now physically exists.
        assert_eq!(t.color_at(0, 0), DEFAULT_COLOR);

        t.set_color(2, 2, DEFAULT_COLOR);
        assert!(
            t.region(RegionCoord::ORIGIN).unwrap().color().is_some(),
            "the color layer stays allocated even once every pixel is default again"
        );
        assert_eq!(t.region_count(), 1);
    }

    #[test]
    fn explicit_remove_is_the_only_way_a_region_disappears() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        t.set_height(0, 0, 1.0);
        t.set_height(0, 0, 0.0);
        assert_eq!(t.region_count(), 1, "still present after sculpting to 0");
        let removed = t.remove_region(RegionCoord::ORIGIN);
        assert!(removed.is_some());
        assert_eq!(t.region_count(), 0);
        assert!(t.remove_region(RegionCoord::ORIGIN).is_none());
    }

    #[test]
    fn cells_at_a_region_edge_and_the_next_regions_corner_are_distinct() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        t.set_height(3, 3, 1.0); // last cell of region (0,0)
        t.set_height(4, 4, 2.0); // first cell of region (1,1)
        assert_eq!(t.region_count(), 2);
        assert_eq!(t.height_at(3, 3), 1.0);
        assert_eq!(t.height_at(4, 4), 2.0);
        assert_eq!(t.height_at(4, 3), 0.0);
        assert_eq!(t.height_at(3, 4), 0.0);
        assert!(t.region(RegionCoord::new(0, 0)).is_some());
        assert!(t.region(RegionCoord::new(1, 1)).is_some());
    }

    #[test]
    fn negative_cell_coordinates_address_negative_regions_correctly() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        t.set_height(-1, -1, 9.0); // last cell of region (-1,-1)
        assert_eq!(t.height_at(-1, -1), 9.0);
        assert!(t.region(RegionCoord::new(-1, -1)).is_some());
        // The region boundary sits exactly at 0: cell 0 is region (0,0),
        // not (-1,-1).
        assert_eq!(t.height_at(0, 0), 0.0);
        assert!(t.region(RegionCoord::new(0, 0)).is_none());

        t.set_height(-4, -4, 7.0); // first cell of region (-1,-1)
        assert_eq!(t.height_at(-4, -4), 7.0);
        assert_eq!(t.height_at(-1, -1), 9.0, "still holds its earlier write");

        t.set_height(-5, -5, 3.0); // last cell of region (-2,-2)
        assert!(t.region(RegionCoord::new(-2, -2)).is_some());
        assert_eq!(t.height_at(-5, -5), 3.0);
    }

    #[test]
    fn region_boundary_math_matches_across_a_zero_crossing() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        for x in -8..8 {
            for z in -8..8 {
                t.set_height(x, z, ((x + z) as f32) + 0.5);
            }
        }
        for x in -8..8 {
            for z in -8..8 {
                assert_eq!(t.height_at(x, z), ((x + z) as f32) + 0.5, "at ({x},{z})");
            }
        }
    }

    #[test]
    fn iter_sorted_is_stable_regardless_of_insertion_order() {
        let mut a = TerrainRegions::new(RegionSize::new(2).unwrap());
        a.set_height(0, 0, 1.0);
        a.set_height(10, 10, 2.0);
        a.set_height(-10, 3, 3.0);

        let mut b = TerrainRegions::new(RegionSize::new(2).unwrap());
        b.set_height(-10, 3, 3.0);
        b.set_height(0, 0, 1.0);
        b.set_height(10, 10, 2.0);

        let coords_a: Vec<_> = a.iter_sorted().map(|(c, _)| c).collect();
        let coords_b: Vec<_> = b.iter_sorted().map(|(c, _)| c).collect();
        assert_eq!(coords_a, coords_b);
        assert_eq!(
            coords_a,
            vec![
                RegionCoord::new(-5, 1),
                RegionCoord::new(0, 0),
                RegionCoord::new(5, 5),
            ]
        );
    }

    #[test]
    fn insert_region_keeps_an_all_default_region() {
        let mut t = TerrainRegions::new(RegionSize::new(2).unwrap());
        let region = Region::from_parts(2, vec![0.0; 4], vec![Control::default(); 4], None);
        t.insert_region(RegionCoord::ORIGIN, region);
        assert_eq!(
            t.region_count(),
            1,
            "an explicitly authored all-default region is still present"
        );
        assert_eq!(t.height_at(0, 0), 0.0);
    }

    #[test]
    fn insert_region_keeps_a_non_default_region() {
        let mut t = TerrainRegions::new(RegionSize::new(2).unwrap());
        let region = Region::from_parts(
            2,
            vec![1.0, 0.0, 0.0, 0.0],
            vec![Control::default(); 4],
            None,
        );
        t.insert_region(RegionCoord::ORIGIN, region);
        assert_eq!(t.region_count(), 1);
        assert_eq!(t.height_at(0, 0), 1.0);
    }
}
