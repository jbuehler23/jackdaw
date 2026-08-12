//! Sparse region storage for a terrain's heights, control map, and color.
//!
//! A terrain is a sparse grid of fixed-size square [`Region`]s, addressed by
//! integer [`RegionCoord`]. A region springs into existence the first time
//! an edit touches one of its cells and is dropped again once every cell in
//! it goes back to the default value -- a terrain that has only ever been
//! edited near the origin costs nothing for the rest of the world, however
//! far it nominally extends. This mirrors `Terrain3D`'s region model.
//!
//! Heights and the control map (see [`crate::control`]) always exist per
//! region; the color layer is optional and only allocated the first time a
//! cell in that region is painted, per the design's "allocated on first
//! paint" rule.
//!
//! Cell coordinates are signed: a region's world position can be negative
//! in either axis, since edits are not constrained to start at the origin.

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

/// Why a region size could not be used for a newly created terrain.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionSizeError {
    /// A region cannot have zero cells per side.
    Zero,
    /// New terrains require a power-of-two region size, so the mesher and
    /// LOD clipmap (later tasks) can assume clean subdivision.
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
/// exists: changing it would require re-bucketing every region.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegionSize(u32);

impl RegionSize {
    /// Region side length new terrains use unless the author picks another
    /// power of two at creation time.
    pub const DEFAULT: u32 = 256;

    /// Validated constructor for a terrain created going forward: `side`
    /// must be a nonzero power of two.
    pub fn new(side: u32) -> Result<Self, RegionSizeError> {
        if side == 0 {
            return Err(RegionSizeError::Zero);
        }
        if !side.is_power_of_two() {
            return Err(RegionSizeError::NotPowerOfTwo);
        }
        Ok(Self(side))
    }

    /// Unchecked constructor for sizes that predate the power-of-two rule.
    ///
    /// A v1 sidecar's whole resolution becomes one implicit region of that
    /// exact size on migration (see [`crate::sidecar`]), and that
    /// resolution was never required to be a power of two. Sidecar v3
    /// decode also uses this once it has confirmed the declared size is
    /// backed by enough file bytes to be plausible.
    ///
    /// The caller must ensure `side > 0`; this is not re-checked.
    pub(crate) fn new_unchecked(side: u32) -> Self {
        debug_assert!(side > 0, "region size must be nonzero");
        Self(side)
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
    control: Vec<u32>,
    color: Option<Vec<[u8; 4]>>,
}

impl Region {
    fn empty(side: u32) -> Self {
        let n = (side as usize) * (side as usize);
        Self {
            side,
            heights: vec![0.0; n],
            control: vec![0; n],
            color: None,
        }
    }

    /// Build a region directly from already-sized layers. Used by sidecar
    /// decode and the v1 migration bridge, both of which validate lengths
    /// against `side` themselves before calling this.
    pub(crate) fn from_parts(
        side: u32,
        heights: Vec<f32>,
        control: Vec<u32>,
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

    /// Row-major packed control words, length `side * side`.
    pub fn control_words(&self) -> &[u32] {
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
        Control::from_raw(self.control[self.idx(lx, lz)])
    }

    fn set_control(&mut self, lx: u32, lz: u32, value: Control) {
        let i = self.idx(lx, lz);
        self.control[i] = value.to_raw();
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

    /// Drop the color layer back to `None` if painting has undone every
    /// cell back to [`DEFAULT_COLOR`], so an undo-to-nothing stroke does
    /// not leave a permanently allocated (if uniform) color buffer behind.
    fn prune_color_if_default(&mut self) {
        if matches!(&self.color, Some(layer) if layer.iter().all(|c| *c == DEFAULT_COLOR)) {
            self.color = None;
        }
    }

    fn is_default(&self) -> bool {
        self.color.is_none()
            && self.heights.iter().all(|h| *h == 0.0)
            && self.control.iter().all(|c| *c == 0)
    }
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

    /// Insert an already-built region, dropping it instead if it turns out
    /// to be all-default. Used by sidecar decode and the v1 migration
    /// bridge; edits during normal use go through [`Self::set_height`] and
    /// friends instead.
    pub(crate) fn insert_region(&mut self, coord: RegionCoord, region: Region) {
        debug_assert_eq!(region.side(), self.region_size.get());
        if !region.is_default() {
            self.regions.insert(coord, region);
        }
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
    /// non-default write, and dropping it again if the write brought every
    /// cell in it back to default.
    pub fn set_height(&mut self, x: i32, z: i32, value: f32) {
        let (coord, lx, lz) = self.locate(x, z);
        let side = self.region_size;
        self.regions
            .entry(coord)
            .or_insert_with(|| Region::empty(side.get()))
            .set_height(lx, lz, value);
        self.prune(coord);
    }

    pub fn control_at(&self, x: i32, z: i32) -> Control {
        let (coord, lx, lz) = self.locate(x, z);
        self.regions
            .get(&coord)
            .map_or(Control::default(), |region| region.control(lx, lz))
    }

    pub fn set_control(&mut self, x: i32, z: i32, value: Control) {
        let (coord, lx, lz) = self.locate(x, z);
        let side = self.region_size;
        self.regions
            .entry(coord)
            .or_insert_with(|| Region::empty(side.get()))
            .set_control(lx, lz, value);
        self.prune(coord);
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
        let side = self.region_size;
        self.regions
            .entry(coord)
            .or_insert_with(|| Region::empty(side.get()))
            .set_color(lx, lz, value);
        self.prune(coord);
    }

    fn prune(&mut self, coord: RegionCoord) {
        if let Some(region) = self.regions.get_mut(&coord) {
            region.prune_color_if_default();
            if region.is_default() {
                self.regions.remove(&coord);
            }
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
    }

    #[test]
    fn returning_every_cell_to_default_deallocates_the_region() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        t.set_height(0, 0, 3.0);
        t.set_height(1, 2, 4.0);
        assert_eq!(t.region_count(), 1);
        t.set_height(0, 0, 0.0);
        assert_eq!(t.region_count(), 1, "region 1,2 is still non-default");
        t.set_height(1, 2, 0.0);
        assert_eq!(t.region_count(), 0);
    }

    #[test]
    fn control_writes_allocate_and_deallocate_independently_of_height() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        let painted = Control::default().with_base_id(2);
        t.set_control(0, 0, painted);
        assert_eq!(t.region_count(), 1);
        assert_eq!(t.control_at(0, 0), painted);
        t.set_control(0, 0, Control::default());
        assert_eq!(t.region_count(), 0);
    }

    #[test]
    fn color_layer_allocates_on_first_paint_and_frees_when_reverted() {
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
            t.region(RegionCoord::ORIGIN).is_none(),
            "region should fully deallocate once color, control and height are all default"
        );
    }

    #[test]
    fn painting_color_does_not_by_itself_keep_an_otherwise_default_region_forever_once_undone() {
        let mut t = TerrainRegions::new(RegionSize::new(2).unwrap());
        t.set_color(0, 0, [1, 2, 3, 4]);
        t.set_color(1, 1, [5, 6, 7, 8]);
        assert_eq!(t.region_count(), 1);
        t.set_color(0, 0, DEFAULT_COLOR);
        assert_eq!(t.region_count(), 1, "cell (1,1) is still painted");
        t.set_color(1, 1, DEFAULT_COLOR);
        assert_eq!(t.region_count(), 0);
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
    fn insert_region_drops_an_all_default_region_instead_of_storing_it() {
        let mut t = TerrainRegions::new(RegionSize::new(2).unwrap());
        let region = Region::from_parts(2, vec![0.0; 4], vec![0; 4], None);
        t.insert_region(RegionCoord::ORIGIN, region);
        assert_eq!(t.region_count(), 0);
    }

    #[test]
    fn insert_region_keeps_a_non_default_region() {
        let mut t = TerrainRegions::new(RegionSize::new(2).unwrap());
        let region = Region::from_parts(2, vec![1.0, 0.0, 0.0, 0.0], vec![0; 4], None);
        t.insert_region(RegionCoord::ORIGIN, region);
        assert_eq!(t.region_count(), 1);
        assert_eq!(t.height_at(0, 0), 1.0);
    }
}
