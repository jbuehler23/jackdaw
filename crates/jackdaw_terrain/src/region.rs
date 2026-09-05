//! Sparse region storage for a terrain's heights, control map, and color.
//!
//! A terrain is a sparse grid of fixed-size square [`Region`]s, addressed by
//! integer [`RegionCoord`]. A region allocates the first time an edit
//! writes a non-default value into one of its cells.
//!
//! Region presence is explicit, not derived from content: a region
//! sculpted back to every default value stays allocated and round-trips
//! through a sidecar unchanged. [`TerrainRegions::remove_region`] is the
//! only way a region is removed. Allocating on write is an O(1) check
//! against the value being written, not a scan of the region.
//!
//! Heights and the control map (see [`crate::control`]) always exist per
//! region. The color layer is optional, allocated on first paint, and then
//! stays allocated: it does not collapse back to absent when every pixel
//! reads as [`DEFAULT_COLOR`] again.
//!
//! Cell coordinates are signed; a region's position can be negative in
//! either axis.
//!
//! A region is allocated whole, so a terrain's extent is always a whole
//! number of regions; a partly filled region reports the same extent as a
//! full one.
//!
//! Each cell has a single owning region; storage never duplicates an edge
//! row into a neighbor. Edge vertices are read from the neighboring region
//! (via [`TerrainRegions::region`]), and an absent neighbor clamps.

use std::collections::HashMap;

use crate::control::Control;

/// Most regions one terrain may hold: 32 by 32 of them.
///
/// A terrain's extent is emergent, so without a ceiling a runaway stroke
/// or a stray script could allocate until memory ran out. At the default
/// region size and cell size this is an eight-kilometre square: a guard
/// rail, not a shape.
pub const MAX_REGIONS: usize = 32 * 32;

/// Tint for a cell with no color layer: opaque white (no tint).
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

/// An allocation refused because it would take the terrain past
/// [`MAX_REGIONS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionCapError {
    /// Regions the terrain already holds.
    pub held: usize,
    /// Regions the refused allocation would have added.
    pub wanted: usize,
}

impl core::fmt::Display for RegionCapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "this terrain holds {} of its {MAX_REGIONS} regions and that edit needs {} more; \
             nothing was allocated and no ground was changed",
            self.held, self.wanted,
        )
    }
}

impl core::error::Error for RegionCapError {}

/// Why a region size could not be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionSizeError {
    /// A region cannot have zero cells per side.
    Zero,
    /// Region size must be a power of two, so the mesher can assume clean
    /// subdivision. Enforced on every `RegionSize`, not only new terrains.
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
/// exists. Always a nonzero power of two; there is no unchecked
/// constructor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RegionSize(u32);

impl RegionSize {
    /// Region side length new terrains use unless the author picks another
    /// power of two at creation time.
    pub const DEFAULT: RegionSize = RegionSize(256);

    /// The size a terrain of `resolution` vertices per edge stores its
    /// regions at: the resolution itself when that is a power of two, and
    /// otherwise the largest power of two below it.
    ///
    /// The second case stores a `2^k + 1` vertex grid, such as 129,
    /// without resampling: the grid runs one vertex past the region that
    /// holds it, and that seam vertex belongs to the next region along
    /// ([`TerrainRegions::grid_height`]). A non-power-of-two resolution
    /// lands between two powers of two, so the grid never spans more than
    /// two regions per axis.
    ///
    /// That says nothing about extent. A terrain is whole regions, so a
    /// grid that is not a region multiple is *embedded* in the regions
    /// holding it and the terrain reaches to their far edge. Every
    /// authored value keeps the cell it described, and the rest of those
    /// regions reads as zero.
    ///
    /// Resolution 0 has no vertices to store and reports
    /// [`Self::DEFAULT`].
    pub fn for_resolution(resolution: u32) -> Self {
        if resolution == 0 {
            return Self::DEFAULT;
        }
        Self(1u32 << (u32::BITS - 1 - resolution.leading_zeros()))
    }

    /// `side` must be a nonzero power of two.
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
    /// One plane per channel the terrain declares, each `side * side`
    /// values, in the channel directory's order.
    ///
    /// Channels live here, beside the heights, rather than in a dense grid
    /// of their own. Extent is whatever the regions reach, so a second
    /// grid sized independently of them would have to be resized and
    /// re-anchored before it covered a region a stroke had allocated, and
    /// every read in between would land on the wrong cell.
    channels: Vec<Vec<u16>>,
}

impl Region {
    fn empty(side: u32, channels: usize) -> Self {
        let n = (side as usize) * (side as usize);
        Self {
            side,
            heights: vec![0.0; n],
            control: vec![Control::default(); n],
            color: None,
            channels: vec![vec![0; n]; channels],
        }
    }

    /// Build a region from already-sized layers. Lengths must match `side`.
    pub(crate) fn from_parts(
        side: u32,
        heights: Vec<f32>,
        control: Vec<Control>,
        color: Option<Vec<[u8; 4]>>,
        channels: Vec<Vec<u16>>,
    ) -> Self {
        let n = (side as usize) * (side as usize);
        debug_assert_eq!(heights.len(), n);
        debug_assert_eq!(control.len(), n);
        if let Some(c) = &color {
            debug_assert_eq!(c.len(), n);
        }
        debug_assert!(channels.iter().all(|plane| plane.len() == n));
        Self {
            side,
            heights,
            control,
            color,
            channels,
        }
    }

    /// Row-major values of one channel, or `None` when this region carries
    /// no such channel.
    pub fn channel(&self, index: usize) -> Option<&[u16]> {
        self.channels.get(index).map(Vec::as_slice)
    }

    /// Row-major values of one channel, writable in place.
    pub fn channel_mut(&mut self, index: usize) -> Option<&mut [u16]> {
        self.channels.get_mut(index).map(Vec::as_mut_slice)
    }

    /// How many channel planes this region carries.
    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    /// Swap in a whole set of channel planes, already in directory order.
    fn replace_channels(&mut self, planes: Vec<Vec<u16>>) {
        let n = (self.side as usize) * (self.side as usize);
        debug_assert!(planes.iter().all(|plane| plane.len() == n));
        self.channels = planes;
    }

    /// Bring the channel planes to `count`, zero-filling new ones and
    /// dropping the tail.
    fn set_channel_count(&mut self, count: usize) {
        let n = (self.side as usize) * (self.side as usize);
        self.channels.resize_with(count, || vec![0; n]);
    }

    fn channel_value(&self, index: usize, lx: u32, lz: u32) -> u16 {
        let idx = self.idx(lx, lz);
        self.channels.get(index).map_or(0, |plane| plane[idx])
    }

    fn set_channel_value(&mut self, index: usize, lx: u32, lz: u32, value: u16) {
        let idx = self.idx(lx, lz);
        if let Some(plane) = self.channels.get_mut(index) {
            plane[idx] = value;
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

    /// Row-major heights, writable in place.
    ///
    /// The region already exists, so a write through this changes no
    /// region's presence and cannot shrink the layer. Unlike
    /// [`TerrainRegions::set_height`] it does not canonicalize `-0.0`:
    /// what is written is what the sidecar stores.
    pub fn heights_mut(&mut self) -> &mut [f32] {
        &mut self.heights
    }

    /// Row-major control words, writable in place.
    pub fn control_words_mut(&mut self) -> &mut [Control] {
        &mut self.control
    }

    /// Overwrite heights from `source`, zero-filling a short source and
    /// ignoring the tail of a long one, so the layer keeps its size.
    pub fn copy_heights_from(&mut self, source: &[f32]) {
        copy_layer(&mut self.heights, source, 0.0);
    }

    /// Overwrite control words from `source`, under the same
    /// short/long rules as [`Self::copy_heights_from`].
    pub fn copy_control_from(&mut self, source: &[Control]) {
        copy_layer(&mut self.control, source, Control::default());
    }

    /// Overwrite the color layer from `source`, allocating it if `source`
    /// has one and dropping it if it does not.
    pub fn copy_color_from(&mut self, source: Option<&[[u8; 4]]>) {
        match source {
            Some(source) => {
                let n = self.heights.len();
                let layer = self.color.get_or_insert_with(|| vec![DEFAULT_COLOR; n]);
                copy_layer(layer, source, DEFAULT_COLOR);
            }
            None => self.color = None,
        }
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

/// Overwrite `layer` in place from `source`, keeping `layer`'s length:
/// a short source leaves `fill` behind it, a long one has its tail
/// ignored.
fn copy_layer<T: Copy>(layer: &mut [T], source: &[T], fill: T) {
    let shared = layer.len().min(source.len());
    layer[..shared].copy_from_slice(&source[..shared]);
    layer[shared..].fill(fill);
}

/// Collapse `-0.0` to `+0.0`, so a default-valued cell has one canonical
/// bit pattern whether it was touched or not.
fn canonicalize_zero(value: f32) -> f32 {
    if value == 0.0 { 0.0 } else { value }
}

/// A terrain's sparse height, control, color and channel storage.
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainRegions {
    region_size: RegionSize,
    regions: HashMap<RegionCoord, Region>,
    /// How many channel planes each region carries. Every region agrees
    /// with this, so a newly allocated one already covers every channel
    /// the terrain declares.
    channel_count: usize,
}

impl TerrainRegions {
    pub fn new(region_size: RegionSize) -> Self {
        Self {
            region_size,
            regions: HashMap::new(),
            channel_count: 0,
        }
    }

    /// How many channel planes every region carries.
    pub fn channel_count(&self) -> usize {
        self.channel_count
    }

    /// Declare how many channels every region carries, zero-filling a new
    /// plane in each and dropping the tail.
    ///
    /// The channel directory lives on the document; this keeps the stored
    /// planes agreeing with it, so adding a channel paints zero everywhere
    /// rather than leaving regions that answer for a channel and regions
    /// that do not.
    pub fn set_channel_count(&mut self, count: usize) {
        self.channel_count = count;
        for region in self.regions.values_mut() {
            region.set_channel_count(count);
        }
    }

    /// Rebuild every region's channel planes to a new directory order.
    ///
    /// `sources[i]` names which of the current planes becomes plane `i`,
    /// or `None` for a channel that did not exist before and starts
    /// zeroed. Renaming, reordering and removing channels all go through
    /// here, so a plane always follows the channel it belongs to instead
    /// of staying at an index whose meaning changed underneath it.
    pub fn remap_channels(&mut self, sources: &[Option<usize>]) {
        let n = (self.region_size.get() as usize) * (self.region_size.get() as usize);
        for region in self.regions.values_mut() {
            let mut planes = Vec::with_capacity(sources.len());
            for source in sources {
                let plane = source
                    .and_then(|at| region.channel(at))
                    .map(<[u16]>::to_vec)
                    .unwrap_or_else(|| vec![0; n]);
                planes.push(plane);
            }
            region.replace_channels(planes);
        }
        self.channel_count = sources.len();
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

    pub fn region_mut(&mut self, coord: RegionCoord) -> Option<&mut Region> {
        self.regions.get_mut(&coord)
    }

    /// The region at `coord`, allocated all-default if it was absent.
    ///
    /// The authored-presence allocator: a tool that declares a region part
    /// of the terrain calls it, and what it allocates persists even if
    /// every cell reads as default. Writing a value through
    /// [`Self::set_height`] and friends is the other way a region comes
    /// into being, and that one allocates only for a non-default value.
    pub fn ensure_region(&mut self, coord: RegionCoord) -> &mut Region {
        let side = self.region_size.get();
        let channels = self.channel_count;
        self.regions
            .entry(coord)
            .or_insert_with(|| Region::empty(side, channels))
    }

    /// Insert an already-built region as present, regardless of content.
    /// Callers must reject a duplicate coordinate themselves; a second call
    /// for the same coordinate overwrites the first.
    pub(crate) fn insert_region(&mut self, coord: RegionCoord, region: Region) {
        debug_assert_eq!(region.side(), self.region_size.get());
        self.regions.insert(coord, region);
    }

    /// Remove a region regardless of content. The only way a region is
    /// deallocated. Returns the removed region, if any.
    pub fn remove_region(&mut self, coord: RegionCoord) -> Option<Region> {
        self.regions.remove(&coord)
    }

    /// Allocated regions in coordinate-sorted order, independent of
    /// `HashMap` iteration order.
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

    /// Write a height, allocating the region on first non-default write.
    /// A region never deallocates itself as a side effect of a write.
    pub fn set_height(&mut self, x: i32, z: i32, value: f32) {
        let value = canonicalize_zero(value);
        let (coord, lx, lz) = self.locate(x, z);
        match self.regions.get_mut(&coord) {
            Some(region) => region.set_height(lx, lz, value),
            None if value != 0.0 => {
                let side = self.region_size.get();
                let channels = self.channel_count;
                self.regions
                    .entry(coord)
                    .or_insert_with(|| Region::empty(side, channels))
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
                let channels = self.channel_count;
                self.regions
                    .entry(coord)
                    .or_insert_with(|| Region::empty(side, channels))
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

    /// Regions per axis a dense `resolution`-per-edge vertex grid covers,
    /// starting at region `(0, 0)`.
    pub fn grid_span(&self, resolution: u32) -> u32 {
        resolution.div_ceil(self.region_size.get())
    }

    /// Inclusive coordinate bounds of the allocated regions, or `None`
    /// when nothing is allocated.
    pub fn region_bounds(&self) -> Option<(RegionCoord, RegionCoord)> {
        let mut coords = self.regions.keys();
        let first = *coords.next()?;
        let (mut min, mut max) = (first, first);
        for coord in coords {
            min = RegionCoord::new(min.x.min(coord.x), min.z.min(coord.z));
            max = RegionCoord::new(max.x.max(coord.x), max.z.max(coord.z));
        }
        Some((min, max))
    }

    /// Cells per axis the allocated regions reach past the origin, or
    /// `None` when nothing is allocated.
    ///
    /// How much ground the document holds, which a terrain component's
    /// `resolution` is meant to cover. A stored extent larger than the
    /// declared grid means cells no tool can address.
    pub fn stored_extent(&self) -> Option<(u32, u32)> {
        let (min, max) = self.region_bounds()?;
        let side = i64::from(self.region_size.get());
        let span =
            |lo: i32, hi: i32| ((i64::from(hi) + 1 - i64::from(lo).min(0)) * side).max(0) as u32;
        Some((span(min.x, max.x), span(min.z, max.z)))
    }

    /// Allocate every region a dense `resolution`-per-edge vertex grid
    /// touches, so every vertex of the grid has a region holding it.
    ///
    /// The regions are authored presence in the [`Self::ensure_region`]
    /// sense: a grid one vertex wider than its region brings the next
    /// region into being to hold that seam, and it persists.
    /// Refuses outright, allocating nothing, when the grid would take the
    /// terrain past [`MAX_REGIONS`]: a half-allocated grid is a worse
    /// answer than none, because the caller cannot tell how far it got.
    pub fn ensure_grid(&mut self, resolution: u32) -> Result<(), RegionCapError> {
        let span = self.grid_span(resolution) as i32;
        let wanted: usize = (0..span)
            .flat_map(|rz| (0..span).map(move |rx| RegionCoord::new(rx, rz)))
            .filter(|coord| !self.regions.contains_key(coord))
            .count();
        if self.regions.len() + wanted > MAX_REGIONS {
            return Err(RegionCapError {
                held: self.regions.len(),
                wanted,
            });
        }
        for rz in 0..span {
            for rx in 0..span {
                self.ensure_region(RegionCoord::new(rx, rz));
            }
        }
        Ok(())
    }

    /// Whether one more region would fit inside [`MAX_REGIONS`].
    pub fn has_room_for_one_more(&self) -> bool {
        self.regions.len() < MAX_REGIONS
    }

    /// Height at a vertex of the dense grid, clamping into the last
    /// present region when the one that owns the vertex is absent.
    ///
    /// The clamp is the seam rule: each cell belongs to one region, the vertex
    /// past a region's last column belongs to the next region along, and
    /// an absent neighbor repeats the edge rather than dropping to zero
    /// and tearing a cliff into the terrain's border.
    pub fn grid_height(&self, x: u32, z: u32) -> f32 {
        let (x, z) = self.clamped_into_presence(x, z);
        self.height_at(x, z)
    }

    /// [`Self::grid_height`] for the control map.
    pub fn grid_control(&self, x: u32, z: u32) -> Control {
        let (x, z) = self.clamped_into_presence(x, z);
        self.control_at(x, z)
    }

    /// Value of one channel at a cell, or `0` when no region holds it.
    ///
    /// An unallocated cell reads as zero by construction rather than by a
    /// length check: there is no separate grid whose size could disagree
    /// with where the cells are.
    pub fn channel_at(&self, index: usize, x: i32, z: i32) -> u16 {
        let (coord, lx, lz) = self.locate(x, z);
        self.regions
            .get(&coord)
            .map_or(0, |region| region.channel_value(index, lx, lz))
    }

    /// Write a channel value, allocating the region on first non-zero
    /// write, exactly as a height write does.
    pub fn set_channel(&mut self, index: usize, x: i32, z: i32, value: u16) {
        if index >= self.channel_count {
            return;
        }
        let (coord, lx, lz) = self.locate(x, z);
        match self.regions.get_mut(&coord) {
            Some(region) => region.set_channel_value(index, lx, lz, value),
            None if value != 0 => {
                let side = self.region_size.get();
                let channels = self.channel_count;
                self.regions
                    .entry(coord)
                    .or_insert_with(|| Region::empty(side, channels))
                    .set_channel_value(index, lx, lz, value);
            }
            None => {}
        }
    }

    /// [`Self::grid_height`] for one channel.
    pub fn grid_channel(&self, index: usize, x: u32, z: u32) -> u16 {
        let (x, z) = self.clamped_into_presence(x, z);
        self.channel_at(index, x, z)
    }

    /// The dense `resolution`-per-edge grid of one channel, gathered from
    /// every region it spans. Exact, like [`Self::read_grid_heights`].
    pub fn read_grid_channel(&self, index: usize, resolution: u32) -> Vec<u16> {
        self.read_grid(resolution, |regions, x, z| {
            regions.channel_at(index, x as i32, z as i32)
        })
    }

    /// Scatter a dense channel grid back across the regions that own it.
    ///
    /// Cell `(x, z)` of the grid is cell `(x, z)` of the terrain, so a
    /// value lands on the same ground it described.
    pub fn write_grid_channel(&mut self, index: usize, resolution: u32, source: &[u16]) {
        for z in 0..resolution {
            for x in 0..resolution {
                let at = (z as usize) * (resolution as usize) + x as usize;
                let Some(value) = source.get(at).copied() else {
                    continue;
                };
                self.set_channel(index, x as i32, z as i32, value);
            }
        }
    }

    /// Step a vertex back into a present region when its own is absent.
    ///
    /// One step per axis is all the seam can need: only the row and
    /// column past a region's last one can fall outside it. The first
    /// candidate that lands wins, so an axis steps back only when it has
    /// to: a vertex that reaches a present region by clamping in x alone
    /// keeps its own z rather than shearing a row down the seam.
    fn clamped_into_presence(&self, x: u32, z: u32) -> (i32, i32) {
        let (x, z) = (x as i32, z as i32);
        let present = |x: i32, z: i32| {
            let (coord, _, _) = self.locate(x, z);
            self.regions.contains_key(&coord)
        };
        for candidate in [(x, z), (x - 1, z), (x, z - 1), (x - 1, z - 1)] {
            if present(candidate.0, candidate.1) {
                return candidate;
            }
        }
        (x, z)
    }

    /// The dense `resolution`-per-edge height grid, gathered from every
    /// region it spans.
    ///
    /// Exact: a vertex whose region is absent reads as the default, the
    /// same as [`Self::height_at`]. This is what a document is read and
    /// written through, so a gather followed by a scatter stores what was
    /// stored and nothing else. [`Self::sampled_grid_heights`] is the
    /// view for drawing.
    pub fn read_grid_heights(&self, resolution: u32) -> Vec<f32> {
        self.read_grid(resolution, |regions, x, z| {
            regions.height_at(x as i32, z as i32)
        })
    }

    /// [`Self::read_grid_heights`] with the seam clamped: a vertex whose
    /// region is absent repeats its neighbour rather than dropping to
    /// zero.
    ///
    /// What a mesher samples. The difference shows only at the border of
    /// a sparse terrain, and there the clamp is the difference between an
    /// edge that lies flat and a cliff down to zero.
    pub fn sampled_grid_heights(&self, resolution: u32) -> Vec<f32> {
        let mut out = Vec::new();
        self.sample_grid_heights_into(resolution, &mut out);
        out
    }

    /// [`Self::sampled_grid_heights`] into a buffer the caller already
    /// holds, so a terrain being resampled every time it is written does
    /// not allocate its heights afresh each time.
    pub fn sample_grid_heights_into(&self, resolution: u32, out: &mut Vec<f32>) {
        out.clear();
        out.reserve((resolution as usize) * (resolution as usize));
        // At a power-of-two resolution the grid is one whole region,
        // already in the layout wanted, with no vertex held elsewhere.
        if let Some(region) = self.regions.get(&RegionCoord::ORIGIN)
            && region.side() == resolution
        {
            out.extend_from_slice(region.heights());
            return;
        }
        for z in 0..resolution {
            for x in 0..resolution {
                out.push(self.grid_height(x, z));
            }
        }
    }

    /// The dense `resolution`-per-edge control grid, exact in the sense
    /// [`Self::read_grid_heights`] is.
    pub fn read_grid_control(&self, resolution: u32) -> Vec<Control> {
        self.read_grid(resolution, |regions, x, z| {
            regions.control_at(x as i32, z as i32)
        })
    }

    fn read_grid<T>(&self, resolution: u32, at: impl Fn(&Self, u32, u32) -> T) -> Vec<T> {
        let mut out = Vec::with_capacity((resolution as usize) * (resolution as usize));
        for z in 0..resolution {
            for x in 0..resolution {
                out.push(at(self, x, z));
            }
        }
        out
    }

    /// Scatter a dense `resolution`-per-edge height grid back into the
    /// regions that own it, allocating any that are missing.
    ///
    /// A source shorter than the grid leaves zeroes behind it, matching
    /// [`Region::copy_heights_from`]; a longer one has its tail ignored.
    ///
    /// A grid past [`MAX_REGIONS`] writes into the regions that already
    /// exist and allocates none: the cap refuses as one, so nothing here
    /// is half-allocated.
    pub fn write_grid_heights(&mut self, resolution: u32, source: &[f32]) {
        let _ = self.ensure_grid(resolution);
        for z in 0..resolution {
            for x in 0..resolution {
                let index = (z as usize) * (resolution as usize) + x as usize;
                let value = source.get(index).copied().unwrap_or(0.0);
                self.set_height(x as i32, z as i32, value);
            }
        }
    }

    /// [`Self::write_grid_heights`] for the control map.
    pub fn write_grid_control(&mut self, resolution: u32, source: &[Control]) {
        let _ = self.ensure_grid(resolution);
        for z in 0..resolution {
            for x in 0..resolution {
                let index = (z as usize) * (resolution as usize) + x as usize;
                let value = source.get(index).copied().unwrap_or_default();
                self.set_control(x as i32, z as i32, value);
            }
        }
    }

    /// Whether the region owning cell `(x, z)` is present.
    ///
    /// What the mesher asks before it spends vertices on ground nothing
    /// authored.
    pub fn covers(&self, x: i32, z: i32) -> bool {
        let (coord, _, _) = self.locate(x, z);
        self.regions.contains_key(&coord)
    }

    /// Paint a color, allocating the region's color layer on first use.
    pub fn set_color(&mut self, x: i32, z: i32, value: [u8; 4]) {
        let (coord, lx, lz) = self.locate(x, z);
        match self.regions.get_mut(&coord) {
            Some(region) => region.set_color(lx, lz, value),
            None if value != DEFAULT_COLOR => {
                let side = self.region_size.get();
                let channels = self.channel_count;
                self.regions
                    .entry(coord)
                    .or_insert_with(|| Region::empty(side, channels))
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
        // though the color layer exists.
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
    fn ensure_region_allocates_an_all_default_region_that_persists() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        t.ensure_region(RegionCoord::ORIGIN);
        assert_eq!(t.region_count(), 1);
        let region = t.region(RegionCoord::ORIGIN).unwrap();
        assert!(region.heights().iter().all(|h| *h == 0.0));
        assert!(
            region
                .control_words()
                .iter()
                .all(|c| *c == Control::default())
        );
        assert!(region.color().is_none());
    }

    #[test]
    fn ensure_region_keeps_what_an_existing_region_holds() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        t.set_height(1, 1, 5.0);
        t.ensure_region(RegionCoord::ORIGIN);
        assert_eq!(t.region_count(), 1);
        assert_eq!(t.height_at(1, 1), 5.0);
    }

    #[test]
    fn writing_through_heights_mut_lands_in_the_region() {
        let mut t = TerrainRegions::new(RegionSize::new(2).unwrap());
        t.ensure_region(RegionCoord::ORIGIN).heights_mut()[3] = 7.0;
        assert_eq!(t.height_at(1, 1), 7.0);
        t.region_mut(RegionCoord::ORIGIN)
            .unwrap()
            .control_words_mut()[0] = Control::default().with_base_id(2);
        assert_eq!(t.control_at(0, 0).base_id(), 2);
        assert_eq!(t.region_count(), 1);
    }

    #[test]
    fn copying_a_layer_in_keeps_the_regions_size() {
        let mut region = Region::empty(2, 0);
        region.copy_heights_from(&[1.0, 2.0]);
        assert_eq!(region.heights(), &[1.0, 2.0, 0.0, 0.0]);
        region.copy_heights_from(&[9.0; 16]);
        assert_eq!(region.heights(), &[9.0; 4]);
        region.copy_control_from(&[Control::default().with_base_id(3)]);
        assert_eq!(region.control_words()[0].base_id(), 3);
        assert_eq!(region.control_words()[3], Control::default());
    }

    #[test]
    fn copying_color_in_allocates_the_layer_and_copying_none_drops_it() {
        let mut region = Region::empty(2, 0);
        region.copy_color_from(Some(&[[1, 2, 3, 4]]));
        assert_eq!(
            region.color(),
            Some([[1, 2, 3, 4], DEFAULT_COLOR, DEFAULT_COLOR, DEFAULT_COLOR].as_slice())
        );
        region.copy_color_from(None);
        assert!(region.color().is_none());
    }

    #[test]
    fn insert_region_keeps_an_all_default_region() {
        let mut t = TerrainRegions::new(RegionSize::new(2).unwrap());
        let region = Region::from_parts(
            2,
            vec![0.0; 4],
            vec![Control::default(); 4],
            None,
            Vec::new(),
        );
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
            Vec::new(),
        );
        t.insert_region(RegionCoord::ORIGIN, region);
        assert_eq!(t.region_count(), 1);
        assert_eq!(t.height_at(0, 0), 1.0);
    }

    /// A 5-vertex grid on 4-wide regions: the seam column x=4 is the only
    /// part of the grid the second region column holds. Heights are
    /// `x + 10*z`, so a vertex that read a row back is off by 10.
    fn seam_grid() -> TerrainRegions {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        t.ensure_grid(5).expect("inside the region cap");
        for z in 0..5i32 {
            for x in 0..5i32 {
                t.set_height(x, z, x as f32 + 10.0 * z as f32);
            }
        }
        t
    }

    #[test]
    fn an_absent_seam_column_repeats_the_edge_without_shearing_the_rows() {
        let mut t = seam_grid();
        t.remove_region(RegionCoord::new(1, 0));
        t.remove_region(RegionCoord::new(1, 1));
        for z in 0..5 {
            assert_eq!(
                t.grid_height(4, z),
                t.grid_height(3, z),
                "the seam column repeats its neighbour at z={z}"
            );
            assert_eq!(t.grid_height(4, z), 3.0 + 10.0 * z as f32);
        }
    }

    #[test]
    fn an_absent_seam_row_repeats_the_edge_without_shifting_the_columns() {
        let mut t = seam_grid();
        t.remove_region(RegionCoord::new(0, 1));
        t.remove_region(RegionCoord::new(1, 1));
        for x in 0..5 {
            assert_eq!(
                t.grid_height(x, 4),
                t.grid_height(x, 3),
                "the seam row repeats its neighbour at x={x}"
            );
            assert_eq!(t.grid_height(x, 4), x as f32 + 30.0);
        }
    }

    #[test]
    fn a_seam_corner_steps_back_on_both_axes_only_when_neither_alone_lands() {
        let mut t = seam_grid();
        t.remove_region(RegionCoord::new(1, 1));
        assert_eq!(
            t.grid_height(4, 4),
            3.0 + 40.0,
            "region (0, 1) holds this row, so x clamps alone"
        );

        t.remove_region(RegionCoord::new(1, 0));
        t.remove_region(RegionCoord::new(0, 1));
        assert_eq!(
            t.grid_height(4, 4),
            3.0 + 30.0,
            "only region (0, 0) is left"
        );
    }

    /// A terrain's extent is emergent, so the cap is the only thing
    /// standing between a runaway edit and unbounded allocation.
    #[test]
    fn allocating_past_the_cap_is_refused_whole() {
        let mut t = TerrainRegions::new(RegionSize::new(1).unwrap());
        // 32x32 regions of one cell each is exactly the cap.
        assert!(t.ensure_grid(32).is_ok());
        assert_eq!(t.region_count(), MAX_REGIONS);
        assert!(!t.has_room_for_one_more());

        let refused = t.ensure_grid(33).expect_err("past the cap");
        assert_eq!(refused.held, MAX_REGIONS);
        assert!(refused.wanted > 0);
        assert_eq!(
            t.region_count(),
            MAX_REGIONS,
            "a refused allocation leaves the terrain exactly as it was",
        );
    }

    /// The refusal message names the terrain's own numbers.
    #[test]
    fn the_cap_refusal_names_what_it_refused() {
        let mut t = TerrainRegions::new(RegionSize::new(1).unwrap());
        t.ensure_grid(32).expect("the cap exactly");
        let message = t.ensure_grid(64).expect_err("past the cap").to_string();
        assert!(message.contains(&MAX_REGIONS.to_string()), "{message}");
        assert!(message.contains("nothing was allocated"), "{message}");
    }

    #[test]
    fn an_empty_document_has_no_stored_extent() {
        let t = TerrainRegions::new(RegionSize::new(4).unwrap());
        assert_eq!(t.region_bounds(), None);
        assert_eq!(t.stored_extent(), None);
    }

    #[test]
    fn the_stored_extent_counts_the_cells_the_regions_reach() {
        let mut t = TerrainRegions::new(RegionSize::new(256).unwrap());
        t.ensure_region(RegionCoord::ORIGIN);
        assert_eq!(t.stored_extent(), Some((256, 256)));

        t.ensure_region(RegionCoord::new(3, 1));
        assert_eq!(
            t.stored_extent(),
            Some((1024, 512)),
            "four regions across and two down"
        );
    }

    /// The reachability question the extent answers is about the grid a
    /// terrain component declares, which starts at the origin, so a
    /// region in negative space widens the extent rather than sliding it.
    #[test]
    fn negative_regions_widen_the_stored_extent() {
        let mut t = TerrainRegions::new(RegionSize::new(4).unwrap());
        t.ensure_region(RegionCoord::new(-1, 0));
        t.ensure_region(RegionCoord::new(0, 0));
        assert_eq!(t.stored_extent(), Some((8, 4)));
    }

    /// The degraded-scene shape: a kilometre of stored ground under a
    /// component that only declares a quarter of it.
    #[test]
    fn stored_regions_can_reach_past_the_grid_a_resolution_declares() {
        let mut t = TerrainRegions::new(RegionSize::new(256).unwrap());
        for rz in 0..4 {
            for rx in 0..4 {
                t.ensure_region(RegionCoord::new(rx, rz));
            }
        }
        assert_eq!(t.grid_span(256), 1);
        assert_eq!(t.stored_extent(), Some((1024, 1024)));
    }
}
