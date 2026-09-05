//! Binary sidecar format for a terrain's bulk per-cell data.
//!
//! A terrain's heights and paint channels are too large for a text scene
//! file: a 512-resolution heightmap is 262,144 floats. They live in a
//! versioned binary file beside the scene, and the scene keeps only the
//! descriptive parts: resolution, world size, the channel table.
//!
//! An external pipeline can read the format in any language without linking
//! jackdaw:
//!
//! ```text
//! offset  size          field
//! 0       8             magic, b"JDTERRN\0"
//! 8       2             format version, u16
//! 10      2             flags, u16 (reserved, must be 0)
//! 12      4             resolution, u32
//! 16      4             channel count, u32
//! 20      4*res*res     heights, f32 (IEEE 754, little-endian)
//! then, per channel:
//!         4             name length in bytes, u32
//!         n             name, UTF-8
//!         1             element tag, u8 (0 = u8, 1 = u16)
//!         3             padding, must be 0
//!         w*res*res     values, u8 or u16 per the tag
//! ```
//!
//! Every integer and float is little-endian. Encoding is a pure function of
//! the data: the same terrain always produces the same bytes.
//!
//! There is no checksum. Decode catches structural corruption (bad magic,
//! an unreadable version, a length that does not fit the claimed shape),
//! not silent bit rot within otherwise well-formed bytes.
//!
//! Decode requires the file to end exactly where the declared structure
//! ends: trailing bytes, and a header that under-claims a count, are both
//! rejected as [`SidecarError::Truncated`].
//!
//! # Format version 2
//!
//! Format version 1 is the layout above: one dense heightmap plus channels,
//! sized to a single `resolution`. Format version 2 places heights, the
//! control map, and an optional color layer in sparse regions (see
//! [`crate::region`]), addressed by [`RegionCoord`]. Channels stay dense at
//! `channel_resolution`.
//!
//! ```text
//! offset  size          field
//! 0       8             magic, b"JDTERRN\0"             (shared header)
//! 8       2             format version, u16 == 2
//! 10      2             flags, u16 (reserved, must be 0)
//! 12      4             channel_resolution, u32
//! 16      4             channel_count, u32
//! 20      ...           channels, same per-channel layout as version 1
//! then
//!         4             region_size, u32 (cells per region edge; a
//!                        nonzero power of two)
//!         4             region_count, u32
//!         4             material_count, u32
//! then, per material slot (material_count times), in texture-id order:
//!         4             material name length in bytes, u32 (0 = tombstone:
//!                        an id held open by a removed material)
//!         n             material name, UTF-8 (a saved material's name,
//!                        which is also its file stem)
//!         4             uv_scale, f32
//!         4             detile, f32 (version 3 only; version 2 files
//!                        have no such field and read as 0 = off)
//! then, per region (region_count times), each present regardless of
//! content:
//!         4             region coord x, i32
//!         4             region coord z, i32
//!         1             region flags, u8:
//!                          bit 0 = has color layer
//!                          bit 1 = present (always 1)
//!                          bits 2-7 = reserved, must be 0
//!         3             padding, must be 0
//!         4*size^2      heights, f32 (IEEE 754, little-endian)
//!         4*size^2      control, u32 (packed, see crate::control)
//!         4*size^2      color RGBA8 -- only present if flag bit 0 is set
//! ```
//!
//! Format version 3 is version 2 with one more `f32` per material slot:
//! the per-slot detiling strength. A version-2 file reads as strength 0,
//! which is off.
//!
//! # Format version 4
//!
//! Version 4 is version 3 with one fixed-size block between the material
//! table and the region table: the terrain's autoterrain settings. It sits
//! beside the material slots because that is what its two slot ids address,
//! and it leaves the region stream at the end of the file.
//!
//! ```text
//! offset  size          field
//!         1             flags, u8: bit 0 = enabled, bits 1-7 reserved,
//!                        must be 0
//!         1             base slot id, u8
//!         1             slope slot id, u8
//!         1             padding, must be 0
//!         4             slope range start in degrees, f32
//!         4             slope range end in degrees, f32
//! ```
//!
//! A version-2 or version-3 file has no such block and reads as disabled
//! at the default range.
//!
//! # Format version 5
//!
//! Version 5 states where its cells sit and carries the paint channels in
//! the regions.
//!
//! The header has no `channel_resolution`, so it runs magic, version,
//! flags, `channel_count`. What follows is a channel directory: per
//! channel, a name length, the name, an element tag and three padding
//! bytes, and no values. The values are per region, written after that
//! region's colour: one plane per declared channel, in directory order,
//! `side^2` values at that channel's own width.
//!
//! Channels live in the regions because a terrain's extent is emergent. A
//! dense grid sized independently of the regions cannot cover a region a
//! stroke has just allocated without being resized and re-anchored first,
//! and a read in between lands on the wrong cell. Every region carries
//! every declared channel, and a cell no region holds reads as zero.
//!
//! Between the autoterrain block and the region table sits one fixed-size
//! block: where this terrain's cell grid sits and how big its cells are.
//!
//! ```text
//! offset  size          field
//!         4             cell_size, f32 (world metres per cell edge)
//!         4             anchor x, f32 (world offset of cell (0, 0) from
//!                        the terrain entity's origin)
//!         4             anchor z, f32
//! ```
//!
//! A version-4 or older file carries its channels as one dense grid
//! anchored at cell `(0, 0)`, the same anchor the heights use, so the load
//! slices that grid into the regions owning its cells.
//!
//! Grid geometry lives here, beside the cells it describes, rather than
//! only on the `Terrain` component.
//!
//! A terrain's extent is emergent: the allocated regions are the terrain,
//! and how much ground they cover is their count times
//! [`GridGeometry::cell_size`]. Nothing declares a rectangle, so no edit
//! can put stored cells outside what the terrain claims.
//!
//! A save writes the sidecar before the scene text and cannot roll one
//! back if the other fails, so the two can disagree on disk. When they do,
//! the sidecar wins for interpreting stored cells: it travels with the
//! data, so a version-5 sidecar beside stale scene text still draws its
//! ground at the same place and scale. The component's `cell_size` is the
//! authoring surface, and a save copies it here.
//!
//! A version-4 or older file states no geometry. Its cells are addressed
//! by a declared `size`/`resolution` rectangle centred on the entity, so
//! it loads with no geometry of its own and the caller derives it from
//! that vintage's contract: `cell_size = size.x / (resolution - 1)`,
//! anchored at `-size/2`. Every stored cell keeps the world position it
//! had, and the first save records the result as version 5.
//!
//! # Format version 7
//!
//! Version 7 carries a terrain's scatter as data. One block follows the
//! surface block, ahead of the region table: two count-prefixed side
//! tables the placements index into.
//!
//! ```text
//! offset  size          field
//!         4             palette entry count, u32
//! then, per palette entry:
//!         4             asset path length in bytes, u32 (0 = tombstone:
//!                        an index held open by a removed entry)
//!         n             asset path, UTF-8, relative to the assets
//!                        directory, ending in .gltf or .glb
//!         1             flags, u8: bit 0 = placements block agents,
//!                        bits 1-7 reserved, must be 0
//!         3             padding, must be 0
//!         4             cull distance in world units, f32 (0 = no cutoff)
//! then
//!         4             group key count, u32
//! then, per group key:
//!         4             key length in bytes, u32 (0 = tombstone)
//!         n             key, UTF-8
//! ```
//!
//! Each region then ends with its own placement list, written after that
//! region's channel planes:
//!
//! ```text
//! offset  size          field
//!         4             placement count, u32
//! then, per placement:
//!         2             group index, u16
//!         2             palette index, u16
//!         4             x offset from the region's minimum corner, f32
//!         4             height in the terrain's local space, f32
//!         4             z offset from the region's minimum corner, f32
//!         4             yaw about Y in radians, f32
//!         4             uniform scale, f32
//! ```
//!
//! Both indices must name an entry the file's own tables declare;
//! anything else is [`SidecarError::UnknownScatterIndex`]. A row is
//! emptied rather than removed while a document is open, because an index
//! is what every placement in memory refers to; the writer drops those
//! tombstoned rows and renumbers the placements with them, so a file holds
//! only rows something names.
//!
//! A placement belongs to the region covering its cell and is positioned
//! against that region's minimum corner, so it moves with the ground it
//! stands on. A version-6 or older file carries no such block and loads
//! with an empty palette and no placements: its scatter, if it had any,
//! is scene entities.
//!
//! [`load`] and [`save`] are the entry points: `load` upgrades a version-1
//! file into regions sized to its resolution, reads a version-2 file as
//! detiling-off, a version-3 file as autoterrain-off, and a version-4 file
//! as stating no grid geometry; `save` always writes the current version.
//! A newer-than-this-build version is refused by both.
//! `save`/[`encode_regions`] refuse to write a malformed material name;
//! [`decode_regions`] rejects one too and clamps a slot's tiling and
//! detiling floats to their bounds. The bare `encode`/`decode` and
//! `encode_regions`/`decode_regions` pairs address one format directly.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use crate::channel::{ChannelData, ChannelDescriptor, ChannelElement};
use crate::control::Control;
use crate::placement::{
    ScatterAssetError, ScatterGroupError, ScatterPalette, ScatterPaletteEntry, ScatterPlacement,
    validate_scatter_asset, validate_scatter_group,
};
use crate::region::{Region, RegionCoord, RegionSize, RegionSizeError, TerrainRegions};

/// Magic bytes at the head of every sidecar.
pub const MAGIC: [u8; 8] = *b"JDTERRN\0";

/// Format version [`encode`]/[`decode`] read and write: the original
/// single dense heightmap layout, with no regions.
pub const VERSION: u16 = 1;

/// Format version [`encode_regions`]/[`decode_regions`] read and write, and
/// what [`save`] always writes: regions replace the dense heightmap. See
/// the module-level "Format version 2" section for the full layout.
pub const VERSION_2: u16 = 2;

/// Region documents with a per-slot detiling strength. A version-2 file
/// loads as strength 0.
pub const VERSION_3: u16 = 3;

/// Region documents with a trailing autoterrain block. A version-2 or
/// version-3 file loads with autoterrain off.
pub const VERSION_4: u16 = 4;

/// Region documents that state their own grid geometry. What [`save`]
/// writes. See the module-level "Format version 5" section.
pub const VERSION_5: u16 = 5;

/// Region documents with a trailing surface block: the blend sharpness
/// and the tint strength the splat material shades with. A version-5 or
/// older file loads at [`SurfaceSettings::default`], which is what those
/// files were rendered at. What [`save`] writes.
pub const VERSION_6: u16 = 6;

/// Region documents that carry their scatter as data: a palette block
/// after the surface block, and a placement list on each region. A
/// version-6 or older file loads with an empty palette and no placements.
/// What [`save`] writes.
pub const VERSION_7: u16 = 7;

/// Conventional file extension for a terrain sidecar.
pub const EXTENSION: &str = "jdterrain";

/// Region flags bit: this region has a color layer, written right after
/// its control words.
const REGION_FLAG_HAS_COLOR: u8 = 0b0000_0001;
/// Region flags bit: this region is present. Always set by [`save`].
const REGION_FLAG_PRESENT: u8 = 0b0000_0010;
/// Every region flags bit this build understands.
const REGION_FLAGS_KNOWN: u8 = REGION_FLAG_HAS_COLOR | REGION_FLAG_PRESENT;

/// Why a terrain sidecar path could not be resolved beneath a scene.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidecarPathError {
    /// No sidecar path was provided.
    Empty,
    /// The path is absolute or carries a platform-specific prefix.
    Absolute,
    /// The path contains a separator or component that is not a plain name.
    InvalidComponent,
    /// The path does not end in the exact lowercase `.jdterrain` extension.
    WrongExtension,
}

impl core::fmt::Display for SidecarPathError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "terrain sidecar path is empty"),
            Self::Absolute => write!(f, "terrain sidecar path must be relative to the scene"),
            Self::InvalidComponent => write!(
                f,
                "terrain sidecar path must contain only normal forward-slash-separated components"
            ),
            Self::WrongExtension => write!(
                f,
                "terrain sidecar path must end in the exact .{EXTENSION} extension"
            ),
        }
    }
}

impl core::error::Error for SidecarPathError {}

/// Traversal guard shared by every project-relative path string this crate
/// accepts from a file: sidecar `data_path` and texture-set references both
/// go through this before any format-specific check.
///
/// Stricter than the host platform: backslashes and Windows drive prefixes
/// are rejected on every platform, as are empty, `.` and `..` components.
fn reject_path_traversal(data_path: &str) -> Result<(), SidecarPathError> {
    if data_path.is_empty() {
        return Err(SidecarPathError::Empty);
    }
    if data_path.contains('\\') {
        return Err(SidecarPathError::InvalidComponent);
    }

    let path = Path::new(data_path);
    if path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
    {
        return Err(SidecarPathError::Absolute);
    }

    for (index, component) in data_path.split('/').enumerate() {
        if component.is_empty() || matches!(component, "." | "..") {
            return Err(SidecarPathError::InvalidComponent);
        }
        if index == 0
            && component.as_bytes().get(1) == Some(&b':')
            && component.as_bytes()[0].is_ascii_alphabetic()
        {
            return Err(SidecarPathError::Absolute);
        }
        if component.contains(':') {
            return Err(SidecarPathError::InvalidComponent);
        }
    }

    Ok(())
}

/// Validate `data_path` and resolve it beneath `scene_dir`.
pub fn resolve_path(scene_dir: &Path, data_path: &str) -> Result<PathBuf, SidecarPathError> {
    reject_path_traversal(data_path)?;
    let path = Path::new(data_path);

    if path.extension().and_then(|extension| extension.to_str()) != Some(EXTENSION) {
        return Err(SidecarPathError::WrongExtension);
    }

    Ok(scene_dir.join(path))
}

/// Validate a project-relative asset reference, resolved beneath the
/// project root by the asset system that loads it. Same traversal guard as
/// [`resolve_path`], minus the extension check.
pub fn validate_asset_ref(path: &str) -> Result<(), SidecarPathError> {
    reject_path_traversal(path)
}

/// Why a material name could not be stored on a terrain slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaterialNameError {
    /// A character that cannot appear in a file stem, so the name could
    /// never address one material file.
    InvalidCharacter,
    /// `.` or `..`, which name a directory rather than a material.
    Reserved,
}

impl core::fmt::Display for MaterialNameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidCharacter => write!(
                f,
                "material name may hold only letters, digits, '.', '_' and '-'"
            ),
            Self::Reserved => write!(f, "'.' and '..' are not material names"),
        }
    }
}

impl core::error::Error for MaterialNameError {}

/// Validate a terrain slot's material reference.
///
/// A material's name, its `@Name` identity and its file stem are one
/// string, so a name that could not be a file stem could never resolve.
/// Checked at the format boundary, because an unchecked name stored on a
/// terrain fails every subsequent save of the scene.
///
/// The empty name is valid and means a tombstone: see
/// [`TerrainMaterialSlot::tombstone`]. No material can answer to it, since
/// a name is a file stem and there is no empty one.
pub fn validate_material_name(name: &str) -> Result<(), MaterialNameError> {
    if name.is_empty() {
        return Ok(());
    }
    if name == "." || name == ".." {
        return Err(MaterialNameError::Reserved);
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
    {
        return Err(MaterialNameError::InvalidCharacter);
    }
    Ok(())
}

/// One texture id on a terrain: the saved material it draws, and how many
/// times that material's textures repeat per world unit here.
///
/// The scale lives on the slot rather than on the material because one
/// material is shared across surfaces: the same rock tiles differently on
/// a terrain than on a brush face.
///
/// A slot with an empty name is a tombstone: an id whose material was
/// removed. It still occupies its position, because the position is the
/// id, and the control map holds ids that must not change meaning
/// underneath what was painted with them. See [`Self::tombstone`].
#[derive(Clone, Debug, PartialEq)]
pub struct TerrainMaterialSlot {
    /// Name of a saved material: the `@Name` identity, which is also the
    /// `.material.bsn` file stem. Empty for a tombstone.
    pub material: String,
    /// Texture repeats per world unit. Unused by a tombstone, which draws
    /// nothing.
    pub uv_scale: f32,
    /// How hard the shader breaks up this slot's repetition, `0..1`. 0 is
    /// off, and every version-2 sidecar reads back as 0.
    pub detile: f32,
}

impl TerrainMaterialSlot {
    /// A slot at the default tiling.
    pub fn new(material: impl Into<String>) -> Self {
        Self {
            material: material.into(),
            uv_scale: crate::texture_set::DEFAULT_UV_SCALE,
            detile: crate::texture_set::DEFAULT_DETILE,
        }
    }

    /// A vacated id: no material, but the id itself is held open.
    ///
    /// Removing a material leaves this behind. Every id above it keeps its
    /// number, so cells painted with those ids keep drawing what they
    /// drew; cells painted with this id draw the fallback until a material
    /// is added back into it.
    pub fn tombstone() -> Self {
        Self {
            material: String::new(),
            uv_scale: 0.0,
            detile: 0.0,
        }
    }

    /// Whether this slot holds an id open without a material behind it.
    pub fn is_tombstone(&self) -> bool {
        self.material.is_empty()
    }

    /// Clamp the floats into the bounds downstream code assumes:
    /// non-finite becomes the field's default, out of range clamps.
    ///
    /// A corrupt or hand-edited sidecar can carry a NaN here, and a NaN
    /// fails every comparison guarding it, including the shader's
    /// `strength <= 0.0`, so it would reach the sampler as a NaN UV.
    ///
    /// A tombstone is left alone: it draws nothing, and its zeroed floats
    /// are what it is written back out as.
    fn sanitize(&mut self) {
        if self.is_tombstone() {
            return;
        }
        self.uv_scale = if self.uv_scale.is_finite() {
            self.uv_scale.clamp(
                crate::texture_set::MIN_UV_SCALE,
                crate::texture_set::MAX_UV_SCALE,
            )
        } else {
            crate::texture_set::DEFAULT_UV_SCALE
        };
        self.detile = if self.detile.is_finite() {
            self.detile.clamp(0.0, crate::texture_set::MAX_DETILE)
        } else {
            crate::texture_set::DEFAULT_DETILE
        };
    }
}

/// Shallowest and steepest slope the autoterrain range can name. A
/// vertical face is 90 degrees.
pub const MIN_SLOPE_DEG: f32 = 0.0;
pub const MAX_SLOPE_DEG: f32 = 90.0;
/// Default autoterrain transition: gentle ground keeps the base texture,
/// a bank past 40 degrees is fully the slope one.
pub const DEFAULT_SLOPE_START_DEG: f32 = 25.0;
pub const DEFAULT_SLOPE_END_DEG: f32 = 40.0;
/// Narrowest band the two ends may be apart. The shader's `smoothstep`
/// divides by the band width, and flat ground lands on a 0-to-0 band
/// exactly (`acos(1.0)` is 0), evaluating 0/0 and shading every unclaimed
/// cell NaN.
pub const MIN_SLOPE_BAND_DEG: f32 = 0.5;

/// Bytes the version-4 autoterrain block occupies.
const AUTOTERRAIN_BLOCK_LEN: usize = 12;
/// Bytes the version-5 grid geometry block occupies.
const GRID_BLOCK_LEN: usize = 12;
/// Bytes the version-6 surface block occupies.
const SURFACE_BLOCK_LEN: usize = 8;
/// Bytes one version-7 placement record occupies: two indices and five
/// floats.
const PLACEMENT_LEN: usize = 24;
/// Bytes an empty version-7 scatter palette block occupies: the two table
/// counts, with no entries behind them.
#[cfg(test)]
const EMPTY_SCATTER_BLOCK_LEN: usize = 8;
/// Palette entry flags bit: placements of this asset block agents.
const SCATTER_FLAG_OBSTACLE: u8 = 0b0000_0001;
/// Autoterrain flags bit: the terrain textures its unclaimed
/// cells from their slope.
const AUTOTERRAIN_FLAG_ENABLED: u8 = 0b0000_0001;

/// How a terrain's whole surface is shaded, over and above what any one
/// slot says.
///
/// Both fields are authored dials rather than measurements, so both are
/// `0..1` and both default to what every pre-version-6 sidecar was
/// rendered at: the middle of the sharpness dial, and a colour layer
/// applied at full strength (white being no tint, an unpainted terrain
/// draws the same either way).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceSettings {
    /// How hard the height blend cuts between two texture ids, `0..1`.
    /// The shader remaps it onto a `4..64` power exponent: low is a soft
    /// cross-fade, high a near-binary cutout.
    pub blend_sharpness: f32,
    /// How much of the colour layer reaches the albedo, `0..1`. 0 draws
    /// the textures untinted; 1 multiplies them by the painted colour.
    pub tint_strength: f32,
}

impl Default for SurfaceSettings {
    fn default() -> Self {
        Self {
            blend_sharpness: DEFAULT_BLEND_SHARPNESS,
            tint_strength: DEFAULT_TINT_STRENGTH,
        }
    }
}

impl SurfaceSettings {
    /// [`Self::sanitize`] by value, for a caller passing these numbers on
    /// without knowing where they came from.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.sanitize();
        self
    }

    /// Clamp the block into the bounds the shader assumes: non-finite
    /// becomes the field's default, out of range clamps to `0..=1`.
    ///
    /// A NaN here fails every comparison guarding it and would reach the
    /// shader as a NaN exponent or a NaN mix factor, which paints the
    /// terrain black.
    pub fn sanitize(&mut self) {
        self.blend_sharpness = sane_unit(self.blend_sharpness, DEFAULT_BLEND_SHARPNESS);
        self.tint_strength = sane_unit(self.tint_strength, DEFAULT_TINT_STRENGTH);
    }
}

/// Blend sharpness a terrain shades at unless it says otherwise: the
/// middle of the dial. The shader remaps `0..1` onto a `4..64` power
/// exponent.
pub const DEFAULT_BLEND_SHARPNESS: f32 = 0.5;

/// Tint strength a terrain shades at unless it says otherwise: the whole
/// colour layer, which for an unpainted terrain is white and so no tint.
pub const DEFAULT_TINT_STRENGTH: f32 = 1.0;

/// A `0..1` dial, with non-finite falling back to `default`.
fn sane_unit(value: f32, default: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        default
    }
}

/// How a terrain textures the cells no hand has claimed.
///
/// Off until a terrain says otherwise, in which case the terrain draws
/// what its control map says and nothing else.
///
/// The two slots are texture ids into the same list the control map is
/// addressed by, so a slot naming a vacated or missing material draws the
/// fallback layer, the same as a cell painted with that id.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutoTerrainSettings {
    /// Whether cells without [`Control::manual`] take their texture from
    /// the geometry rather than from their control word.
    pub enabled: bool,
    /// Texture id flat ground draws.
    pub base_slot: u8,
    /// Texture id steep ground draws.
    pub slope_slot: u8,
    /// Slope at which the base texture starts giving way, in degrees.
    pub slope_start_deg: f32,
    /// Slope at which the slope texture has fully taken over, in degrees.
    pub slope_end_deg: f32,
}

impl Default for AutoTerrainSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            base_slot: 0,
            slope_slot: 1,
            slope_start_deg: DEFAULT_SLOPE_START_DEG,
            slope_end_deg: DEFAULT_SLOPE_END_DEG,
        }
    }
}

impl AutoTerrainSettings {
    /// Clamps the settings to the ranges the shader accepts, by value, for a
    /// caller passing these numbers on without knowing where they came from.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.sanitize();
        self
    }

    /// Clamp the block into the bounds the shader assumes.
    ///
    /// As in [`TerrainMaterialSlot::sanitize`], a NaN fails every
    /// comparison guarding it and would reach the shader's `smoothstep`.
    /// Non-finite becomes the field's default, out of range clamps to
    /// `0..=90`, and a range whose ends arrive the wrong way round is
    /// swapped, which keeps the width the author chose.
    ///
    /// The result is always at least [`MIN_SLOPE_BAND_DEG`] wide: the end
    /// is pushed up to make the width, or the start pulled down where the
    /// end is already against the 90-degree ceiling.
    fn sanitize(&mut self) {
        self.base_slot = self.base_slot.min(crate::control::MAX_TEXTURE_ID);
        self.slope_slot = self.slope_slot.min(crate::control::MAX_TEXTURE_ID);
        self.slope_start_deg = sane_degrees(self.slope_start_deg, DEFAULT_SLOPE_START_DEG);
        self.slope_end_deg = sane_degrees(self.slope_end_deg, DEFAULT_SLOPE_END_DEG);
        if self.slope_start_deg > self.slope_end_deg {
            core::mem::swap(&mut self.slope_start_deg, &mut self.slope_end_deg);
        }
        if self.slope_end_deg - self.slope_start_deg < MIN_SLOPE_BAND_DEG {
            self.slope_end_deg = self.slope_start_deg + MIN_SLOPE_BAND_DEG;
            if self.slope_end_deg > MAX_SLOPE_DEG {
                self.slope_end_deg = MAX_SLOPE_DEG;
                self.slope_start_deg = MAX_SLOPE_DEG - MIN_SLOPE_BAND_DEG;
            }
        }
    }
}

fn sane_degrees(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value.clamp(MIN_SLOPE_DEG, MAX_SLOPE_DEG)
    } else {
        fallback
    }
}

/// A terrain's bulk per-cell data: the heightmap plus every paint channel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainData {
    /// Vertices per edge. Every array below is `resolution^2` long.
    pub resolution: u32,
    /// Row-major heights.
    pub heights: Vec<f32>,
    /// Named integer layers, in the order the project declared them.
    pub channels: Vec<ChannelData>,
}

/// Why a sidecar could not be read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidecarError {
    /// The file does not start with [`MAGIC`]. Not a terrain sidecar.
    BadMagic,
    /// Written by a newer jackdaw than this one.
    UnsupportedVersion(u16),
    /// A reserved field was non-zero, so the file means something this
    /// build does not understand.
    ReservedFieldSet,
    /// The file ended before the declared data did, or had bytes left
    /// over once every declared field was read.
    Truncated,
    /// A channel declared an element tag this build does not know.
    UnknownElement(u8),
    /// A channel name was not valid UTF-8.
    BadName,
    /// The declared dimensions do not fit in this platform's address space.
    TooLarge,
    /// A region table declared an unusable region size.
    InvalidRegionSize(RegionSizeError),
    /// A material slot named something that could never address a material
    /// file.
    InvalidMaterialName(MaterialNameError),
    /// A region table declared the same region coordinate twice.
    DuplicateRegion(RegionCoord),
    /// A resolution is not a power of two, so it cannot become a region
    /// without resampling.
    UnmigratableResolution(u32),
    /// A scatter palette entry named something the asset system could
    /// never load.
    InvalidScatterAsset(ScatterAssetError),
    /// A scatter group table held a key no editor field could show.
    InvalidScatterGroup(ScatterGroupError),
    /// A placement named a palette or group index the file's own tables do
    /// not reach.
    UnknownScatterIndex,
}

impl core::fmt::Display for SidecarError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "not a terrain sidecar (bad magic)"),
            Self::UnsupportedVersion(v) => {
                write!(f, "terrain sidecar version {v} is newer than this build")
            }
            Self::ReservedFieldSet => write!(f, "terrain sidecar sets a reserved field"),
            Self::Truncated => write!(f, "terrain sidecar is truncated"),
            Self::UnknownElement(t) => write!(f, "unknown channel element tag {t}"),
            Self::BadName => write!(f, "channel name is not valid UTF-8"),
            Self::TooLarge => write!(f, "terrain sidecar declares more data than fits in memory"),
            Self::InvalidRegionSize(reason) => {
                write!(f, "terrain sidecar region size is invalid: {reason}")
            }
            Self::InvalidMaterialName(reason) => {
                write!(f, "terrain sidecar material slot is invalid: {reason}")
            }
            Self::DuplicateRegion(coord) => {
                write!(f, "terrain sidecar declares region {coord} twice")
            }
            Self::UnmigratableResolution(res) => write!(
                f,
                "terrain resolution {res} is not a power of two and cannot become a region without resampling"
            ),
            Self::InvalidScatterAsset(reason) => {
                write!(f, "terrain sidecar scatter palette is invalid: {reason}")
            }
            Self::InvalidScatterGroup(reason) => {
                write!(
                    f,
                    "terrain sidecar scatter group table is invalid: {reason}"
                )
            }
            Self::UnknownScatterIndex => write!(
                f,
                "terrain sidecar placement names a palette or group index the file does not declare"
            ),
        }
    }
}

impl core::error::Error for SidecarError {}

impl TerrainData {
    /// Cell count, or `None` if `resolution^2` overflows `usize`.
    pub fn cell_count(&self) -> Option<usize> {
        let res = self.resolution as usize;
        res.checked_mul(res)
    }

    /// Bring every array to exactly `resolution^2` entries, zero-filling.
    ///
    /// Called after a decode of a file whose arrays disagree with its
    /// header, and whenever a terrain's resolution changes.
    pub fn normalize(&mut self) {
        let Some(cells) = self.cell_count() else {
            return;
        };
        self.heights.resize(cells, 0.0);
        for channel in &mut self.channels {
            channel.values.resize(cells, 0);
        }
    }

    /// Byte length [`encode`] will produce, or `None` on overflow.
    pub fn encoded_len(&self) -> Option<usize> {
        let cells = self.cell_count()?;
        let mut len = 20usize.checked_add(cells.checked_mul(4)?)?;
        for channel in &self.channels {
            len = len
                .checked_add(8)?
                .checked_add(channel.name.len())?
                .checked_add(cells.checked_mul(channel.element.byte_width())?)?;
        }
        Some(len)
    }
}

/// Cursor over a sidecar's bytes that refuses to read past the end.
struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SidecarError> {
        let end = self.at.checked_add(n).ok_or(SidecarError::TooLarge)?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or(SidecarError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, SidecarError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, SidecarError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, SidecarError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn i32(&mut self) -> Result<i32, SidecarError> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}

/// The channel directory a version-5 file carries: name and width per
/// channel, and no values. The values live in the region table.
fn encode_channel_directory(out: &mut Vec<u8>, channels: &[ChannelDescriptor]) {
    for channel in channels {
        out.extend_from_slice(&(channel.name.len() as u32).to_le_bytes());
        out.extend_from_slice(channel.name.as_bytes());
        out.push(channel.element.tag());
        out.extend_from_slice(&[0u8; 3]);
    }
}

fn decode_channel_directory(
    r: &mut Reader<'_>,
    channel_count: u32,
) -> Result<Vec<ChannelDescriptor>, SidecarError> {
    let mut channels = Vec::new();
    for _ in 0..channel_count {
        let name_len = r.u32()? as usize;
        let name = core::str::from_utf8(r.take(name_len)?)
            .map_err(|_| SidecarError::BadName)?
            .to_string();
        let tag = r.u8()?;
        let element = ChannelElement::from_tag(tag).ok_or(SidecarError::UnknownElement(tag))?;
        if r.take(3)? != [0u8; 3] {
            return Err(SidecarError::ReservedFieldSet);
        }
        channels.push(ChannelDescriptor { name, element });
    }
    Ok(channels)
}

fn encode_channels(out: &mut Vec<u8>, channels: &[ChannelData], cells: usize) {
    for channel in channels {
        out.extend_from_slice(&(channel.name.len() as u32).to_le_bytes());
        out.extend_from_slice(channel.name.as_bytes());
        out.push(channel.element.tag());
        out.extend_from_slice(&[0u8; 3]);
        for i in 0..cells {
            let v = channel.values.get(i).copied().unwrap_or(0);
            match channel.element {
                ChannelElement::U8 => out.push(v.min(u16::from(u8::MAX)) as u8),
                ChannelElement::U16 => out.extend_from_slice(&v.to_le_bytes()),
            }
        }
    }
}

fn decode_channels(
    r: &mut Reader<'_>,
    channel_count: u32,
    cells: usize,
) -> Result<Vec<ChannelData>, SidecarError> {
    let mut channels = Vec::new();
    for _ in 0..channel_count {
        let name_len = r.u32()? as usize;
        let name = core::str::from_utf8(r.take(name_len)?)
            .map_err(|_| SidecarError::BadName)?
            .to_string();
        let tag = r.u8()?;
        let element = ChannelElement::from_tag(tag).ok_or(SidecarError::UnknownElement(tag))?;
        if r.take(3)? != [0u8; 3] {
            return Err(SidecarError::ReservedFieldSet);
        }

        let value_bytes = r.take(
            cells
                .checked_mul(element.byte_width())
                .ok_or(SidecarError::TooLarge)?,
        )?;
        let values = match element {
            ChannelElement::U8 => value_bytes.iter().map(|b| u16::from(*b)).collect(),
            ChannelElement::U16 => value_bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect(),
        };
        channels.push(ChannelData {
            name,
            element,
            values,
        });
    }
    Ok(channels)
}

/// Serialize a terrain's bulk data.
///
/// Arrays shorter than `resolution^2` are zero-padded and longer ones are
/// truncated, so the output always matches its own header. Returns `None`
/// only when the declared size overflows `usize`.
pub fn encode(data: &TerrainData) -> Option<Vec<u8>> {
    let cells = data.cell_count()?;
    let mut out = Vec::with_capacity(data.encoded_len()?);

    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&data.resolution.to_le_bytes());
    out.extend_from_slice(&(data.channels.len() as u32).to_le_bytes());

    for i in 0..cells {
        let h = data.heights.get(i).copied().unwrap_or(0.0);
        out.extend_from_slice(&h.to_le_bytes());
    }

    encode_channels(&mut out, &data.channels, cells);

    Some(out)
}

/// Deserialize a terrain's bulk data. Requires the file to end exactly
/// where the declared shape says it should.
pub fn decode(bytes: &[u8]) -> Result<TerrainData, SidecarError> {
    let mut r = Reader { bytes, at: 0 };

    if r.take(MAGIC.len())? != MAGIC {
        return Err(SidecarError::BadMagic);
    }
    let version = r.u16()?;
    // Version 0 is never valid: it would accept a file whose version
    // bytes were zeroed out by corruption.
    if version == 0 || version > VERSION {
        return Err(SidecarError::UnsupportedVersion(version));
    }
    if r.u16()? != 0 {
        return Err(SidecarError::ReservedFieldSet);
    }

    let resolution = r.u32()?;
    let channel_count = r.u32()?;
    let cells = (resolution as usize)
        .checked_mul(resolution as usize)
        .ok_or(SidecarError::TooLarge)?;

    // The remaining length is checked before every reservation, so a
    // truncated file cannot make this allocate its claimed size.
    let heights_bytes = r.take(cells.checked_mul(4).ok_or(SidecarError::TooLarge)?)?;
    let mut heights = Vec::with_capacity(cells);
    for chunk in heights_bytes.chunks_exact(4) {
        heights.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }

    let channels = decode_channels(&mut r, channel_count, cells)?;

    if r.at != bytes.len() {
        return Err(SidecarError::Truncated);
    }

    Ok(TerrainData {
        resolution,
        heights,
        channels,
    })
}

/// Where a terrain's cell grid sits, and how big its cells are.
///
/// The whole of a terrain's geometry. There is no declared rectangle:
/// which cells exist is the allocated regions' business, and this states
/// only where cell `(0, 0)` is and how far apart cells are.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridGeometry {
    /// World metres per cell edge.
    pub cell_size: f32,
    /// World-space offset of cell `(0, 0)` from the terrain entity's
    /// origin.
    ///
    /// Zero for a terrain authored against this format: cell `(0, 0)` sits
    /// at the entity. Nonzero only on a terrain migrated from a version-4
    /// or older sidecar, whose cells are addressed by a rectangle centred
    /// on the entity, giving that vintage's `-size/2`.
    pub anchor: bevy_math::Vec2,
}

impl GridGeometry {
    /// One metre per cell, cell `(0, 0)` at the entity.
    pub const DEFAULT: GridGeometry = GridGeometry {
        cell_size: 1.0,
        anchor: bevy_math::Vec2::ZERO,
    };

    /// The geometry a terrain declaring a square `size` metres across a
    /// `resolution`-vertex grid is drawn with.
    ///
    /// Vertex spacing is `size / (resolution - 1)`, since `resolution`
    /// counts vertices and a 256-vertex edge has 255 cells, and the
    /// rectangle is centred on the entity, putting its first vertex at
    /// `-size/2`.
    ///
    /// A cell is square, so only a square rectangle re-describes exactly.
    /// A rectangle asking for two different spacings is respaced to the
    /// one its X axis asked for, which moves its cells along Z;
    /// [`declared_rect_respacing`] reports that in advance.
    pub fn for_declared_rect(size: bevy_math::Vec2, resolution: u32) -> Self {
        Self {
            cell_size: size.x / (resolution.max(2) - 1) as f32,
            anchor: -size / 2.0,
        }
    }
}

impl Default for GridGeometry {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The two spacings a declared rectangle asked for, when it asked for
/// two different ones: `(x, z)`, in metres per cell.
///
/// `None` for a square rectangle, which
/// [`GridGeometry::for_declared_rect`] re-describes exactly. A caller
/// migrating an old file gets `Some` when the migration respaces that
/// terrain's cells along Z.
pub fn declared_rect_respacing(size: bevy_math::Vec2, resolution: u32) -> Option<(f32, f32)> {
    let cells = (resolution.max(2) - 1) as f32;
    let (x, z) = (size.x / cells, size.y / cells);
    (x != z).then_some((x, z))
}

/// The geometry to draw a terrain's stored cells at, given what its
/// sidecar says and the rectangle its component declares.
///
/// The sidecar wins whenever it states geometry: it travels with the
/// cells it describes, so a version-5 file beside scene text left stale
/// by a half-finished save still places its ground the same way. Only a
/// file too old to state geometry falls back to the declared rectangle.
///
/// The editor and the runtime both resolve through here, so a scene draws
/// the same ground in a game as in the editor that made it.
pub fn resolve_grid(
    stored: Option<GridGeometry>,
    declared_size: bevy_math::Vec2,
    declared_resolution: u32,
) -> GridGeometry {
    stored.unwrap_or_else(|| GridGeometry::for_declared_rect(declared_size, declared_resolution))
}

/// The dense grid a terrain presents: how many vertices it has, how much
/// ground they cover, and where the first one sits.
///
/// Derived, never declared. The resolution is how far the stored regions
/// reach, and the placement is the document's own geometry, so this
/// changes when an edit allocates a region.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridShape {
    /// Vertices per edge.
    pub resolution: u32,
    /// World-space XZ extent those vertices span.
    pub size: bevy_math::Vec2,
    /// Local-space position of grid vertex `(0, 0)`.
    pub origin: bevy_math::Vec2,
}

/// A terrain document: channels plus sparse region-based heights/control/
/// color, plus the materials the splat material paints with.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionTerrainData {
    /// Integer layers this terrain declares, in the order the project
    /// declared them. Names and widths only; each region carries a plane
    /// per entry, in this order.
    pub channels: Vec<ChannelDescriptor>,
    /// Sparse heights, control map, color and channel planes.
    pub regions: TerrainRegions,
    /// Materials this terrain paints with, in texture-id order: slot `i`
    /// is what a control word naming id `i` draws. Order is the id space,
    /// so a slot is never dropped, only replaced.
    pub materials: Vec<TerrainMaterialSlot>,
    /// How the cells no hand has claimed are textured. Off by default.
    pub autoterrain: AutoTerrainSettings,
    /// How the whole surface is shaded: blend sharpness and how much of
    /// the colour layer reaches the albedo.
    pub surface: SurfaceSettings,
    /// Where this document's cells sit in the world, or `None` when the
    /// file it came from is too old to state it.
    ///
    /// `None` is a version-4-or-older file, whose cells are placed by the
    /// `Terrain` component's declared rectangle. A caller holding that
    /// component resolves it with [`GridGeometry::for_declared_rect`]; the
    /// next save writes the result, and it reads as `Some` thereafter.
    pub grid: Option<GridGeometry>,
    /// The assets and stamp identities this document's stored scatter
    /// placements name. Empty on a document with no stored scatter, and
    /// on every file older than [`VERSION_7`].
    pub scatter: ScatterPalette,
}

/// An empty document: no channels, no regions, no materials, regions
/// sized [`RegionSize::DEFAULT`], at this format's own grid geometry.
///
/// The geometry is stated rather than `None`; `None` arises only from
/// decoding a file too old to state it.
impl Default for RegionTerrainData {
    fn default() -> Self {
        Self {
            channels: Vec::new(),
            regions: TerrainRegions::new(RegionSize::DEFAULT),
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        }
    }
}

impl RegionTerrainData {
    /// Vertices per edge of the dense grid this document presents: how far
    /// its regions reach, squared off to the longer axis. A document with
    /// no regions has no grid.
    pub fn grid_resolution(&self) -> u32 {
        self.regions.stored_extent().map_or(0, |(x, z)| x.max(z))
    }

    /// The dense grid this document presents, placed and scaled by the
    /// geometry its cells are drawn at.
    ///
    /// The extent comes from the regions and the placement from the file.
    /// The editor and the runtime both go through here, so a scene draws
    /// the same ground in a game as in the editor that made it.
    ///
    /// The declared rectangle is consulted only for a document from a file
    /// too old to state its geometry; see [`resolve_grid`].
    pub fn grid_shape(
        &self,
        declared_size: bevy_math::Vec2,
        declared_resolution: u32,
    ) -> GridShape {
        let grid = resolve_grid(self.grid, declared_size, declared_resolution);
        let resolution = self.grid_resolution();
        GridShape {
            resolution,
            size: bevy_math::Vec2::splat(resolution.saturating_sub(1) as f32 * grid.cell_size),
            origin: grid.anchor,
        }
    }

    /// The document's dense vertex grid as one borrowable region: the
    /// region at [`RegionCoord::ORIGIN`] when the whole grid fits inside
    /// it exactly.
    ///
    /// Callers addressing a terrain as one dense `resolution^2` array can
    /// borrow it in place when the grid is exactly one region, which is
    /// every power-of-two resolution. A grid spanning regions goes through
    /// [`Self::grid_heights`] and [`Self::set_grid_heights`], which read
    /// and write across as many regions as it covers.
    pub fn contiguous_grid(&self) -> Option<&Region> {
        self.regions
            .region(RegionCoord::ORIGIN)
            .filter(|region| region.side() == self.grid_resolution())
    }

    /// [`Self::contiguous_grid`], writable. Writing through it changes no
    /// region's presence.
    pub fn contiguous_grid_mut(&mut self) -> Option<&mut Region> {
        let side = self.grid_resolution();
        self.regions
            .region_mut(RegionCoord::ORIGIN)
            .filter(|region| region.side() == side)
    }

    /// The dense `channel_resolution`-per-edge height grid, gathered from
    /// every region it spans.
    pub fn grid_heights(&self) -> Vec<f32> {
        self.regions.read_grid_heights(self.grid_resolution())
    }

    /// The dense control grid, gathered the same way.
    pub fn grid_control(&self) -> Vec<Control> {
        self.regions.read_grid_control(self.grid_resolution())
    }

    /// Scatter a dense height grid back across the regions that own it.
    pub fn set_grid_heights(&mut self, heights: &[f32]) {
        self.regions
            .write_grid_heights(self.grid_resolution(), heights);
    }

    /// Scatter a dense control grid back across the regions that own it.
    pub fn set_grid_control(&mut self, control: &[Control]) {
        self.regions
            .write_grid_control(self.grid_resolution(), control);
    }

    /// Migrate a [`TerrainData`] into a region document: the legacy dense
    /// grid laid over regions sized by [`RegionSize::for_resolution`],
    /// with no control or color paint. Present even if every height is
    /// default. Channels move across unchanged.
    ///
    /// A resolution that is not a power of two is stored, not resampled:
    /// it spans two regions per axis and the vertices past the first
    /// region are held by the next one, so the migration keeps every
    /// height the file carried. A resolution of 0 migrates to zero
    /// regions under [`RegionSize::DEFAULT`].
    ///
    /// Such a grid is embedded, and the terrain it becomes is wider than
    /// the grid: regions are allocated whole, so a 129-vertex grid becomes
    /// a 256-cell terrain. The cells past the authored grid read as height
    /// zero and are flat ground to the mesher, the export manifest and the
    /// navmesh bake alike. The migration keeps every authored value in
    /// place; it does not keep the terrain's edge.
    ///
    /// Channels are embedded the same way and against the same anchor, so
    /// a channel value stays on the height it described.
    pub fn from_legacy_v1(data: &TerrainData) -> Result<Self, SidecarError> {
        let mut normalized = data.clone();
        normalized.normalize();
        let resolution = normalized.resolution;

        let mut regions = TerrainRegions::new(RegionSize::for_resolution(resolution));
        if resolution != 0 {
            regions.write_grid_heights(resolution, &normalized.heights);
        }
        // The dense channel grid shares the heights' anchor, so slicing it
        // into the regions leaves every value on the cell it describes.
        regions.set_channel_count(normalized.channels.len());
        for (index, channel) in normalized.channels.iter().enumerate() {
            regions.write_grid_channel(index, resolution, &channel.values);
        }
        let directory = normalized
            .channels
            .iter()
            .map(|channel| ChannelDescriptor {
                name: channel.name.clone(),
                element: channel.element,
            })
            .collect();

        Ok(Self {
            channels: directory,
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            // Version 1 states no geometry, so its cells are placed by the
            // component's rectangle.
            grid: None,
            scatter: ScatterPalette::default(),
        })
    }

    /// A [`TerrainData`] view of this document: the regions its dense
    /// vertex grid covers, with no control or color paint. Returns `None`
    /// once any of that stops holding, rather than flattening data it
    /// cannot represent.
    ///
    /// A region outside the grid holds ground the legacy format has no
    /// coordinate for, so it refuses rather than dropping it.
    pub fn as_legacy(&self) -> Option<TerrainData> {
        if self.autoterrain != AutoTerrainSettings::default() {
            return None;
        }
        let span = self.regions.grid_span(self.grid_resolution()) as i32;
        for (coord, region) in self.regions.iter_sorted() {
            if coord.x < 0 || coord.z < 0 || coord.x >= span || coord.z >= span {
                return None;
            }
            if region.color().is_some() {
                return None;
            }
            if region
                .control_words()
                .iter()
                .any(|c| *c != Control::default())
            {
                return None;
            }
        }

        // The legacy shape carries each channel as one dense grid,
        // gathered back out of the regions holding it.
        let resolution = self.grid_resolution();
        let channels = self
            .channels
            .iter()
            .enumerate()
            .map(|(index, channel)| ChannelData {
                name: channel.name.clone(),
                element: channel.element,
                values: self.regions.read_grid_channel(index, resolution),
            })
            .collect();

        Some(TerrainData {
            resolution,
            heights: self.grid_heights(),
            channels,
        })
    }

    /// Bring the stored channel planes into agreement with the directory,
    /// zero-filling a plane the regions do not carry yet.
    ///
    /// Idempotent: the planes are per-region, so only their count is
    /// settled here, never their size or placement.
    pub fn normalize(&mut self) {
        self.regions.set_channel_count(self.channels.len());
    }

    /// Where the stored placements' offsets are measured from, and how
    /// wide a region is in world units.
    ///
    /// A document too old to state its geometry has none stored, so it
    /// carries no placements either and the default is never consulted for
    /// one.
    fn placement_frame(&self) -> (GridGeometry, f32) {
        let grid = self.grid.unwrap_or(GridGeometry::DEFAULT);
        let span = self.regions.region_size().get() as f32 * grid.cell_size;
        (grid, span)
    }

    /// The region covering terrain-local position `local`, and the offset
    /// within it a placement there would be stored at.
    pub fn placement_slot(&self, local: bevy_math::Vec3) -> (RegionCoord, f32, f32) {
        let (grid, span) = self.placement_frame();
        let from_anchor_x = local.x - grid.anchor.x;
        let from_anchor_z = local.z - grid.anchor.y;
        let coord = RegionCoord::new(
            (from_anchor_x / span).floor() as i32,
            (from_anchor_z / span).floor() as i32,
        );
        (
            coord,
            from_anchor_x - coord.x as f32 * span,
            from_anchor_z - coord.z as f32 * span,
        )
    }

    /// Terrain-local position of a placement stored on `coord`.
    pub fn placement_position(
        &self,
        coord: RegionCoord,
        placement: &ScatterPlacement,
    ) -> bevy_math::Vec3 {
        let (grid, span) = self.placement_frame();
        bevy_math::Vec3::new(
            grid.anchor.x + coord.x as f32 * span + placement.x,
            placement.y,
            grid.anchor.y + coord.z as f32 * span + placement.z,
        )
    }

    /// Store one instance at terrain-local `local`, allocating the region
    /// under it if the terrain does not reach there yet.
    ///
    /// Returns where it landed. `None` when the terrain is already at
    /// [`crate::region::MAX_REGIONS`] and the position needs a new one:
    /// scatter must not be the edit that grows a terrain past its cap.
    pub fn add_placement(
        &mut self,
        local: bevy_math::Vec3,
        group: u16,
        asset: u16,
        yaw: f32,
        scale: f32,
    ) -> Option<(RegionCoord, usize)> {
        let (mut coord, mut x, mut z) = self.placement_slot(local);
        // A position exactly on a region's minimum edge belongs to that
        // region by the floor above, but the last vertex of a terrain sits
        // exactly on the edge of the region past its last one. Storing it
        // there would allocate ground the terrain does not have, so an
        // edge position falls back into the region already holding it.
        let (_, span) = self.placement_frame();
        if self.regions.region(coord).is_none() {
            let back_x = x == 0.0;
            let back_z = z == 0.0;
            for (dx, dz) in [(1, 1), (1, 0), (0, 1)] {
                if (dx == 1 && !back_x) || (dz == 1 && !back_z) {
                    continue;
                }
                let candidate = RegionCoord::new(coord.x - dx, coord.z - dz);
                if self.regions.region(candidate).is_some() {
                    coord = candidate;
                    if dx == 1 {
                        x = span;
                    }
                    if dz == 1 {
                        z = span;
                    }
                    break;
                }
            }
        }
        if self.regions.region(coord).is_none() && !self.regions.has_room_for_one_more() {
            return None;
        }
        let mut placement = ScatterPlacement {
            group,
            asset,
            x,
            y: local.y,
            z,
            yaw,
            scale,
        };
        placement.sanitize();
        let region = self.regions.ensure_region(coord);
        region.placements_mut().push(placement);
        Some((coord, region.placements().len() - 1))
    }

    /// Every stored placement, by the region holding it and its index
    /// within that region, in region-coordinate order.
    pub fn placements(&self) -> impl Iterator<Item = (RegionCoord, usize, &ScatterPlacement)> {
        self.regions.iter_sorted().flat_map(|(coord, region)| {
            region
                .placements()
                .iter()
                .enumerate()
                .map(move |(index, placement)| (coord, index, placement))
        })
    }

    /// Every stored placement of one stamp identity.
    pub fn group_placements(
        &self,
        group: u16,
    ) -> impl Iterator<Item = (RegionCoord, usize, &ScatterPlacement)> {
        self.placements()
            .filter(move |(_, _, placement)| placement.group == group)
    }

    /// Every stored placement of one palette entry.
    pub fn asset_placements(
        &self,
        asset: u16,
    ) -> impl Iterator<Item = (RegionCoord, usize, &ScatterPlacement)> {
        self.placements()
            .filter(move |(_, _, placement)| placement.asset == asset)
    }

    /// One stored placement, by where it sits.
    pub fn placement(&self, coord: RegionCoord, index: usize) -> Option<&ScatterPlacement> {
        self.regions.region(coord)?.placements().get(index)
    }

    /// How many placements this document stores.
    pub fn placement_count(&self) -> usize {
        self.regions
            .iter_sorted()
            .map(|(_, region)| region.placements().len())
            .sum()
    }

    /// How many placements each stamp identity holds, in group-index
    /// order. A tombstoned or unused index counts zero.
    pub fn group_counts(&self) -> Vec<usize> {
        let mut counts = vec![0; self.scatter.groups.len()];
        for (_, _, placement) in self.placements() {
            if let Some(slot) = counts.get_mut(placement.group as usize) {
                *slot += 1;
            }
        }
        counts
    }

    /// Take one placement out. Indices after it in the same region shift
    /// down, so a caller removing several works from the back.
    pub fn remove_placement(
        &mut self,
        coord: RegionCoord,
        index: usize,
    ) -> Option<ScatterPlacement> {
        let region = self.regions.region_mut(coord)?;
        (index < region.placements().len()).then(|| region.placements_mut().remove(index))
    }

    /// Drop every placement of one stamp identity, keeping its key,
    /// returning how many were removed.
    ///
    /// What a re-run of a stamp does: the key stays where it is, so the
    /// replacement placements name the row the last run wrote rather than
    /// a new one.
    pub fn clear_group(&mut self, group: u16) -> usize {
        let mut removed = 0;
        for (_, region) in self.regions.iter_sorted_mut() {
            let placements = region.placements_mut();
            let before = placements.len();
            placements.retain(|placement| placement.group != group);
            removed += before - placements.len();
        }
        removed
    }

    /// Drop every placement of one stamp identity and tombstone its key,
    /// returning how many were removed.
    ///
    /// The key is tombstoned rather than removed because the index is what
    /// the remaining placements name, and renumbering the table here would
    /// re-point every one of them. The placements go first, which is what
    /// leaves the row free for the next key to take.
    pub fn remove_group(&mut self, group: u16) -> usize {
        let removed = self.clear_group(group);
        if let Some(key) = self.scatter.groups.get_mut(group as usize) {
            key.clear();
        }
        removed
    }

    /// Byte length [`encode_regions`] will produce, or `None` on overflow.
    pub fn encoded_len(&self) -> Option<usize> {
        let mut len = 12usize; // magic + version + flags
        len = len.checked_add(4)?; // channel_count
        for channel in &self.channels {
            // name length + name + element tag + padding
            len = len.checked_add(8)?.checked_add(channel.name.len())?;
        }
        // region_size + region_count + material_count
        len = len.checked_add(4)?.checked_add(4)?.checked_add(4)?;
        for slot in &self.materials {
            len = len
                .checked_add(4)?
                .checked_add(slot.material.len())?
                .checked_add(4)? // uv_scale
                .checked_add(4)?; // detile
        }

        for (_, region) in self.regions.iter_sorted() {
            let region_cells = (region.side() as usize).checked_mul(region.side() as usize)?;
            len = len.checked_add(8)?.checked_add(4)?; // coord + flags/pad
            len = len.checked_add(region_cells.checked_mul(4)?)?; // heights
            len = len.checked_add(region_cells.checked_mul(4)?)?; // control
            if region.color().is_some() {
                len = len.checked_add(region_cells.checked_mul(4)?)?; // color
            }
            for channel in &self.channels {
                len = len.checked_add(region_cells.checked_mul(channel.element.byte_width())?)?;
            }
            len = len
                .checked_add(4)?
                .checked_add(region.placements().len().checked_mul(PLACEMENT_LEN)?)?;
        }
        len = len.checked_add(AUTOTERRAIN_BLOCK_LEN)?;
        len = len.checked_add(GRID_BLOCK_LEN)?;
        len = len.checked_add(SURFACE_BLOCK_LEN)?;

        // Scatter palette block: two count-prefixed tables. Tombstoned
        // rows are not written, so they are not measured either.
        len = len.checked_add(4)?.checked_add(4)?;
        for entry in self.scatter.assets.iter().filter(|e| !e.is_tombstone()) {
            len = len
                .checked_add(4)?
                .checked_add(entry.asset.len())?
                .checked_add(4)? // flags + padding
                .checked_add(4)?; // cull distance
        }
        for key in self.scatter.groups.iter().filter(|k| !k.is_empty()) {
            len = len.checked_add(4)?.checked_add(key.len())?;
        }
        Some(len)
    }
}

/// Where each row of a side table lands once the tombstones are dropped,
/// or `None` for a row that is not written at all.
fn compact_table(tombstones: impl Iterator<Item = bool>) -> Vec<Option<u16>> {
    let mut next = 0u16;
    tombstones
        .map(|tombstone| {
            if tombstone {
                return None;
            }
            let at = next;
            next = next.saturating_add(1);
            Some(at)
        })
        .collect()
}

/// Serialize a terrain document. Refuses a malformed material name, and a
/// size that overflows `usize`.
pub fn encode_regions(data: &RegionTerrainData) -> Result<Vec<u8>, SidecarError> {
    for slot in &data.materials {
        validate_material_name(&slot.material).map_err(SidecarError::InvalidMaterialName)?;
    }
    for entry in &data.scatter.assets {
        if !entry.is_tombstone() {
            validate_scatter_asset(&entry.asset).map_err(SidecarError::InvalidScatterAsset)?;
        }
    }
    for key in &data.scatter.groups {
        if !key.is_empty() {
            validate_scatter_group(key).map_err(SidecarError::InvalidScatterGroup)?;
        }
    }
    // Tombstoned rows are dropped on the way out and the surviving ones
    // renumbered, so a document that has been stamped and cleared many
    // times does not carry a row per run in every future file. A
    // placement's index is remapped with them; one naming a row that is
    // not written has nothing to be remapped to.
    let assets = compact_table(
        data.scatter
            .assets
            .iter()
            .map(ScatterPaletteEntry::is_tombstone),
    );
    let groups = compact_table(data.scatter.groups.iter().map(String::is_empty));
    if data.placements().any(|(_, _, placement)| {
        assets
            .get(placement.asset as usize)
            .copied()
            .flatten()
            .is_none()
            || groups
                .get(placement.group as usize)
                .copied()
                .flatten()
                .is_none()
    }) {
        return Err(SidecarError::UnknownScatterIndex);
    }

    let mut out = Vec::with_capacity(data.encoded_len().ok_or(SidecarError::TooLarge)?);

    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION_7.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&(data.channels.len() as u32).to_le_bytes());
    encode_channel_directory(&mut out, &data.channels);

    out.extend_from_slice(&data.regions.region_size().get().to_le_bytes());
    out.extend_from_slice(&(data.regions.region_count() as u32).to_le_bytes());

    out.extend_from_slice(&(data.materials.len() as u32).to_le_bytes());
    for slot in &data.materials {
        out.extend_from_slice(&(slot.material.len() as u32).to_le_bytes());
        out.extend_from_slice(slot.material.as_bytes());
        out.extend_from_slice(&slot.uv_scale.to_le_bytes());
        out.extend_from_slice(&slot.detile.to_le_bytes());
    }

    let mut autoterrain = data.autoterrain;
    autoterrain.sanitize();
    out.push(if autoterrain.enabled {
        AUTOTERRAIN_FLAG_ENABLED
    } else {
        0
    });
    out.push(autoterrain.base_slot);
    out.push(autoterrain.slope_slot);
    out.push(0);
    out.extend_from_slice(&autoterrain.slope_start_deg.to_le_bytes());
    out.extend_from_slice(&autoterrain.slope_end_deg.to_le_bytes());

    // A document that states no geometry is written at the default.
    let grid = data.grid.unwrap_or(GridGeometry::DEFAULT);
    out.extend_from_slice(&grid.cell_size.to_le_bytes());
    out.extend_from_slice(&grid.anchor.x.to_le_bytes());
    out.extend_from_slice(&grid.anchor.y.to_le_bytes());

    let surface = data.surface.sanitized();
    out.extend_from_slice(&surface.blend_sharpness.to_le_bytes());
    out.extend_from_slice(&surface.tint_strength.to_le_bytes());

    out.extend_from_slice(&(assets.iter().flatten().count() as u32).to_le_bytes());
    for entry in data.scatter.assets.iter().filter(|e| !e.is_tombstone()) {
        let mut entry = entry.clone();
        entry.sanitize();
        out.extend_from_slice(&(entry.asset.len() as u32).to_le_bytes());
        out.extend_from_slice(entry.asset.as_bytes());
        out.push(if entry.obstacle {
            SCATTER_FLAG_OBSTACLE
        } else {
            0
        });
        out.extend_from_slice(&[0u8; 3]);
        out.extend_from_slice(&entry.cull_distance.to_le_bytes());
    }
    out.extend_from_slice(&(groups.iter().flatten().count() as u32).to_le_bytes());
    for key in data.scatter.groups.iter().filter(|k| !k.is_empty()) {
        out.extend_from_slice(&(key.len() as u32).to_le_bytes());
        out.extend_from_slice(key.as_bytes());
    }

    for (coord, region) in data.regions.iter_sorted() {
        out.extend_from_slice(&coord.x.to_le_bytes());
        out.extend_from_slice(&coord.z.to_le_bytes());
        let mut flags = REGION_FLAG_PRESENT;
        if region.color().is_some() {
            flags |= REGION_FLAG_HAS_COLOR;
        }
        out.push(flags);
        out.extend_from_slice(&[0u8; 3]);

        for h in region.heights() {
            out.extend_from_slice(&h.to_le_bytes());
        }
        for c in region.control_words() {
            out.extend_from_slice(&c.to_raw().to_le_bytes());
        }
        if let Some(color) = region.color() {
            for px in color {
                out.extend_from_slice(px);
            }
        }
        // One plane per declared channel, in directory order, at that
        // channel's own width. A region always carries every declared
        // channel, so the count here needs no per-region flag.
        let cells = (region.side() as usize) * (region.side() as usize);
        for (index, channel) in data.channels.iter().enumerate() {
            let plane = region.channel(index).unwrap_or(&[]);
            for at in 0..cells {
                let v = plane.get(at).copied().unwrap_or(0);
                match channel.element {
                    ChannelElement::U8 => out.push(v.min(u16::from(u8::MAX)) as u8),
                    ChannelElement::U16 => out.extend_from_slice(&v.to_le_bytes()),
                }
            }
        }

        out.extend_from_slice(&(region.placements().len() as u32).to_le_bytes());
        for placement in region.placements() {
            let mut placement = *placement;
            placement.sanitize();
            let group = groups[placement.group as usize].unwrap_or_default();
            let asset = assets[placement.asset as usize].unwrap_or_default();
            out.extend_from_slice(&group.to_le_bytes());
            out.extend_from_slice(&asset.to_le_bytes());
            out.extend_from_slice(&placement.x.to_le_bytes());
            out.extend_from_slice(&placement.y.to_le_bytes());
            out.extend_from_slice(&placement.z.to_le_bytes());
            out.extend_from_slice(&placement.yaw.to_le_bytes());
            out.extend_from_slice(&placement.scale.to_le_bytes());
        }
    }

    Ok(out)
}

/// Deserialize a terrain document. Requires the file to end exactly where
/// the declared shape says it should, and rejects a region table that
/// declares the same coordinate twice.
pub fn decode_regions(bytes: &[u8]) -> Result<RegionTerrainData, SidecarError> {
    let mut r = Reader { bytes, at: 0 };

    if r.take(MAGIC.len())? != MAGIC {
        return Err(SidecarError::BadMagic);
    }
    let version = r.u16()?;
    if !(VERSION_2..=VERSION_7).contains(&version) {
        return Err(SidecarError::UnsupportedVersion(version));
    }
    if r.u16()? != 0 {
        return Err(SidecarError::ReservedFieldSet);
    }

    // Before version 5 a channel is a dense grid of its own, sized by a
    // `channel_resolution` in the header and written out whole here.
    // Version 5 carries only the directory; the values are in the regions.
    let legacy_channel_resolution = if version < VERSION_5 { r.u32()? } else { 0 };
    let channel_count = r.u32()?;
    let (channels, legacy_channels) = if version < VERSION_5 {
        let cells = (legacy_channel_resolution as usize)
            .checked_mul(legacy_channel_resolution as usize)
            .ok_or(SidecarError::TooLarge)?;
        let dense = decode_channels(&mut r, channel_count, cells)?;
        let directory = dense
            .iter()
            .map(|channel| ChannelDescriptor {
                name: channel.name.clone(),
                element: channel.element,
            })
            .collect();
        (directory, dense)
    } else {
        (decode_channel_directory(&mut r, channel_count)?, Vec::new())
    };

    let region_size_raw = r.u32()?;
    let region_size = RegionSize::new(region_size_raw).map_err(SidecarError::InvalidRegionSize)?;
    let region_count = r.u32()?;

    let material_count = r.u32()?;
    let mut materials = Vec::new();
    for _ in 0..material_count {
        let name_len = r.u32()? as usize;
        let material = core::str::from_utf8(r.take(name_len)?)
            .map_err(|_| SidecarError::BadName)?
            .to_string();
        validate_material_name(&material).map_err(SidecarError::InvalidMaterialName)?;
        let bytes = r.take(4)?;
        let uv_scale = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
        // Version 2 has no detiling field and loads as off.
        let detile = if version >= VERSION_3 {
            let bytes = r.take(4)?;
            f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
        } else {
            0.0
        };
        let mut slot = TerrainMaterialSlot {
            material,
            uv_scale,
            detile,
        };
        slot.sanitize();
        materials.push(slot);
    }

    // Versions 2 and 3 have no autoterrain block and load with it off.
    let mut autoterrain = AutoTerrainSettings::default();
    if version >= VERSION_4 {
        let flags = r.u8()?;
        if flags & !AUTOTERRAIN_FLAG_ENABLED != 0 {
            return Err(SidecarError::ReservedFieldSet);
        }
        autoterrain.enabled = flags & AUTOTERRAIN_FLAG_ENABLED != 0;
        autoterrain.base_slot = r.u8()?;
        autoterrain.slope_slot = r.u8()?;
        if r.u8()? != 0 {
            return Err(SidecarError::ReservedFieldSet);
        }
        let degrees = r.take(8)?;
        autoterrain.slope_start_deg =
            f32::from_le_bytes([degrees[0], degrees[1], degrees[2], degrees[3]]);
        autoterrain.slope_end_deg =
            f32::from_le_bytes([degrees[4], degrees[5], degrees[6], degrees[7]]);
        autoterrain.sanitize();
    }

    // Versions 2 through 4 state no grid geometry: their cells are placed
    // by the component's declared rectangle, which the caller resolves.
    let grid = if version >= VERSION_5 {
        let bytes = r.take(12)?;
        let f = |i: usize| f32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        Some(GridGeometry {
            cell_size: f(0),
            anchor: bevy_math::Vec2::new(f(4), f(8)),
        })
    } else {
        None
    };

    // Versions 2 through 5 state no surface block: they were rendered at
    // the defaults, which is what they load as.
    let mut surface = SurfaceSettings::default();
    if version >= VERSION_6 {
        let bytes = r.take(8)?;
        let f = |i: usize| f32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        surface = SurfaceSettings {
            blend_sharpness: f(0),
            tint_strength: f(4),
        }
        .sanitized();
    }

    // Versions 2 through 6 carry their scatter as scene entities and
    // state no palette, so they load with an empty one and no placements.
    let mut scatter = ScatterPalette::default();
    if version >= VERSION_7 {
        let asset_count = r.u32()?;
        for _ in 0..asset_count {
            let name_len = r.u32()? as usize;
            let asset = core::str::from_utf8(r.take(name_len)?)
                .map_err(|_| SidecarError::BadName)?
                .to_string();
            if !asset.is_empty() {
                validate_scatter_asset(&asset).map_err(SidecarError::InvalidScatterAsset)?;
            }
            let flags = r.u8()?;
            if flags & !SCATTER_FLAG_OBSTACLE != 0 {
                return Err(SidecarError::ReservedFieldSet);
            }
            if r.take(3)? != [0u8; 3] {
                return Err(SidecarError::ReservedFieldSet);
            }
            let bytes = r.take(4)?;
            let mut entry = ScatterPaletteEntry {
                asset,
                obstacle: flags & SCATTER_FLAG_OBSTACLE != 0,
                cull_distance: f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
            };
            entry.sanitize();
            scatter.assets.push(entry);
        }
        let group_count = r.u32()?;
        for _ in 0..group_count {
            let name_len = r.u32()? as usize;
            let key = core::str::from_utf8(r.take(name_len)?)
                .map_err(|_| SidecarError::BadName)?
                .to_string();
            if !key.is_empty() {
                validate_scatter_group(&key).map_err(SidecarError::InvalidScatterGroup)?;
            }
            scatter.groups.push(key);
        }
    }

    let cells = (region_size_raw as usize)
        .checked_mul(region_size_raw as usize)
        .ok_or(SidecarError::TooLarge)?;

    let mut regions = TerrainRegions::new(region_size);
    let mut seen = HashSet::with_capacity(region_count.min(1024) as usize);
    for _ in 0..region_count {
        let x = r.i32()?;
        let z = r.i32()?;
        let flags = r.u8()?;
        if flags & !REGION_FLAGS_KNOWN != 0 {
            return Err(SidecarError::ReservedFieldSet);
        }
        if flags & REGION_FLAG_PRESENT == 0 {
            return Err(SidecarError::ReservedFieldSet);
        }
        if r.take(3)? != [0u8; 3] {
            return Err(SidecarError::ReservedFieldSet);
        }
        let has_color = flags & REGION_FLAG_HAS_COLOR != 0;

        let heights_bytes = r.take(cells.checked_mul(4).ok_or(SidecarError::TooLarge)?)?;
        let heights: Vec<f32> = heights_bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        let control_bytes = r.take(cells.checked_mul(4).ok_or(SidecarError::TooLarge)?)?;
        let mut control = Vec::with_capacity(cells);
        for chunk in control_bytes.chunks_exact(4) {
            let raw = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            let word = Control::from_raw(raw);
            if word.reserved() != 0 {
                return Err(SidecarError::ReservedFieldSet);
            }
            control.push(word);
        }

        let color = if has_color {
            let color_bytes = r.take(cells.checked_mul(4).ok_or(SidecarError::TooLarge)?)?;
            Some(
                color_bytes
                    .chunks_exact(4)
                    .map(|c| [c[0], c[1], c[2], c[3]])
                    .collect(),
            )
        } else {
            None
        };

        // Version 5 stores a plane per declared channel here. Older files
        // have none: their values arrive as one dense grid and are laid
        // into the regions once every region is read.
        let mut planes = Vec::with_capacity(channels.len());
        if version >= VERSION_5 {
            for channel in &channels {
                let width = channel.element.byte_width();
                let plane_bytes =
                    r.take(cells.checked_mul(width).ok_or(SidecarError::TooLarge)?)?;
                let plane: Vec<u16> = match channel.element {
                    ChannelElement::U8 => plane_bytes.iter().map(|b| u16::from(*b)).collect(),
                    ChannelElement::U16 => plane_bytes
                        .chunks_exact(2)
                        .map(|c| u16::from_le_bytes([c[0], c[1]]))
                        .collect(),
                };
                planes.push(plane);
            }
        } else {
            planes.resize(channels.len(), vec![0; cells]);
        }

        // Version 7 stores this region's scatter here. An older file has
        // none: its scatter is scene entities.
        let mut placements = Vec::new();
        if version >= VERSION_7 {
            let placement_count = r.u32()? as usize;
            placements.reserve(placement_count.min(4096));
            for _ in 0..placement_count {
                let bytes = r.take(PLACEMENT_LEN)?;
                let f = |i: usize| {
                    f32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]])
                };
                let group = u16::from_le_bytes([bytes[0], bytes[1]]);
                let asset = u16::from_le_bytes([bytes[2], bytes[3]]);
                if usize::from(group) >= scatter.groups.len()
                    || usize::from(asset) >= scatter.assets.len()
                {
                    return Err(SidecarError::UnknownScatterIndex);
                }
                let mut placement = ScatterPlacement {
                    group,
                    asset,
                    x: f(4),
                    y: f(8),
                    z: f(12),
                    yaw: f(16),
                    scale: f(20),
                };
                placement.sanitize();
                placements.push(placement);
            }
        }

        let coord = RegionCoord::new(x, z);
        if !seen.insert(coord) {
            return Err(SidecarError::DuplicateRegion(coord));
        }
        regions.insert_region(
            coord,
            Region::from_parts(region_size_raw, heights, control, color, planes, placements),
        );
    }
    regions.set_channel_count(channels.len());

    // A pre-version-5 file's channels are a dense grid anchored at cell
    // (0, 0), the same anchor the heights use, so every value lands in the
    // region owning the cell it describes. The spacing is the resolution
    // ahead of the channel table, the one the heights are read at, not the
    // regions' extent: regions are allocated whole and reach past the grid
    // a file declares, so spreading a channel over that wider ground would
    // take every value off the height it describes.
    for (index, dense) in legacy_channels.iter().enumerate() {
        regions.write_grid_channel(index, legacy_channel_resolution, &dense.values);
    }

    if r.at != bytes.len() {
        return Err(SidecarError::Truncated);
    }

    Ok(RegionTerrainData {
        channels,
        regions,
        materials,
        autoterrain,
        surface,
        grid,
        scatter,
    })
}

/// Load a sidecar of either format version, upgrading version 1 to a
/// region document and normalizing it before returning. Refuses a file
/// written by a newer build, and a version-1 file whose resolution cannot
/// become a region.
pub fn load(bytes: &[u8]) -> Result<RegionTerrainData, SidecarError> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(MAGIC.len())? != MAGIC {
        return Err(SidecarError::BadMagic);
    }
    let version = r.u16()?;

    let mut data = match version {
        0 => Err(SidecarError::UnsupportedVersion(0)),
        VERSION => decode(bytes).and_then(|legacy| RegionTerrainData::from_legacy_v1(&legacy)),
        VERSION_2..=VERSION_7 => decode_regions(bytes),
        other => Err(SidecarError::UnsupportedVersion(other)),
    }?;
    data.normalize();
    Ok(data)
}

/// Serialize a terrain document as the current format version. The inverse
/// of [`load`] for any file [`load`] can produce.
pub fn save(data: &RegionTerrainData) -> Result<Vec<u8>, SidecarError> {
    encode_regions(data)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::region::DEFAULT_COLOR;
    use crate::texture_set::{
        DEFAULT_DETILE, DEFAULT_UV_SCALE, MAX_DETILE, MAX_UV_SCALE, MIN_UV_SCALE,
    };

    fn sample() -> TerrainData {
        let mut biome = ChannelData::new("biome", ChannelElement::U8, 4);
        biome.values[5] = 2;
        biome.values[6] = 255;
        let mut zoning = ChannelData::new("zoning-\u{e9}", ChannelElement::U16, 4);
        zoning.values[0] = 40000;
        TerrainData {
            resolution: 4,
            heights: (0..16).map(|i| i as f32 * 0.25).collect(),
            channels: vec![biome, zoning],
        }
    }

    #[test]
    fn round_trips_heights_and_channels_exactly() {
        let data = sample();
        let bytes = encode(&data).expect("encodes");
        let back = decode(&bytes).expect("decodes");
        assert_eq!(back, data);
    }

    #[test]
    fn encoding_is_deterministic() {
        let data = sample();
        assert_eq!(encode(&data), encode(&data));
    }

    #[test]
    fn encoded_len_matches_the_bytes_produced() {
        let data = sample();
        let bytes = encode(&data).expect("encodes");
        assert_eq!(Some(bytes.len()), data.encoded_len());
    }

    #[test]
    fn a_u8_channel_costs_half_of_a_u16_channel() {
        let res = 8;
        let narrow = TerrainData {
            resolution: res,
            heights: vec![0.0; 64],
            channels: vec![ChannelData::new("c", ChannelElement::U8, res)],
        };
        let wide = TerrainData {
            resolution: res,
            heights: vec![0.0; 64],
            channels: vec![ChannelData::new("c", ChannelElement::U16, res)],
        };
        let narrow_len = encode(&narrow).expect("encodes").len();
        let wide_len = encode(&wide).expect("encodes").len();
        assert_eq!(wide_len - narrow_len, 64);
    }

    #[test]
    fn a_512_heightmap_is_a_megabyte_of_binary_not_text_floats() {
        let res = 512u32;
        let data = TerrainData {
            resolution: res,
            heights: vec![0.5; (res * res) as usize],
            channels: vec![],
        };
        let bytes = encode(&data).expect("encodes");
        assert_eq!(bytes.len(), 20 + 512 * 512 * 4);
        assert_eq!(decode(&bytes).expect("decodes").heights.len(), 262_144);
    }

    #[test]
    fn u8_channel_values_saturate_rather_than_wrap() {
        let mut c = ChannelData::new("c", ChannelElement::U8, 2);
        c.values = vec![300, 0, 0, 0];
        let data = TerrainData {
            resolution: 2,
            heights: vec![0.0; 4],
            channels: vec![c],
        };
        let back = decode(&encode(&data).expect("encodes")).expect("decodes");
        assert_eq!(back.channels[0].values[0], 255);
    }

    #[test]
    fn short_arrays_are_zero_padded_to_the_declared_resolution() {
        let data = TerrainData {
            resolution: 3,
            heights: vec![1.0, 2.0],
            channels: vec![ChannelData {
                name: "c".into(),
                element: ChannelElement::U16,
                values: vec![7],
            }],
        };
        let back = decode(&encode(&data).expect("encodes")).expect("decodes");
        assert_eq!(
            back.heights,
            vec![1.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(back.channels[0].values.len(), 9);
        assert_eq!(back.channels[0].values[0], 7);
    }

    #[test]
    fn an_empty_terrain_round_trips() {
        let data = TerrainData::default();
        let back = decode(&encode(&data).expect("encodes")).expect("decodes");
        assert_eq!(back, data);
    }

    #[test]
    fn rejects_a_file_that_is_not_a_sidecar() {
        assert_eq!(decode(b"not a terrain at all"), Err(SidecarError::BadMagic));
        assert_eq!(decode(b""), Err(SidecarError::Truncated));
    }

    #[test]
    fn rejects_a_newer_format_version() {
        let mut bytes = encode(&sample()).expect("encodes");
        bytes[8..10].copy_from_slice(&(VERSION + 1).to_le_bytes());
        assert_eq!(
            decode(&bytes),
            Err(SidecarError::UnsupportedVersion(VERSION + 1))
        );
    }

    #[test]
    fn rejects_version_zero() {
        let mut bytes = encode(&sample()).expect("encodes");
        bytes[8..10].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(decode(&bytes), Err(SidecarError::UnsupportedVersion(0)));
    }

    #[test]
    fn rejects_a_set_reserved_flag() {
        let mut bytes = encode(&sample()).expect("encodes");
        bytes[10..12].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(decode(&bytes), Err(SidecarError::ReservedFieldSet));
    }

    #[test]
    fn rejects_an_unknown_channel_element() {
        let data = TerrainData {
            resolution: 2,
            heights: vec![0.0; 4],
            channels: vec![ChannelData::new("c", ChannelElement::U8, 2)],
        };
        let mut bytes = encode(&data).expect("encodes");
        // header 20 + heights 16 + name_len 4 + name 1 = 41 is the tag byte.
        bytes[41] = 9;
        assert_eq!(decode(&bytes), Err(SidecarError::UnknownElement(9)));
    }

    #[test]
    fn a_truncated_file_is_rejected_and_does_not_allocate_its_claim() {
        let bytes = encode(&sample()).expect("encodes");
        for cut in [20, 30, bytes.len() - 1] {
            assert_eq!(decode(&bytes[..cut]), Err(SidecarError::Truncated));
        }
    }

    #[test]
    fn a_header_claiming_a_huge_terrain_over_a_tiny_file_is_rejected() {
        let mut bytes = encode(&TerrainData::default()).expect("encodes");
        bytes[12..16].copy_from_slice(&100_000u32.to_le_bytes());
        assert_eq!(decode(&bytes), Err(SidecarError::Truncated));
    }

    #[test]
    fn rejects_trailing_bytes_after_a_complete_v1_document() {
        let mut bytes = encode(&sample()).expect("encodes");
        bytes.push(0);
        assert_eq!(decode(&bytes), Err(SidecarError::Truncated));
    }

    #[test]
    fn rejects_an_under_claimed_channel_count_that_leaves_bytes_unconsumed() {
        let data = TerrainData {
            resolution: 2,
            heights: vec![0.0; 4],
            channels: vec![
                ChannelData::new("a", ChannelElement::U8, 2),
                ChannelData::new("b", ChannelElement::U8, 2),
            ],
        };
        let mut bytes = encode(&data).expect("encodes");
        bytes[16..20].copy_from_slice(&1u32.to_le_bytes()); // claim only 1 of 2
        assert_eq!(decode(&bytes), Err(SidecarError::Truncated));
    }

    #[test]
    fn normalize_squares_every_array_to_the_resolution() {
        let mut data = TerrainData {
            resolution: 3,
            heights: vec![1.0; 2],
            channels: vec![ChannelData {
                name: "c".into(),
                element: ChannelElement::U8,
                values: vec![1; 20],
            }],
        };
        data.normalize();
        assert_eq!(data.heights.len(), 9);
        assert_eq!(data.channels[0].values.len(), 9);
    }

    #[test]
    fn resolves_nested_sidecars_beneath_the_scene_directory() {
        let scene_dir = Path::new("project").join("scenes");

        assert_eq!(
            resolve_path(&scene_dir, "terrain/chunks/ground.jdterrain"),
            Ok(scene_dir.join("terrain/chunks/ground.jdterrain")),
        );
    }

    #[test]
    fn rejects_paths_that_can_escape_or_change_meaning_across_platforms() {
        let scene_dir = Path::new("project/scenes");
        let invalid = [
            "",
            ".",
            "..",
            "./ground.jdterrain",
            "terrain/./ground.jdterrain",
            "terrain/../ground.jdterrain",
            "terrain//ground.jdterrain",
            "/tmp/ground.jdterrain",
            "//server/share/ground.jdterrain",
            "C:/terrain/ground.jdterrain",
            r"C:\terrain\ground.jdterrain",
            r"terrain\ground.jdterrain",
            "terrain/ground:stream.jdterrain",
            "ground.terrain",
            "ground.jdterrain.bak",
            "ground.JDTERRAIN",
        ];

        for data_path in invalid {
            assert!(
                resolve_path(scene_dir, data_path).is_err(),
                "{data_path:?} must not resolve",
            );
        }
    }

    fn sample_regions() -> RegionTerrainData {
        let mut regions = TerrainRegions::new(RegionSize::new(4).unwrap());
        regions.set_height(0, 0, 1.5);
        regions.set_height(3, 3, -2.25);
        regions.set_control(1, 1, Control::default().with_base_id(3).with_blend(40));
        regions.set_color(2, 2, [10, 20, 30, 255]);
        // A second region at negative coordinates, covering multi-region
        // and negative-coord encode/decode together.
        regions.set_height(-1, -4, 9.0);

        // One declared channel, painted in the region at the origin, so a
        // round trip carries a plane that is not all zero next to one
        // that is.
        regions.set_channel_count(1);
        regions.set_channel(0, 0, 0, 7);

        RegionTerrainData {
            channels: vec![ChannelDescriptor::new("biome", ChannelElement::U8)],
            regions,
            materials: vec![
                TerrainMaterialSlot::new("grass"),
                TerrainMaterialSlot {
                    material: "rock_05".to_string(),
                    uv_scale: 0.25,
                    detile: 0.7,
                },
            ],
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        }
    }

    #[test]
    fn v2_round_trips_regions_channels_and_materials() {
        let data = sample_regions();
        let bytes = encode_regions(&data).expect("encodes");
        let back = decode_regions(&bytes).expect("decodes");
        assert_eq!(back, data);
    }

    #[test]
    fn v2_encoding_is_deterministic_regardless_of_edit_order() {
        let mut a = TerrainRegions::new(RegionSize::new(4).unwrap());
        a.set_height(0, 0, 1.0);
        a.set_height(20, 20, 2.0);
        a.set_height(-9, 4, 3.0);
        let mut b = TerrainRegions::new(RegionSize::new(4).unwrap());
        b.set_height(-9, 4, 3.0);
        b.set_height(20, 20, 2.0);
        b.set_height(0, 0, 1.0);

        let da = RegionTerrainData {
            channels: vec![],
            regions: a,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let db = RegionTerrainData {
            channels: vec![],
            regions: b,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        assert_eq!(encode_regions(&da), encode_regions(&db));
    }

    #[test]
    fn v2_with_no_materials_round_trips_to_an_empty_list() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, 1.0);
        let data = RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let back = decode_regions(&encode_regions(&data).expect("encodes")).expect("decodes");
        assert!(back.materials.is_empty());
    }

    #[test]
    fn v2_with_an_empty_region_set_round_trips() {
        let data = RegionTerrainData {
            channels: vec![],
            regions: TerrainRegions::new(RegionSize::new(64).unwrap()),
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let back = decode_regions(&encode_regions(&data).expect("encodes")).expect("decodes");
        assert_eq!(back, data);
    }

    #[test]
    fn v2_round_trip_preserves_an_explicitly_declared_all_default_region() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, 1.0);
        regions.set_height(0, 0, 0.0);
        let data = RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        assert_eq!(data.regions.region_count(), 1);

        let bytes = encode_regions(&data).expect("encodes");
        let decoded = decode_regions(&bytes).expect("decodes");
        assert_eq!(
            decoded.regions.region_count(),
            1,
            "the authored, now-default region survives the round trip"
        );
        assert_eq!(decoded, data);

        let bytes_again = encode_regions(&decoded).expect("encodes");
        assert_eq!(bytes, bytes_again);
    }

    #[test]
    fn v2_round_trips_nan_and_negative_zero_heights_bit_exact() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, f32::NAN);
        regions.set_height(1, 0, -0.0);
        let data = RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let bytes = encode_regions(&data).expect("encodes");
        let back = decode_regions(&bytes).expect("decodes");

        let original = data.regions.region(RegionCoord::ORIGIN).unwrap();
        let round_tripped = back.regions.region(RegionCoord::ORIGIN).unwrap();
        // NaN != NaN; compare bits.
        for (a, b) in original.heights().iter().zip(round_tripped.heights()) {
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "NaN/-0.0 must round-trip bit exact"
            );
        }
    }

    #[test]
    fn migrating_a_real_v1_file_produces_a_single_origin_region() {
        let migrated = RegionTerrainData::from_legacy_v1(&sample()).unwrap();
        assert_eq!(migrated.grid_resolution(), 4);
        // The directory keeps every channel the file declared, name and
        // width, in order.
        let declared: Vec<ChannelDescriptor> = sample()
            .channels
            .iter()
            .map(|c| ChannelDescriptor::new(c.name.clone(), c.element))
            .collect();
        assert_eq!(migrated.channels, declared);
        // The dense grid the file carries becomes a plane in the region
        // owning those cells, value for value on the same ground.
        for (at, want) in sample().channels[0].values.iter().enumerate() {
            let (x, z) = ((at % 4) as i32, (at / 4) as i32);
            assert_eq!(migrated.regions.channel_at(0, x, z), *want);
        }
        assert_eq!(migrated.regions.region_count(), 1);
        assert!(migrated.materials.is_empty());
        let region = migrated.regions.region(RegionCoord::ORIGIN).unwrap();
        assert_eq!(region.side(), 4);
        assert_eq!(region.heights(), sample().heights.as_slice());
        assert!(
            region
                .control_words()
                .iter()
                .all(|c| *c == Control::default())
        );
        assert!(region.color().is_none());
    }

    #[test]
    fn migrating_an_empty_v1_terrain_allocates_no_regions() {
        let migrated = RegionTerrainData::from_legacy_v1(&TerrainData::default()).unwrap();
        assert_eq!(migrated.regions.region_count(), 0);
        assert_eq!(migrated.grid_resolution(), 0);
    }

    #[test]
    fn migrating_a_flat_nonzero_resolution_v1_terrain_still_produces_a_present_region() {
        let data = TerrainData {
            resolution: 4,
            heights: vec![0.0; 16],
            channels: vec![],
        };
        let migrated = RegionTerrainData::from_legacy_v1(&data).unwrap();
        assert_eq!(migrated.regions.region_count(), 1);
    }

    /// A vertex grid that is not a power of two is stored, not resampled:
    /// it runs one region past the first, and the vertices out there are
    /// held by that next region. The resulting terrain is those whole
    /// regions, so it reaches further than the grid.
    #[test]
    fn migrating_a_non_power_of_two_legacy_resolution_embeds_every_height() {
        let data = TerrainData {
            resolution: 3,
            heights: (0..9).map(|i| i as f32).collect(),
            channels: vec![],
        };
        let migrated = RegionTerrainData::from_legacy_v1(&data).expect("migrates");
        assert_eq!(migrated.regions.region_size().get(), 2);
        assert_eq!(migrated.regions.region_count(), 4);
        // Four 2-cell regions hold a 3-vertex grid, so the terrain is four
        // cells a side with the authored three embedded in the corner.
        assert_eq!(migrated.grid_resolution(), 4);
        assert_embedded(&migrated.regions, &data.heights, 3, 4);
    }

    /// Channels embed against the same anchor the heights do, so a
    /// painted value stays on the cell it describes rather than sliding
    /// by the difference between the grid and the region block.
    #[test]
    fn a_dense_channel_grid_embeds_onto_the_cells_it_described() {
        let mut channel = ChannelData::new("biome", ChannelElement::U8, 3);
        channel.values = vec![1, 2, 3, 4, 5, 6, 7, 8, 9];
        let data = TerrainData {
            resolution: 3,
            heights: (0..9).map(|i| i as f32).collect(),
            channels: vec![channel.clone()],
        };

        let migrated = RegionTerrainData::from_legacy_v1(&data).expect("migrates");
        assert_eq!(migrated.grid_resolution(), 4);
        for (at, want) in channel.values.iter().enumerate() {
            let (x, z) = ((at % 3) as i32, (at / 3) as i32);
            assert_eq!(
                migrated.regions.channel_at(0, x, z),
                *want,
                "channel value at cell ({x}, {z})",
            );
            // It sits on the height it was painted over.
            assert_eq!(migrated.regions.height_at(x, z), data.heights[at]);
        }
        // The ground the embedding added carries no paint.
        assert_eq!(migrated.regions.channel_at(0, 3, 3), 0);
    }

    /// A 129-vertex grid over 128-cell regions is embedded, not preserved:
    /// it lands in the 2x2 block of regions holding it, every authored
    /// height on the cell it describes, and the remainder of those regions
    /// reads as zero. Nothing is resampled and nothing is lost.
    #[test]
    fn a_129_vertex_grid_is_embedded_in_the_regions_that_hold_it() {
        let data = TerrainData {
            resolution: 129,
            heights: (0..129 * 129).map(|i| i as f32 * 0.5).collect(),
            channels: vec![],
        };
        let mut migrated = RegionTerrainData::from_legacy_v1(&data).expect("migrates");
        assert_eq!(migrated.regions.region_size().get(), 128);
        // Two regions per axis hold a 129-vertex grid, so the extent is
        // 256: wider than the grid, by whole regions.
        assert_eq!(migrated.grid_resolution(), 256);
        assert_embedded(&migrated.regions, &data.heights, 129, 256);

        // A version-1 file states no geometry; the caller resolves it
        // before saving.
        assert_eq!(migrated.grid, None);
        migrated.grid = Some(GridGeometry::DEFAULT);

        let reloaded = load(&save(&migrated).expect("encodes")).expect("decodes");
        assert_eq!(reloaded, migrated);
        assert_embedded(&reloaded.regions, &data.heights, 129, 256);
    }

    #[test]
    fn v1_to_v2_migration_round_trips_through_load_and_save() {
        let original = sample();
        let v1_bytes = encode(&original).expect("v1 encodes");

        let mut migrated = load(&v1_bytes).expect("loads v1");
        assert_eq!(
            migrated,
            RegionTerrainData::from_legacy_v1(&original).unwrap()
        );
        assert_eq!(migrated.as_legacy(), Some(original.clone()));
        migrated.grid = Some(GridGeometry::DEFAULT);

        let v2_bytes = save(&migrated).expect("encodes");
        assert_eq!(u16::from_le_bytes([v2_bytes[8], v2_bytes[9]]), VERSION_7);
        assert_ne!(v2_bytes[8..10], v1_bytes[8..10]);

        let reloaded = load(&v2_bytes).expect("loads");
        assert_eq!(reloaded, migrated);
    }

    #[test]
    fn as_legacy_refuses_once_control_has_been_painted() {
        let mut migrated = RegionTerrainData::from_legacy_v1(&sample()).unwrap();
        migrated
            .regions
            .set_control(0, 0, Control::default().with_base_id(1));
        assert_eq!(migrated.as_legacy(), None);
    }

    #[test]
    fn as_legacy_refuses_once_color_has_been_painted() {
        let mut migrated = RegionTerrainData::from_legacy_v1(&sample()).unwrap();
        migrated.regions.set_color(0, 0, [1, 2, 3, 4]);
        assert_eq!(migrated.as_legacy(), None);
    }

    /// Sculpting far out from the origin widens the terrain rather than
    /// putting ground beyond its reach.
    #[test]
    fn sculpting_far_out_widens_the_terrain_rather_than_escaping_it() {
        let mut migrated = RegionTerrainData::from_legacy_v1(&sample()).unwrap();
        assert_eq!(migrated.grid_resolution(), 4);
        migrated.regions.set_height(100, 100, 1.0);
        assert_eq!(migrated.grid_resolution(), 104);
        assert_eq!(migrated.regions.height_at(100, 100), 1.0);
    }

    /// A region far from the origin is not out of bounds: the terrain
    /// reaches that far, and the legacy view spans out to meet it.
    #[test]
    fn a_region_far_from_the_origin_widens_the_terrain_to_reach_it() {
        let mut regions = TerrainRegions::new(RegionSize::new(4).unwrap());
        regions.set_height(100, 100, 1.0);
        let data = RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        // Cell 100 sits in region 25, so the terrain runs out to the far
        // edge of that region: 26 regions of 4 cells.
        let legacy = data.as_legacy().expect("nothing is out of reach now");
        assert_eq!(legacy.resolution, 104);
        assert_eq!(data.regions.height_at(100, 100), 1.0);
    }

    /// A grid wider than the regions holding it is not a refusal: the
    /// vertices past them are unauthored ground, and the legacy view
    /// carries them as the default.
    #[test]
    fn as_legacy_reads_absent_regions_of_the_grid_as_default() {
        let mut regions = TerrainRegions::new(RegionSize::new(4).unwrap());
        regions.set_height(0, 0, 1.0);
        let data = RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        // The grid is the one allocated region, four cells a side: extent
        // is what the regions reach.
        let legacy = data.as_legacy().expect("the grid is representable");
        assert_eq!(legacy.resolution, 4);
        assert_eq!(legacy.heights[0], 1.0);
        assert!(legacy.heights[1..].iter().all(|h| *h == 0.0));
    }

    /// A terrain nobody has sculpted holds no regions, and therefore no
    /// ground.
    #[test]
    fn a_never_edited_terrain_reads_as_having_no_ground() {
        let data = RegionTerrainData {
            channels: vec![],
            regions: TerrainRegions::new(RegionSize::new(256).unwrap()),
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        assert_eq!(
            data.as_legacy(),
            Some(TerrainData {
                resolution: 0,
                heights: vec![],
                channels: vec![],
            })
        );
    }

    #[test]
    fn rejects_a_v2_file_written_by_a_newer_build() {
        let bytes = encode_regions(&sample_regions()).expect("encodes");
        let mut newer = bytes.clone();
        newer[8..10].copy_from_slice(&(VERSION_7 + 1).to_le_bytes());
        assert_eq!(
            decode_regions(&newer),
            Err(SidecarError::UnsupportedVersion(VERSION_7 + 1))
        );
        assert_eq!(
            load(&newer),
            Err(SidecarError::UnsupportedVersion(VERSION_7 + 1))
        );
    }

    #[test]
    fn load_never_misparses_one_version_as_the_other() {
        let v1 = encode(&sample()).expect("encodes");
        let regions_bytes = encode_regions(&sample_regions()).expect("encodes");
        assert_eq!(
            decode_regions(&v1),
            Err(SidecarError::UnsupportedVersion(VERSION))
        );
        assert_eq!(
            decode(&regions_bytes),
            Err(SidecarError::UnsupportedVersion(VERSION_7))
        );
    }

    #[test]
    fn v2_rejects_version_zero() {
        let mut bytes = encode_regions(&sample_regions()).expect("encodes");
        bytes[8..10].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(
            decode_regions(&bytes),
            Err(SidecarError::UnsupportedVersion(0))
        );
        assert_eq!(load(&bytes), Err(SidecarError::UnsupportedVersion(0)));
    }

    #[test]
    fn v2_rejects_a_set_header_reserved_flag() {
        let mut bytes = encode_regions(&sample_regions()).expect("encodes");
        bytes[10..12].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(decode_regions(&bytes), Err(SidecarError::ReservedFieldSet));
    }

    #[test]
    fn v2_rejects_a_region_size_of_zero() {
        let data = RegionTerrainData {
            channels: vec![],
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let mut bytes = encode_regions(&data).expect("encodes");
        // header(12) + channel_count(4) = offset 16
        // is region_size (no channels in this fixture).
        bytes[16..20].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(
            decode_regions(&bytes),
            Err(SidecarError::InvalidRegionSize(RegionSizeError::Zero))
        );
    }

    #[test]
    fn v2_rejects_a_non_power_of_two_region_size() {
        let data = RegionTerrainData {
            channels: vec![],
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let mut bytes = encode_regions(&data).expect("encodes");
        bytes[16..20].copy_from_slice(&300u32.to_le_bytes());
        assert_eq!(
            decode_regions(&bytes),
            Err(SidecarError::InvalidRegionSize(
                RegionSizeError::NotPowerOfTwo
            ))
        );
    }

    #[test]
    fn v2_rejects_reserved_bits_set_in_a_control_word() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_control(0, 0, Control::default().with_base_id(1));
        let data = RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let bytes = encode_regions(&data).expect("encodes");

        // Locate the control block: header(12) + channel_resolution(4) +
        // channel_count(4) + region_size(4) + region_count(4) +
        // material_count(4) + the autoterrain and grid blocks + coord(8) +
        // flags+pad(4) + heights(4*4) is where the control words start for
        // this 2x2 region.
        let control_offset = 56
            + AUTOTERRAIN_BLOCK_LEN
            + GRID_BLOCK_LEN
            + SURFACE_BLOCK_LEN
            + EMPTY_SCATTER_BLOCK_LEN;
        let word = u32::from_le_bytes([
            bytes[control_offset],
            bytes[control_offset + 1],
            bytes[control_offset + 2],
            bytes[control_offset + 3],
        ]);
        let poison = |bit: u32| {
            let mut poisoned = bytes.clone();
            poisoned[control_offset..control_offset + 4]
                .copy_from_slice(&(word | bit).to_le_bytes());
            decode_regions(&poisoned)
        };

        assert_eq!(poison(1 << 19), Err(SidecarError::ReservedFieldSet));
        assert_eq!(poison(1 << 31), Err(SidecarError::ReservedFieldSet));
        // The manual bit sits directly below them and is part of the
        // format, so it decodes.
        assert!(
            poison(crate::control::MANUAL_BIT)
                .expect("a claimed cell decodes")
                .regions
                .grid_control(0, 0)
                .manual()
        );
    }

    #[test]
    fn v2_rejects_reserved_bits_set_in_region_flags() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, 1.0);
        let data = RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let bytes = encode_regions(&data).expect("encodes");
        // header(12) + channel_resolution(4) + channel_count(4) +
        // region_size(4) + region_count(4) + material_count(4) +
        // the autoterrain block + coord(8) is the region flags byte.
        let flags_offset = 36
            + AUTOTERRAIN_BLOCK_LEN
            + GRID_BLOCK_LEN
            + SURFACE_BLOCK_LEN
            + EMPTY_SCATTER_BLOCK_LEN;
        let mut poisoned = bytes.clone();
        poisoned[flags_offset] |= 0b1000_0000;
        assert_eq!(
            decode_regions(&poisoned),
            Err(SidecarError::ReservedFieldSet)
        );
    }

    /// A clear presence bit declares a state this build does not
    /// understand, so it is rejected like any other unknown reserved bit.
    #[test]
    fn v2_rejects_a_region_whose_presence_bit_is_clear() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, 1.0);
        let data = RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let bytes = encode_regions(&data).expect("encodes");
        let flags_offset = 36
            + AUTOTERRAIN_BLOCK_LEN
            + GRID_BLOCK_LEN
            + SURFACE_BLOCK_LEN
            + EMPTY_SCATTER_BLOCK_LEN;
        let mut poisoned = bytes.clone();
        assert_eq!(poisoned[flags_offset], REGION_FLAG_PRESENT);
        poisoned[flags_offset] = 0;
        assert_eq!(
            decode_regions(&poisoned),
            Err(SidecarError::ReservedFieldSet)
        );
    }

    #[test]
    fn v2_rejects_nonzero_region_padding() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, 1.0);
        let data = RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let bytes = encode_regions(&data).expect("encodes");
        // one byte after the flags byte
        let pad_offset = 37
            + AUTOTERRAIN_BLOCK_LEN
            + GRID_BLOCK_LEN
            + SURFACE_BLOCK_LEN
            + EMPTY_SCATTER_BLOCK_LEN;
        let mut poisoned = bytes.clone();
        poisoned[pad_offset] = 1;
        assert_eq!(
            decode_regions(&poisoned),
            Err(SidecarError::ReservedFieldSet)
        );
    }

    #[test]
    fn encode_regions_refuses_a_material_name_that_is_not_a_file_stem() {
        let data = RegionTerrainData {
            channels: vec![],
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            materials: vec![TerrainMaterialSlot::new("../escape")],
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        assert!(matches!(
            encode_regions(&data),
            Err(SidecarError::InvalidMaterialName(_))
        ));
        assert!(matches!(
            save(&data),
            Err(SidecarError::InvalidMaterialName(_))
        ));
    }

    #[test]
    fn decode_regions_also_refuses_a_hand_crafted_bad_material_name() {
        let bad_name = b"../escape";
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION_2.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // channel_resolution
        bytes.extend_from_slice(&0u32.to_le_bytes()); // channel_count
        bytes.extend_from_slice(&4u32.to_le_bytes()); // region_size
        bytes.extend_from_slice(&0u32.to_le_bytes()); // region_count
        bytes.extend_from_slice(&1u32.to_le_bytes()); // material_count
        bytes.extend_from_slice(&(bad_name.len() as u32).to_le_bytes());
        bytes.extend_from_slice(bad_name);
        bytes.extend_from_slice(&0.1f32.to_le_bytes());
        assert!(matches!(
            decode_regions(&bytes),
            Err(SidecarError::InvalidMaterialName(_))
        ));
    }

    #[test]
    fn material_names_accept_a_file_stem_and_reject_anything_that_is_not_one() {
        for good in ["grass", "rock_05", "moss-2", "a.b"] {
            assert!(
                validate_material_name(good).is_ok(),
                "{good:?} must validate"
            );
        }
        for bad in [".", "..", "../x", "a/b", r"a\b", "C:x", "my material"] {
            assert!(
                validate_material_name(bad).is_err(),
                "{bad:?} must not validate"
            );
        }
    }

    /// A removed material leaves its id behind, so the empty name survives
    /// the round trip like any other slot.
    #[test]
    fn a_tombstone_round_trips_and_keeps_the_ids_above_it_where_they_were() {
        let data = RegionTerrainData {
            channels: vec![],
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            materials: vec![
                TerrainMaterialSlot::new("grass"),
                TerrainMaterialSlot::tombstone(),
                TerrainMaterialSlot::new("sand"),
            ],
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let back = decode_regions(&encode_regions(&data).expect("encodes")).expect("decodes");
        assert_eq!(back, data);
        assert!(back.materials[1].is_tombstone());
        assert_eq!(
            back.materials[2].material, "sand",
            "texture id 2 must still be the same material it was",
        );
    }

    /// An earlier version-2 layout carries a texture-set path where the
    /// material count sits. Such a file is refused rather than decoded as
    /// a material list.
    #[test]
    fn a_file_from_the_previous_layout_is_refused_rather_than_misread() {
        let stale_path = b"moor.textureset.bsn";
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION_2.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // channel_resolution
        bytes.extend_from_slice(&0u32.to_le_bytes()); // channel_count
        bytes.extend_from_slice(&4u32.to_le_bytes()); // region_size
        bytes.extend_from_slice(&0u32.to_le_bytes()); // region_count
        // Where material_count sits, that layout writes the length of its
        // texture-set path, then the path itself.
        bytes.extend_from_slice(&(stale_path.len() as u32).to_le_bytes());
        bytes.extend_from_slice(stale_path);

        assert!(
            decode_regions(&bytes).is_err(),
            "a stale texture-set path must not decode as a material list",
        );
        assert!(load(&bytes).is_err());

        // A short path has a length small enough to pass as a slot count,
        // so the refusal comes from the bytes not fitting.
        let mut short = bytes[..28].to_vec();
        short.extend_from_slice(&3u32.to_le_bytes());
        short.extend_from_slice(b"abc");
        assert!(decode_regions(&short).is_err());
    }

    /// A terrain with no texture set writes a zero length there, which
    /// reads as an empty material list.
    #[test]
    fn a_previous_layout_file_with_no_texture_set_reads_as_no_materials() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION_2.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // channel_resolution
        bytes.extend_from_slice(&0u32.to_le_bytes()); // channel_count
        bytes.extend_from_slice(&4u32.to_le_bytes()); // region_size
        bytes.extend_from_slice(&0u32.to_le_bytes()); // region_count
        bytes.extend_from_slice(&0u32.to_le_bytes()); // was texture_set_len

        let decoded = decode_regions(&bytes).expect("a set-less file still decodes");
        assert!(decoded.materials.is_empty());
    }

    /// A version-2 sidecar has no detiling field, and loads as off rather
    /// than reading the next slot's bytes as a strength.
    #[test]
    fn a_version_2_file_loads_with_detiling_off() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION_2.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // channel_resolution
        bytes.extend_from_slice(&0u32.to_le_bytes()); // channel_count
        bytes.extend_from_slice(&4u32.to_le_bytes()); // region_size
        bytes.extend_from_slice(&0u32.to_le_bytes()); // region_count
        bytes.extend_from_slice(&2u32.to_le_bytes()); // material_count
        for (name, uv_scale) in [("grass", 0.25f32), ("rock", 0.5)] {
            bytes.extend_from_slice(&(name.len() as u32).to_le_bytes());
            bytes.extend_from_slice(name.as_bytes());
            bytes.extend_from_slice(&uv_scale.to_le_bytes());
        }

        let decoded = decode_regions(&bytes).expect("a version-2 file still decodes");
        assert_eq!(decoded.materials.len(), 2);
        assert_eq!(decoded.materials[0].material, "grass");
        assert_eq!(decoded.materials[0].uv_scale, 0.25);
        assert_eq!(decoded.materials[0].detile, 0.0);
        assert_eq!(decoded.materials[1].material, "rock");
        assert_eq!(decoded.materials[1].uv_scale, 0.5);
        assert_eq!(decoded.materials[1].detile, 0.0);

        // Saving it forward writes the current version without changing
        // what any slot draws.
        let forward = load(&save(&decoded).expect("encodes")).expect("reloads");
        assert_eq!(forward.materials, decoded.materials);
    }

    /// A version-3 file has no autoterrain block and loads with
    /// autoterrain off. Saving it forward writes the block at its
    /// defaults.
    #[test]
    fn a_version_3_file_loads_with_autoterrain_off() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION_3.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // channel_resolution
        bytes.extend_from_slice(&0u32.to_le_bytes()); // channel_count
        bytes.extend_from_slice(&4u32.to_le_bytes()); // region_size
        bytes.extend_from_slice(&0u32.to_le_bytes()); // region_count
        bytes.extend_from_slice(&1u32.to_le_bytes()); // material_count
        bytes.extend_from_slice(&5u32.to_le_bytes());
        bytes.extend_from_slice(b"grass");
        bytes.extend_from_slice(&0.25f32.to_le_bytes()); // uv_scale
        bytes.extend_from_slice(&0.5f32.to_le_bytes()); // detile

        let decoded = decode_regions(&bytes).expect("a version-3 file still decodes");
        assert_eq!(decoded.autoterrain, AutoTerrainSettings::default());
        assert!(!decoded.autoterrain.enabled);
        assert_eq!(decoded.materials[0].detile, 0.5);

        let forward = save(&decoded).expect("encodes");
        assert_eq!(u16::from_le_bytes([forward[8], forward[9]]), VERSION_7);
        assert_eq!(
            load(&forward).expect("reloads").autoterrain,
            decoded.autoterrain
        );
    }

    /// A document with no channels and no materials puts the grid block
    /// at a fixed offset: the shared header is 12 bytes, then
    /// `channel_resolution`, `channel_count`, `region_size`,
    /// `region_count` and `material_count` are 4 each, then the
    /// autoterrain block.
    const EMPTY_DOC_GRID_OFFSET: usize = 28 + AUTOTERRAIN_BLOCK_LEN;

    /// Assert that a dense `resolution`-per-edge grid is embedded rather
    /// than resampled: every authored value reads at the cell it
    /// describes, and the ground around it reads as zero.
    fn assert_embedded(regions: &TerrainRegions, source: &[f32], resolution: u32, extent: u32) {
        assert!(
            extent >= resolution,
            "an embedding never loses ground: {extent} < {resolution}",
        );
        for z in 0..extent {
            for x in 0..extent {
                let got = regions.height_at(x as i32, z as i32);
                let want = if x < resolution && z < resolution {
                    source[(z as usize) * (resolution as usize) + x as usize]
                } else {
                    0.0
                };
                assert_eq!(got, want, "cell ({x}, {z}) of a {resolution}-vertex grid");
            }
        }
    }

    fn bare_document() -> RegionTerrainData {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, 3.0);
        RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        }
    }

    #[test]
    fn grid_geometry_round_trips_through_a_version_5_file() {
        let mut data = bare_document();
        data.grid = Some(GridGeometry {
            cell_size: 0.39215687,
            anchor: bevy_math::Vec2::new(-50.0, -50.0),
        });

        let bytes = encode_regions(&data).expect("encodes");
        assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), VERSION_7);
        assert_eq!(decode_regions(&bytes).expect("decodes"), data);
        assert_eq!(load(&bytes).expect("loads"), data);
    }

    /// The anchor keeps a migrated terrain's ground in place, so it
    /// survives the file as an exact float.
    #[test]
    fn a_negative_anchor_survives_the_file_exactly() {
        let mut data = bare_document();
        let anchor = bevy_math::Vec2::new(-512.0, -0.5);
        data.grid = Some(GridGeometry {
            cell_size: 1.0 / 3.0,
            anchor,
        });

        let reloaded = load(&encode_regions(&data).expect("encodes")).expect("loads");
        let grid = reloaded.grid.expect("version 5 states its geometry");
        assert_eq!(grid.anchor, anchor);
        assert_eq!(grid.cell_size, 1.0 / 3.0);
    }

    /// A version-4 file has no geometry to read and decodes as `None`
    /// rather than the default: the caller places its cells from the
    /// component's declared rectangle, and defaulting to 1.0 would
    /// rescale ground drawn at some other spacing.
    #[test]
    fn a_version_4_file_states_no_grid_geometry() {
        let data = bare_document();
        let current = encode_regions(&data).expect("encodes");

        // Rewrite it as version 4: no grid, surface or scatter block, no
        // per-region placement count, and a `channel_resolution` ahead of
        // the channel count.
        let mut v4 = current.clone();
        v4.truncate(v4.len() - 4);
        v4.splice(
            EMPTY_DOC_GRID_OFFSET
                ..EMPTY_DOC_GRID_OFFSET
                    + GRID_BLOCK_LEN
                    + SURFACE_BLOCK_LEN
                    + EMPTY_SCATTER_BLOCK_LEN,
            [],
        );
        v4.splice(12..12, 0u32.to_le_bytes());
        v4[8..10].copy_from_slice(&VERSION_4.to_le_bytes());

        let decoded = decode_regions(&v4).expect("a version-4 file still decodes");
        assert_eq!(decoded.grid, None);
        assert_eq!(decoded.regions, data.regions);
        assert_eq!(load(&v4).expect("loads").grid, None);
    }

    /// Bytes in the version-6 layout, kept as a file for the same reason
    /// the older fixtures are: a fixture spliced out of this build's
    /// writer would only agree with this build's model of the format.
    const VERSION_6_FILE: &[u8] = include_bytes!("../tests/fixtures/version_6.jdterrain");

    fn scattered_document() -> RegionTerrainData {
        let mut data = RegionTerrainData {
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            scatter: ScatterPalette {
                assets: vec![
                    ScatterPaletteEntry {
                        asset: "models/tree.gltf".to_string(),
                        obstacle: true,
                        cull_distance: 0.0,
                    },
                    ScatterPaletteEntry {
                        asset: "models/grass.glb".to_string(),
                        obstacle: false,
                        cull_distance: 60.0,
                    },
                ],
                groups: vec!["woods".to_string(), "meadow".to_string()],
            },
            ..RegionTerrainData::default()
        };
        data.regions.set_height(0, 0, 1.0);
        data.regions.set_height(5, 5, 2.0);
        data.add_placement(bevy_math::Vec3::new(1.5, 0.25, 2.5), 0, 0, 0.75, 1.1)
            .expect("the region is already allocated");
        data.add_placement(bevy_math::Vec3::new(6.0, 0.5, 7.0), 1, 1, -0.5, 0.9)
            .expect("the region is already allocated");
        data
    }

    #[test]
    fn a_scatter_palette_and_its_placements_round_trip() {
        let data = scattered_document();
        let bytes = save(&data).expect("encodes");
        assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), VERSION_7);
        assert_eq!(bytes.len(), data.encoded_len().expect("fits"));
        assert_eq!(load(&bytes).expect("loads"), data);
    }

    #[test]
    fn a_version_6_file_loads_with_no_scatter() {
        let data = load(VERSION_6_FILE).expect("a version-6 file loads");
        assert!(data.scatter.is_empty());
        assert_eq!(data.placement_count(), 0);
        assert_eq!(data.regions.height_at(1, 1), 4.0);

        // Saving it forward writes the empty palette and one zero
        // placement count per region, and reads back the same document.
        let forward = save(&data).expect("encodes");
        assert_eq!(u16::from_le_bytes([forward[8], forward[9]]), VERSION_7);
        assert_eq!(load(&forward).expect("loads"), data);
    }

    #[test]
    fn a_placement_lands_in_the_region_covering_it_and_reads_back_where_it_was_put() {
        let data = scattered_document();
        let placed: Vec<_> = data
            .placements()
            .map(|(coord, _, placement)| (coord, data.placement_position(coord, placement)))
            .collect();
        assert_eq!(
            placed,
            vec![
                (RegionCoord::new(0, 0), bevy_math::Vec3::new(1.5, 0.25, 2.5)),
                (RegionCoord::new(1, 1), bevy_math::Vec3::new(6.0, 0.5, 7.0)),
            ]
        );
    }

    #[test]
    fn a_placement_on_a_region_edge_stays_in_the_region_already_there() {
        let mut data = RegionTerrainData {
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            scatter: ScatterPalette {
                assets: vec![ScatterPaletteEntry::new("a.glb")],
                groups: vec!["g".to_string()],
            },
            ..RegionTerrainData::default()
        };
        data.regions.set_height(0, 0, 1.0);
        let before = data.regions.region_count();

        // The far edge of the only region: the floor puts it in the next
        // one, which does not exist and must not be allocated for it.
        let (coord, _) = data
            .add_placement(bevy_math::Vec3::new(4.0, 0.0, 4.0), 0, 0, 0.0, 1.0)
            .expect("the edge falls back into the region holding it");
        assert_eq!(coord, RegionCoord::ORIGIN);
        assert_eq!(data.regions.region_count(), before);
        assert_eq!(
            data.placement_position(coord, data.placement(coord, 0).expect("stored")),
            bevy_math::Vec3::new(4.0, 0.0, 4.0)
        );
    }

    #[test]
    fn removing_a_group_drops_its_placements_and_leaves_the_other_indices_meaning_what_they_did() {
        let mut data = scattered_document();
        assert_eq!(data.group_counts(), vec![1, 1]);
        assert_eq!(data.remove_group(0), 1);
        assert_eq!(data.placement_count(), 1);
        assert_eq!(data.scatter.group(0), None);
        assert_eq!(data.scatter.group(1), Some("meadow"));
        assert_eq!(
            data.group_placements(1).count(),
            1,
            "the surviving group keeps its index"
        );
    }

    #[test]
    fn a_re_run_of_a_group_writes_its_placements_over_the_row_it_already_had() {
        let mut data = scattered_document();
        let group = data.scatter.intern_group("woods").expect("already there");
        assert_eq!(data.clear_group(group), 1);
        assert_eq!(data.scatter.intern_group("woods"), Ok(group));
        assert_eq!(data.scatter.groups.len(), 2, "no row per run");
    }

    #[test]
    fn a_tombstoned_row_is_dropped_on_the_way_out_and_its_placements_renumbered() {
        let mut data = scattered_document();
        assert_eq!(data.remove_group(0), 1);
        let bytes = save(&data).expect("encodes");
        assert_eq!(bytes.len(), data.encoded_len().expect("fits"));

        let read = load(&bytes).expect("loads");
        assert_eq!(read.scatter.groups, vec!["meadow".to_string()]);
        assert_eq!(read.placement_count(), 1);
        let (_, _, placement) = read.placements().next().expect("one placement");
        assert_eq!(read.scatter.group(placement.group), Some("meadow"));
        assert_eq!(
            read.scatter
                .asset(placement.asset)
                .map(|e| e.asset.as_str()),
            Some("models/grass.glb"),
            "the palette index is remapped with the table"
        );
    }

    #[test]
    fn encode_refuses_a_palette_entry_the_asset_system_could_not_load() {
        let mut data = scattered_document();
        data.scatter.assets[0].asset = "../outside.gltf".to_string();
        assert_eq!(
            encode_regions(&data),
            Err(SidecarError::InvalidScatterAsset(
                ScatterAssetError::NotAssetsRelative
            ))
        );
    }

    #[test]
    fn encode_refuses_a_group_key_no_field_could_show() {
        let mut data = scattered_document();
        data.scatter.groups[0] = "bad\nkey".to_string();
        assert_eq!(
            encode_regions(&data),
            Err(SidecarError::InvalidScatterGroup(
                ScatterGroupError::InvalidCharacter
            ))
        );
    }

    #[test]
    fn a_placement_naming_an_index_the_tables_do_not_reach_is_rejected() {
        let mut data = scattered_document();
        data.regions
            .region_mut(RegionCoord::ORIGIN)
            .expect("allocated")
            .placements_mut()[0]
            .asset = 9;
        assert_eq!(
            encode_regions(&data),
            Err(SidecarError::UnknownScatterIndex)
        );

        // The last placement record is the tail of the file, so poisoning
        // its group index leaves the decoder, not the model, to refuse it.
        let bytes = encode_regions(&scattered_document()).expect("encodes");
        let at = bytes.len() - PLACEMENT_LEN;
        let mut poisoned = bytes.clone();
        poisoned[at..at + 2].copy_from_slice(&9u16.to_le_bytes());
        assert_eq!(
            decode_regions(&poisoned),
            Err(SidecarError::UnknownScatterIndex)
        );
    }

    /// Bytes written by a build that spoke version 4, kept as a file: a
    /// fixture spliced out of this build's own writer would only agree
    /// with this build's model of the format.
    const VERSION_4_FILE: &[u8] = include_bytes!("../tests/fixtures/version_4.jdterrain");

    /// Bytes written by a build that spoke version 5, kept as a file for
    /// the same reason the version-4 one is: a fixture spliced out of
    /// this build's writer would only agree with this build's model.
    const VERSION_5_FILE: &[u8] = include_bytes!("../tests/fixtures/version_5.jdterrain");

    /// A version-5 file states no surface block, so it loads at the
    /// settings it was rendered at and nothing else about it moves.
    #[test]
    fn a_version_5_file_loads_at_the_default_surface_settings() {
        let data = load(VERSION_5_FILE).expect("a version-5 file loads");

        assert_eq!(data.surface, SurfaceSettings::default());
        assert_eq!(data.surface.blend_sharpness, DEFAULT_BLEND_SHARPNESS);
        assert_eq!(data.surface.tint_strength, DEFAULT_TINT_STRENGTH);

        assert_eq!(data.regions.height_at(0, 0), 1.0);
        assert_eq!(data.regions.height_at(3, 3), 4.0);
        assert_eq!(data.regions.color_at(0, 0), [200, 180, 160, 255]);
        assert_eq!(data.regions.color_at(1, 0), crate::region::DEFAULT_COLOR);
        assert_eq!(data.materials[0].material, "grass");
        assert!(data.autoterrain.enabled);
        assert_eq!(
            data.grid,
            Some(GridGeometry {
                cell_size: 0.5,
                anchor: bevy_math::Vec2::new(-10.0, -10.0),
            })
        );
    }

    /// Loading a version-5 file and saving it back writes version 6 with
    /// the defaults spelled out, and reloading that changes nothing.
    #[test]
    fn a_version_5_file_saves_forward_as_version_6() {
        let data = load(VERSION_5_FILE).expect("loads");
        let forward = save(&data).expect("encodes");

        assert_eq!(u16::from_le_bytes([forward[8], forward[9]]), VERSION_7);
        assert_eq!(load(&forward).expect("reloads"), data);
    }

    #[test]
    fn surface_settings_round_trip_through_a_version_6_file() {
        let data = RegionTerrainData {
            surface: SurfaceSettings {
                blend_sharpness: 0.875,
                tint_strength: 0.25,
            },
            ..RegionTerrainData::default()
        };

        let bytes = save(&data).expect("encodes");
        assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), VERSION_7);
        let back = load(&bytes).expect("decodes");
        assert_eq!(back.surface, data.surface);
        assert_eq!(back, data);
    }

    /// Sanitized like the autoterrain block: a NaN sharpness reaches the
    /// shader's `pow` and a NaN strength its `mix`, and both paint the
    /// terrain black.
    #[test]
    fn a_nonfinite_or_out_of_range_surface_block_is_clamped_on_the_way_back() {
        let data = RegionTerrainData {
            surface: SurfaceSettings {
                blend_sharpness: f32::NAN,
                tint_strength: 4.0,
            },
            ..RegionTerrainData::default()
        };

        let back = load(&save(&data).expect("encodes")).expect("decodes");
        assert_eq!(back.surface.blend_sharpness, DEFAULT_BLEND_SHARPNESS);
        assert_eq!(back.surface.tint_strength, 1.0);
    }

    /// A version-4 file loads with everything it carries on the cell it
    /// was written on: heights, the dense channel grid that vintage
    /// stores ahead of the regions, the material list and the autoterrain
    /// block.
    #[test]
    fn a_real_version_4_file_loads_with_its_data_where_it_was_written() {
        let data = load(VERSION_4_FILE).expect("a version-4 file loads");

        assert_eq!(data.grid, None, "version 4 states no geometry");
        assert_eq!(data.regions.height_at(0, 0), 1.0);
        assert_eq!(data.regions.height_at(1, 0), 2.0);
        assert_eq!(data.regions.height_at(0, 1), 3.0);
        assert_eq!(data.regions.height_at(3, 3), 4.0);

        assert_eq!(data.channels.len(), 1);
        assert_eq!(data.channels[0].name, "biome");
        assert_eq!(data.regions.channel_at(0, 0, 0), 7);
        assert_eq!(data.regions.channel_at(0, 1, 0), 8);
        assert_eq!(data.regions.channel_at(0, 0, 1), 9);
        assert_eq!(data.regions.channel_at(0, 4, 4), 11);

        // Regions are allocated whole, so they reach further than the grid
        // the file declares, and the channel sits on the cells its heights
        // do rather than spreading over the wider extent.
        assert_eq!(data.regions.stored_extent(), Some((8, 8)));

        assert_eq!(data.materials.len(), 1);
        assert_eq!(data.materials[0].material, "grass");
        assert!(data.autoterrain.enabled);
        assert_eq!(data.autoterrain.slope_start_deg, 20.0);
        assert_eq!(data.autoterrain.slope_end_deg, 45.0);
    }

    /// The spacing a declared rectangle draws its cells at is
    /// `size / (resolution - 1)`: `resolution` counts vertices, so the
    /// last vertex sits on the far edge. The value is not rounded.
    #[test]
    fn a_declared_rect_becomes_the_spacing_and_corner_it_drew_with() {
        // What a scene eliding both fields refills from the component's
        // default contract.
        let elided = GridGeometry::for_declared_rect(bevy_math::Vec2::splat(100.0), 256);
        assert_eq!(elided.cell_size, 100.0 / 255.0);
        assert_eq!(elided.anchor, bevy_math::Vec2::splat(-50.0));

        // A 2^k+1 vertex grid lands on a whole number.
        let square = GridGeometry::for_declared_rect(bevy_math::Vec2::splat(128.0), 129);
        assert_eq!(square.cell_size, 1.0);
        assert_eq!(square.anchor, bevy_math::Vec2::splat(-64.0));

        // A 1024-vertex grid across 1024 metres does not.
        let thousand = GridGeometry::for_declared_rect(bevy_math::Vec2::splat(1024.0), 1024);
        assert_eq!(thousand.cell_size, 1024.0 / 1023.0);
        assert_ne!(thousand.cell_size, 1.0);
    }

    /// A rectangle whose axes ask for different spacings cannot be
    /// re-described by one square cell. X wins, Z is respaced, and both
    /// spacings are reported to the caller.
    #[test]
    fn a_non_square_rect_takes_the_spacing_its_x_axis_asked_for() {
        let size = bevy_math::Vec2::new(2000.0, 500.0);
        let grid = GridGeometry::for_declared_rect(size, 1024);

        assert_eq!(grid.cell_size, 2000.0 / 1023.0);
        assert_eq!(grid.anchor, bevy_math::Vec2::new(-1000.0, -250.0));
        assert_eq!(
            declared_rect_respacing(size, 1024),
            Some((2000.0 / 1023.0, 500.0 / 1023.0)),
        );
    }

    /// A square rectangle is a spacing both axes agree on, so there is
    /// nothing to report.
    #[test]
    fn a_square_rect_has_nothing_to_report() {
        assert_eq!(
            declared_rect_respacing(bevy_math::Vec2::splat(1024.0), 1024),
            None,
        );
    }

    /// When the sidecar lands and the scene text does not, the component
    /// still declares its previous rectangle. The cells are drawn at what
    /// the file beside them states, not at what the stale text computes.
    #[test]
    fn a_stated_geometry_beats_the_rectangle_stale_scene_text_declares() {
        let stated = GridGeometry {
            cell_size: 2.0,
            anchor: bevy_math::Vec2::ZERO,
        };
        let resolved = resolve_grid(Some(stated), bevy_math::Vec2::splat(100.0), 256);
        assert_eq!(resolved, stated);
    }

    #[test]
    fn a_file_too_old_to_state_geometry_falls_back_to_the_declared_rect() {
        let resolved = resolve_grid(None, bevy_math::Vec2::splat(100.0), 256);
        assert_eq!(
            resolved,
            GridGeometry::for_declared_rect(bevy_math::Vec2::splat(100.0), 256)
        );
        assert_eq!(resolved.cell_size, 100.0 / 255.0);
    }

    /// A degenerate resolution has no spacing, and must not divide by
    /// zero and hand every consumer a NaN grid.
    #[test]
    fn a_rect_too_small_to_have_a_spacing_still_yields_a_finite_one() {
        for resolution in [0, 1, 2] {
            let grid = GridGeometry::for_declared_rect(bevy_math::Vec2::splat(10.0), resolution);
            assert!(
                grid.cell_size.is_finite() && grid.cell_size > 0.0,
                "resolution {resolution} gave {}",
                grid.cell_size
            );
        }
    }

    #[test]
    fn autoterrain_settings_round_trip() {
        let data = RegionTerrainData {
            autoterrain: AutoTerrainSettings {
                enabled: true,
                base_slot: 2,
                slope_slot: 5,
                slope_start_deg: 12.5,
                slope_end_deg: 63.25,
            },
            ..RegionTerrainData::default()
        };
        let back = load(&save(&data).expect("encodes")).expect("decodes");
        assert_eq!(back.autoterrain, data.autoterrain);
        assert_eq!(back, data);
    }

    /// Sanitized like the slot floats: an unclamped NaN would reach the
    /// shader's `smoothstep`.
    #[test]
    fn corrupt_autoterrain_settings_decode_to_sanitized_values() {
        let good = encode_regions(&RegionTerrainData::default()).expect("encodes");
        // 8 magic, 2 version, 2 reserved, 4
        // channel_count, 4 region_size, 4 region_count, 4 material_count,
        // then the block: flags, base, slope, padding.
        let flags_at = 8 + 2 + 2 + 4 + 4 + 4 + 4;
        let start_at = flags_at + 4;
        let end_at = start_at + 4;
        assert_eq!(
            decode_regions(&good).expect("decodes").autoterrain,
            AutoTerrainSettings::default(),
            "offset math must match the encoder",
        );

        let decoded = |flags: u8, base: u8, slope: u8, start: f32, end: f32| {
            let mut bytes = good.clone();
            bytes[flags_at] = flags;
            bytes[flags_at + 1] = base;
            bytes[flags_at + 2] = slope;
            bytes[start_at..start_at + 4].copy_from_slice(&start.to_le_bytes());
            bytes[end_at..end_at + 4].copy_from_slice(&end.to_le_bytes());
            decode_regions(&bytes).expect("decodes").autoterrain
        };

        // A non-finite end is not a number to clamp, so it goes back to
        // the default rather than to the end of the range.
        let nonfinite = decoded(1, 0, 1, f32::NAN, f32::INFINITY);
        assert_eq!(nonfinite.slope_start_deg, DEFAULT_SLOPE_START_DEG);
        assert_eq!(nonfinite.slope_end_deg, DEFAULT_SLOPE_END_DEG);

        let out_of_range = decoded(1, 0, 1, -30.0, 400.0);
        assert_eq!(out_of_range.slope_start_deg, MIN_SLOPE_DEG);
        assert_eq!(out_of_range.slope_end_deg, MAX_SLOPE_DEG);

        // Ends the wrong way round are swapped, not collapsed, so the
        // width between them survives.
        let backwards = decoded(1, 0, 1, 70.0, 20.0);
        assert_eq!(backwards.slope_start_deg, 20.0);
        assert_eq!(backwards.slope_end_deg, 70.0);

        // Slot ids past the id space clamp into it, as a control word's
        // ids do.
        let wild_slots = decoded(1, 200, 99, 25.0, 40.0);
        assert_eq!(wild_slots.base_slot, crate::control::MAX_TEXTURE_ID);
        assert_eq!(wild_slots.slope_slot, crate::control::MAX_TEXTURE_ID);
    }

    /// A band of zero degrees makes flat ground evaluate
    /// `smoothstep(x, x, 0)`, so every unclaimed cell comes back NaN. A
    /// file holding one opens with a band.
    #[test]
    fn an_equal_band_off_disk_decodes_wide_enough_to_shade() {
        let good = encode_regions(&RegionTerrainData::default()).expect("encodes");
        let flags_at = 8 + 2 + 2 + 4 + 4 + 4 + 4;
        let start_at = flags_at + 4;
        let end_at = start_at + 4;

        let decoded = |start: f32, end: f32| {
            let mut bytes = good.clone();
            bytes[flags_at] = 1;
            bytes[start_at..start_at + 4].copy_from_slice(&start.to_le_bytes());
            bytes[end_at..end_at + 4].copy_from_slice(&end.to_le_bytes());
            decode_regions(&bytes).expect("decodes").autoterrain
        };

        let flat = decoded(0.0, 0.0);
        assert_eq!(flat.slope_start_deg, MIN_SLOPE_DEG);
        assert_eq!(flat.slope_end_deg, MIN_SLOPE_DEG + MIN_SLOPE_BAND_DEG);

        let middle = decoded(40.0, 40.0);
        assert_eq!(middle.slope_start_deg, 40.0);
        assert_eq!(middle.slope_end_deg, 40.0 + MIN_SLOPE_BAND_DEG);

        // Against the ceiling there is no room above, so the start comes
        // down instead.
        let vertical = decoded(90.0, 90.0);
        assert_eq!(vertical.slope_start_deg, MAX_SLOPE_DEG - MIN_SLOPE_BAND_DEG);
        assert_eq!(vertical.slope_end_deg, MAX_SLOPE_DEG);
    }

    #[test]
    fn v4_rejects_unknown_autoterrain_flag_bits_and_nonzero_padding() {
        let good = encode_regions(&RegionTerrainData::default()).expect("encodes");
        let flags_at = 8 + 2 + 2 + 4 + 4 + 4 + 4;

        let mut unknown_flag = good.clone();
        unknown_flag[flags_at] = 0b0000_0010;
        assert_eq!(
            decode_regions(&unknown_flag),
            Err(SidecarError::ReservedFieldSet)
        );

        let mut padded = good.clone();
        padded[flags_at + 3] = 1;
        assert_eq!(decode_regions(&padded), Err(SidecarError::ReservedFieldSet));
    }

    /// Detiling is a persisted per-slot value and survives the trip at
    /// full precision, like the tiling beside it.
    #[test]
    fn detiling_round_trips_per_slot() {
        let data = RegionTerrainData {
            materials: vec![
                TerrainMaterialSlot {
                    material: "grass".to_string(),
                    uv_scale: 0.1,
                    detile: 0.0,
                },
                TerrainMaterialSlot {
                    material: "rock".to_string(),
                    uv_scale: 0.1,
                    detile: 0.375,
                },
                TerrainMaterialSlot::tombstone(),
            ],
            ..RegionTerrainData::default()
        };
        let back = load(&save(&data).expect("encodes")).expect("decodes");
        assert_eq!(back.materials[0].detile, 0.0);
        assert_eq!(back.materials[1].detile, 0.375);
        assert_eq!(back.materials[2].detile, 0.0, "a tombstone draws nothing");
        assert_eq!(back, data);
    }

    /// A sidecar with corrupt float bytes decodes into a slot the shader
    /// can sample: NaN never reaches a UV.
    #[test]
    fn corrupt_slot_floats_decode_to_sanitized_values() {
        let slot = |uv_scale, detile| TerrainMaterialSlot {
            material: "g".to_string(),
            uv_scale,
            detile,
        };
        let data = RegionTerrainData {
            materials: vec![slot(0.25, 0.5), TerrainMaterialSlot::tombstone()],
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
            ..RegionTerrainData::default()
        };

        // Offsets of the named slot's two floats: 8 magic, 2 version,
        // 2 reserved, 4 channel_count, 4 region_size, 4 region_count, 4
        // material_count, 4 name_len, 1 name byte.
        let uv_scale_at = 8 + 2 + 2 + 4 + 4 + 4 + 4 + 4 + 1;
        let detile_at = uv_scale_at + 4;
        let good = encode_regions(&data).expect("encodes");
        assert_eq!(
            decode_regions(&good).expect("decodes").materials[0],
            slot(0.25, 0.5),
            "offset math must match the encoder",
        );

        for (uv_scale, detile, want) in [
            (f32::NAN, f32::NAN, slot(DEFAULT_UV_SCALE, DEFAULT_DETILE)),
            (
                f32::INFINITY,
                f32::NEG_INFINITY,
                slot(DEFAULT_UV_SCALE, DEFAULT_DETILE),
            ),
            (0.0, -3.0, slot(MIN_UV_SCALE, 0.0)),
            (1e9, 7.5, slot(MAX_UV_SCALE, MAX_DETILE)),
        ] {
            let mut bytes = good.clone();
            bytes[uv_scale_at..uv_scale_at + 4].copy_from_slice(&uv_scale.to_le_bytes());
            bytes[detile_at..detile_at + 4].copy_from_slice(&detile.to_le_bytes());
            let decoded = decode_regions(&bytes).expect("decodes");
            assert_eq!(decoded.materials[0], want, "from {uv_scale} / {detile}");
            assert_eq!(
                decoded.materials[1],
                TerrainMaterialSlot::tombstone(),
                "a tombstone's zeroes are not out of range, they are its shape",
            );
        }
    }

    #[test]
    fn asset_refs_accept_a_plain_relative_path_and_reject_traversal() {
        assert!(validate_asset_ref("textures/grass.png").is_ok());
        for bad in ["", "../x", "/abs/x", "a/../b", r"a\b", "C:/x"] {
            assert!(
                validate_asset_ref(bad).is_err(),
                "{bad:?} must not validate"
            );
        }
    }

    /// Truncates one byte short of each field's end. The fixture's layout
    /// is asserted against the encoder's output length first, so a wrong
    /// hand computation fails.
    #[test]
    fn every_structural_boundary_is_rejected_one_byte_short() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, 1.0);
        regions.set_color(0, 0, [9, 9, 9, 9]);
        let mut data = RegionTerrainData {
            channels: vec![],
            regions,
            // A named slot and a vacated one, so the zero-length name a
            // tombstone writes is one of the boundaries walked.
            materials: vec![
                TerrainMaterialSlot::new("t"),
                TerrainMaterialSlot::tombstone(),
            ],
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            // One palette entry, one group key and one placement below,
            // so the version-7 boundaries are walked too.
            scatter: ScatterPalette {
                assets: vec![ScatterPaletteEntry::new("a.glb")],
                groups: vec!["g".to_string()],
            },
        };
        data.regions
            .ensure_region(RegionCoord::ORIGIN)
            .placements_mut()
            .push(ScatterPlacement {
                group: 0,
                asset: 0,
                x: 0.5,
                y: 1.0,
                z: 0.25,
                yaw: 0.0,
                scale: 1.0,
            });
        let bytes = encode_regions(&data).expect("encodes");

        let magic_end = 8;
        let version_end = magic_end + 2;
        let flags_end = version_end + 2;
        let chan_count_end = flags_end + 4;
        let region_size_end = chan_count_end + 4;
        let region_count_end = region_size_end + 4;
        let material_count_end = region_count_end + 4;
        let name_len_end = material_count_end + 4;
        let name_end = name_len_end + 1; // "t"
        let uv_scale_end = name_end + 4;
        let detile_end = uv_scale_end + 4;
        // The tombstone writes a zero length and no name bytes at all.
        let tomb_name_len_end = detile_end + 4;
        let tomb_uv_scale_end = tomb_name_len_end + 4;
        let tomb_detile_end = tomb_uv_scale_end + 4;
        let auto_flags_end = tomb_detile_end + 1;
        let auto_base_end = auto_flags_end + 1;
        let auto_slope_end = auto_base_end + 1;
        let auto_pad_end = auto_slope_end + 1;
        let auto_start_end = auto_pad_end + 4;
        let auto_end_end = auto_start_end + 4;
        let grid_cell_size_end = auto_end_end + 4;
        let grid_anchor_x_end = grid_cell_size_end + 4;
        let grid_anchor_z_end = grid_anchor_x_end + 4;
        let surface_sharpness_end = grid_anchor_z_end + 4;
        let surface_tint_end = surface_sharpness_end + 4;
        let asset_count_end = surface_tint_end + 4;
        let asset_len_end = asset_count_end + 4;
        let asset_name_end = asset_len_end + 5; // "a.glb"
        let asset_flags_end = asset_name_end + 1;
        let asset_pad_end = asset_flags_end + 3;
        let asset_cull_end = asset_pad_end + 4;
        let group_count_end = asset_cull_end + 4;
        let group_len_end = group_count_end + 4;
        let group_name_end = group_len_end + 1; // "g"
        let coord_end = group_name_end + 8;
        let region_flags_end = coord_end + 1;
        let pad_end = region_flags_end + 3;
        let heights_end = pad_end + 4 * 4; // 4 cells * 4 bytes per f32
        let control_end = heights_end + 4 * 4; // 4 cells * 4 bytes per u32
        let color_end = control_end + 4 * 4; // 4 cells * 4 bytes per rgba8
        let placement_count_end = color_end + 4;
        let placement_end = placement_count_end + PLACEMENT_LEN;

        assert_eq!(
            placement_end,
            bytes.len(),
            "fixture layout math must match the real encoder"
        );

        for end in [
            magic_end,
            version_end,
            flags_end,
            chan_count_end,
            region_size_end,
            region_count_end,
            material_count_end,
            name_len_end,
            name_end,
            uv_scale_end,
            detile_end,
            tomb_name_len_end,
            tomb_uv_scale_end,
            tomb_detile_end,
            auto_flags_end,
            auto_base_end,
            auto_slope_end,
            auto_pad_end,
            auto_start_end,
            auto_end_end,
            grid_cell_size_end,
            grid_anchor_x_end,
            grid_anchor_z_end,
            surface_sharpness_end,
            surface_tint_end,
            asset_count_end,
            asset_len_end,
            asset_name_end,
            asset_flags_end,
            asset_pad_end,
            asset_cull_end,
            group_count_end,
            group_len_end,
            group_name_end,
            coord_end,
            region_flags_end,
            pad_end,
            heights_end,
            control_end,
            color_end,
            placement_count_end,
            placement_end,
        ] {
            assert_eq!(
                decode_regions(&bytes[..end - 1]),
                Err(SidecarError::Truncated),
                "truncating 1 byte before offset {end} must be rejected",
            );
        }
    }

    #[test]
    fn v2_an_oversized_region_size_claim_over_a_tiny_file_is_rejected() {
        let data = RegionTerrainData {
            channels: vec![],
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let mut bytes = encode_regions(&data).expect("encodes");
        // Claims an allocation the file cannot back; distinct from the
        // TooLarge case below, which overflows arithmetic before any read.
        bytes[16..20].copy_from_slice(&(1u32 << 20).to_le_bytes()); // region_size
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes()); // region_count
        assert_eq!(decode_regions(&bytes), Err(SidecarError::Truncated));
    }

    /// A region size whose cell-count-in-bytes overflows `usize` reports
    /// `TooLarge`, not `Truncated`: the failure is arithmetic, before any
    /// byte is read.
    #[test]
    fn v2_a_genuinely_overflowing_size_claim_is_too_large_not_truncated() {
        // 2^31 is a valid power-of-two region size; its cell count squared
        // times the 4 bytes per f32 overflows u64 (2^62 * 4 == 2^64).
        let region_size: u32 = 1 << 31;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION_2.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes()); // channel_resolution
        bytes.extend_from_slice(&0u32.to_le_bytes()); // channel_count
        bytes.extend_from_slice(&region_size.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes()); // region_count
        bytes.extend_from_slice(&0u32.to_le_bytes()); // material_count
        bytes.extend_from_slice(&0i32.to_le_bytes()); // coord x
        bytes.extend_from_slice(&0i32.to_le_bytes()); // coord z
        bytes.push(REGION_FLAG_PRESENT);
        bytes.extend_from_slice(&[0u8; 3]);

        assert_eq!(decode_regions(&bytes), Err(SidecarError::TooLarge));
    }

    /// A file claiming more channels than it carries does not make the
    /// reader allocate for them.
    #[test]
    fn an_oversized_channel_count_claim_is_rejected() {
        let data = RegionTerrainData {
            channels: vec![ChannelDescriptor::new("c", ChannelElement::U8)],
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let mut bytes = encode_regions(&data).expect("encodes");
        // Version 5 has no `channel_resolution`, so the count sits here.
        bytes[12..16].copy_from_slice(&1_000_000u32.to_le_bytes());
        assert_eq!(decode_regions(&bytes), Err(SidecarError::Truncated));
    }

    #[test]
    fn v2_rejects_trailing_bytes_after_a_complete_document() {
        let mut bytes = encode_regions(&sample_regions()).expect("encodes");
        bytes.push(0);
        assert_eq!(decode_regions(&bytes), Err(SidecarError::Truncated));
    }

    #[test]
    fn v2_rejects_an_under_claimed_region_count_that_leaves_bytes_unconsumed() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, 1.0);
        regions.set_height(10, 10, 2.0);
        let data = RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let mut bytes = encode_regions(&data).expect("encodes");
        // header(12)+chan_res(4)+chan_count(4)=20 is region_size, +4=24 is
        // region_count.
        bytes[20..24].copy_from_slice(&1u32.to_le_bytes()); // claim only 1 of 2
        assert_eq!(decode_regions(&bytes), Err(SidecarError::Truncated));
    }

    #[test]
    fn v2_rejects_a_duplicate_region_coordinate() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, 1.0);
        let data = RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        let bytes = encode_regions(&data).expect("encodes");
        // No channels, no texture set: the region table starts at 28 for
        // this fixture. Duplicate that one region entry and bump the
        // count to match.
        let region_entry = bytes[28
            + AUTOTERRAIN_BLOCK_LEN
            + GRID_BLOCK_LEN
            + SURFACE_BLOCK_LEN
            + EMPTY_SCATTER_BLOCK_LEN..]
            .to_vec();
        let mut poisoned = bytes.clone();
        poisoned[20..24].copy_from_slice(&2u32.to_le_bytes());
        poisoned.extend_from_slice(&region_entry);
        assert_eq!(
            decode_regions(&poisoned),
            Err(SidecarError::DuplicateRegion(RegionCoord::ORIGIN))
        );
    }

    #[test]
    fn a_default_document_is_empty_and_round_trips() {
        let data = RegionTerrainData::default();
        assert_eq!(data.regions.region_count(), 0);
        assert_eq!(data.regions.region_size(), RegionSize::DEFAULT);
        assert!(data.contiguous_grid().is_none());
        let back = decode_regions(&encode_regions(&data).expect("encodes")).expect("decodes");
        assert_eq!(back, data);
    }

    /// A grid that is one whole region is borrowed in place, and stays
    /// borrowable after paint, where `as_legacy` refuses.
    #[test]
    fn a_grid_of_one_region_stays_borrowable_after_paint_as_legacy_refuses() {
        let mut doc = RegionTerrainData::from_legacy_v1(&sample()).unwrap();
        doc.regions
            .set_control(0, 0, Control::default().with_base_id(1));
        assert_eq!(doc.as_legacy(), None);

        let region = doc.contiguous_grid().expect("one region holds the grid");
        assert_eq!(region.side(), 4);
        assert_eq!(region.heights(), sample().heights.as_slice());

        doc.contiguous_grid_mut().expect("writable").heights_mut()[0] = 42.0;
        assert_eq!(doc.regions.height_at(0, 0), 42.0);
        assert_eq!(doc.regions.region_count(), 1);
    }

    /// A grid that spans regions has no single region to borrow, and
    /// reads and writes through the gathering path instead.
    #[test]
    fn a_grid_that_spans_regions_gathers_rather_than_borrowing() {
        let mut doc = RegionTerrainData::from_legacy_v1(&TerrainData {
            resolution: 3,
            heights: vec![1.0; 9],
            channels: vec![],
        })
        .expect("migrates");
        assert!(doc.contiguous_grid().is_none());
        assert!(doc.contiguous_grid_mut().is_none());
        // Four 2-cell regions hold the 3-vertex grid, so the terrain is
        // four cells a side with the authored three embedded in it.
        assert_eq!(doc.grid_resolution(), 4);
        assert_embedded(&doc.regions, &[1.0; 9], 3, 4);

        // Written and read back at the terrain's own extent.
        let mut written: Vec<f32> = (0..16).map(|i| i as f32).collect();
        doc.set_grid_heights(&written);
        assert_eq!(doc.grid_heights(), written);
        // The seam vertex lives in the next region along, not in the one
        // holding the rest of its row.
        assert_eq!(doc.regions.height_at(2, 0), 2.0);
        assert!(doc.regions.region(RegionCoord::new(1, 0)).is_some());

        written[8] = 9.0;
        doc.set_grid_heights(&written);
        assert_eq!(doc.grid_heights()[8], 9.0);
    }

    #[test]
    fn a_grid_is_not_borrowable_when_the_regions_size_does_not_match_it() {
        let mut doc = RegionTerrainData::from_legacy_v1(&sample()).unwrap();
        assert!(doc.contiguous_grid().is_some());
        // A second region along puts the grid past the one at the origin,
        // so there is no single layer to hand out.
        doc.regions.ensure_region(RegionCoord::new(1, 0));
        assert_eq!(doc.grid_resolution(), 8);
        assert!(doc.contiguous_grid().is_none());
        assert!(doc.contiguous_grid_mut().is_none());
    }

    #[test]
    fn a_document_whose_only_region_is_elsewhere_has_nothing_to_borrow() {
        let mut regions = TerrainRegions::new(RegionSize::new(4).unwrap());
        regions.set_height(100, 100, 1.0);
        let doc = RegionTerrainData {
            channels: vec![],
            regions,
            materials: Vec::new(),
            autoterrain: AutoTerrainSettings::default(),
            surface: SurfaceSettings::default(),
            grid: Some(GridGeometry::DEFAULT),
            scatter: ScatterPalette::default(),
        };
        // The grid reaches out to that region, so it is far wider than the
        // one region holding anything and there is no single layer to hand
        // out.
        assert!(doc.contiguous_grid().is_none());
        assert_eq!(doc.grid_resolution(), 104);
    }

    #[test]
    fn default_color_constant_is_reexported_and_used_by_a_fresh_terrain() {
        let t = TerrainRegions::new(RegionSize::new(4).unwrap());
        assert_eq!(t.color_at(0, 0), DEFAULT_COLOR);
    }
}
