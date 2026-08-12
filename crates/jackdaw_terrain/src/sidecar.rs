//! Binary sidecar format for a terrain's bulk per-cell data.
//!
//! A terrain's heights and paint channels are far too large to live in a
//! text scene file: a 512-resolution heightmap is 262,144 floats, and a
//! `.bsn` document is meant to be read in a `git diff` without crying. They
//! go in a versioned binary file beside the scene instead, and the scene
//! keeps only the small descriptive parts -- resolution, world size, the
//! channel table.
//!
//! The format is deliberately plain so an external pipeline can read it in
//! any language without linking jackdaw:
//!
//! ```text
//! offset  size          field
//! 0       8             magic, b"JDTERRN\0"
//! 8       2             format version, u16
//! 10      2             flags, u16 (reserved, must be 0)
//! 12      4             resolution, u32
//! 16      4             channel count, u32
//! 20      4*res*res     heights, f32
//! then, per channel:
//!         4             name length in bytes, u32
//!         n             name, UTF-8
//!         1             element tag, u8 (0 = u8, 1 = u16)
//!         3             padding, must be 0
//!         w*res*res     values, u8 or u16 per the tag
//! ```
//!
//! Every integer and float is little-endian. Encoding is a pure function of
//! the data, so the same terrain always produces the same bytes and a
//! sidecar can be committed and diffed.
//!
//! There is no checksum. A file can be truncated or bit-flipped by
//! something outside the atomic-write path this format's own writer uses
//! (a bad disk, a naive sync tool) and still pass magic/version/length
//! checks while carrying wrong values -- decode only catches structural
//! corruption (bad magic, an unreadable version, a length that does not
//! fit the claimed shape), not silent bit rot within otherwise
//! well-formed bytes. Accepted as a tradeoff for the format staying
//! trivial to read from any language; revisit if this ever needs to
//! survive untrusted or unreliable transport.
//!
//! # v3: regions
//!
//! [`VERSION`] (1) is the layout above: one dense heightmap plus channels,
//! everything sized to a single `resolution`. It is what the terrain
//! track's design doc calls "v2" -- there has only ever been one shipped
//! format before this one, and the doc's v2/v3 numbering counts generations
//! of the design, not this field's literal value. [`VERSION_V3`] (2) is
//! what the doc calls "v3": heights, the control map and an optional color
//! layer move into sparse regions (see [`crate::region`]), addressed by
//! [`RegionCoord`]. Channels are unaffected -- they are gameplay masks, not
//! visual splat, and stay dense at `channel_resolution` exactly as they
//! were at `resolution` before.
//!
//! ```text
//! offset  size          field
//! 0       8             magic, b"JDTERRN\0"             (shared header)
//! 8       2             format version, u16 == VERSION_V3
//! 10      2             flags, u16 (reserved, must be 0)
//! 12      4             channel_resolution, u32
//! 16      4             channel_count, u32
//! 20      ...           channels, same per-channel layout as v1
//! then
//!         4             region_size, u32 (cells per region edge, > 0)
//!         4             region_count, u32
//!         4             texture_set path length in bytes, u32 (0 = none)
//!         n             texture_set path, UTF-8, project-relative
//! then, per region (region_count times):
//!         4             region coord x, i32
//!         4             region coord z, i32
//!         1             region flags, u8 (bit 0 = has color layer)
//!         3             padding, must be 0
//!         4*size^2      heights, f32
//!         4*size^2      control, u32 (packed, see crate::control)
//!         4*size^2      color RGBA8 -- only present if flag bit 0 is set
//! ```
//!
//! [`load`] and [`save`] are the entry points call sites should use going
//! forward: `load` transparently upgrades a v1 file into a single implicit
//! region at the origin sized to its old resolution, with no control or
//! color data, and `save` always writes v3. A newer-than-this-build version
//! is refused by both, the same as v1 -- a file this build cannot
//! understand is never silently dropped or overwritten. The bare
//! `encode`/`decode` (v1) and `encode_regions`/`decode_regions` (v3) pairs
//! stay available for testing and for code that specifically needs one
//! format or the other.

use std::path::{Component, Path, PathBuf};

use crate::channel::{ChannelData, ChannelElement};
use crate::control::Control;
use crate::region::{Region, RegionCoord, RegionSize, TerrainRegions};

/// Magic bytes at the head of every sidecar.
pub const MAGIC: [u8; 8] = *b"JDTERRN\0";

/// Format version [`encode`]/[`decode`] read and write: the original
/// single dense heightmap layout, with no regions.
pub const VERSION: u16 = 1;

/// Format version [`encode_regions`]/[`decode_regions`] read and write, and
/// what [`save`] always writes: regions replace the dense heightmap. See
/// the module-level "v3: regions" section for the full layout and how the
/// version numbers here map onto the terrain track design doc's v2/v3
/// naming.
pub const VERSION_V3: u16 = 2;

/// Conventional file extension for a terrain sidecar.
pub const EXTENSION: &str = "jdterrain";

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
/// accepts from a file: sidecar `data_path` (resolved beneath a scene) and
/// texture-set references (resolved beneath the project) both go through
/// this before any format-specific check, so both stay equally hostile to
/// the same escape tricks.
///
/// Deliberately stricter than the host platform: backslashes and Windows
/// drive prefixes are rejected on every platform, as are empty, `.` and
/// `..` components.
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

/// Validate a texture-set asset reference: a project-relative path string,
/// stored on a v3 sidecar and resolved beneath the project root by whatever
/// asset system loads it (T2/T3, not this crate). Same traversal guard as
/// [`resolve_path`], minus the sidecar-specific extension check -- the
/// texture-set asset's extension is not this crate's concern.
pub fn validate_texture_set_ref(path: &str) -> Result<(), SidecarPathError> {
    reject_path_traversal(path)
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
    /// The file ended before the declared data did.
    Truncated,
    /// A channel declared an element tag this build does not know.
    UnknownElement(u8),
    /// A channel name was not valid UTF-8.
    BadName,
    /// The declared dimensions do not fit in this platform's address space.
    TooLarge,
    /// A v3 region table declared a region size of zero.
    InvalidRegionSize,
    /// The texture-set reference string is unsafe or malformed.
    InvalidTextureSetRef(SidecarPathError),
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
            Self::InvalidRegionSize => write!(f, "terrain sidecar declares a region size of zero"),
            Self::InvalidTextureSetRef(reason) => {
                write!(
                    f,
                    "terrain sidecar texture-set reference is invalid: {reason}"
                )
            }
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

    for channel in &data.channels {
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

    Some(out)
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

/// Deserialize a terrain's bulk data.
pub fn decode(bytes: &[u8]) -> Result<TerrainData, SidecarError> {
    let mut r = Reader { bytes, at: 0 };

    if r.take(MAGIC.len())? != MAGIC {
        return Err(SidecarError::BadMagic);
    }
    let version = r.u16()?;
    // 0 has never been a version any build wrote (VERSION starts at 1);
    // accepting it would let a file whose version bytes were zeroed out
    // by corruption parse as if it were a real, if ancient, format.
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

    // A truncated file must not make us allocate its claimed size, so the
    // remaining length is checked before every reservation.
    let heights_bytes = r.take(cells.checked_mul(4).ok_or(SidecarError::TooLarge)?)?;
    let mut heights = Vec::with_capacity(cells);
    for chunk in heights_bytes.chunks_exact(4) {
        heights.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }

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

    Ok(TerrainData {
        resolution,
        heights,
        channels,
    })
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

/// A v3 terrain document: channels (unchanged from v1) plus sparse
/// region-based heights/control/color, plus the texture set the splat
/// material paints with.
#[derive(Clone, Debug, PartialEq)]
pub struct RegionTerrainData {
    /// Sizes `channels` exactly as `resolution` sized them in v1. Unrelated
    /// to region addressing -- channels are not part of the region model.
    pub channel_resolution: u32,
    /// Named integer layers, in the order the project declared them.
    pub channels: Vec<ChannelData>,
    /// Sparse heights, control map and color.
    pub regions: TerrainRegions,
    /// Project-relative path to the texture set this terrain paints with,
    /// or `None` if it has not been assigned one yet.
    pub texture_set: Option<String>,
}

impl RegionTerrainData {
    /// Migrate a v1 [`TerrainData`] into a v3 document: one implicit region
    /// at [`RegionCoord::ORIGIN`], sized to the legacy resolution exactly
    /// -- no resampling. The migrated region carries no control or color
    /// data (both stay at their all-default values). Channels are moved
    /// across unchanged.
    pub fn from_legacy_v1(data: &TerrainData) -> Self {
        let mut normalized = data.clone();
        normalized.normalize();
        let resolution = normalized.resolution;

        let mut regions = TerrainRegions::new(RegionSize::new_unchecked(resolution.max(1)));
        if resolution > 0 {
            let cells = (resolution as usize) * (resolution as usize);
            regions.insert_region(
                RegionCoord::ORIGIN,
                Region::from_parts(resolution, normalized.heights, vec![0u32; cells], None),
            );
        }

        Self {
            channel_resolution: resolution,
            channels: normalized.channels,
            regions,
            texture_set: None,
        }
    }

    /// A [`TerrainData`] view of this document, for call sites that have
    /// not migrated to the region API yet -- the "compat view" that lets
    /// existing single-blob call sites keep working unmodified as long as
    /// a terrain is still, in every sense, v1-shaped: at most one region,
    /// at the origin, sized to `channel_resolution`, with no control or
    /// color paint. Returns `None` once any of that stops holding, rather
    /// than lossily flattening real region/control/color data into a shape
    /// that cannot represent it -- at that point the call site has to move
    /// to the region API for real.
    pub fn as_legacy(&self) -> Option<TerrainData> {
        let heights = match self.regions.region_count() {
            0 => vec![0.0; (self.channel_resolution as usize) * (self.channel_resolution as usize)],
            1 => {
                let (coord, region) = self.regions.iter_sorted().next()?;
                if coord != RegionCoord::ORIGIN {
                    return None;
                }
                if region.side() != self.channel_resolution {
                    return None;
                }
                if region.color().is_some() {
                    return None;
                }
                if region.control_words().iter().any(|c| *c != 0) {
                    return None;
                }
                region.heights().to_vec()
            }
            _ => return None,
        };

        Some(TerrainData {
            resolution: self.channel_resolution,
            heights,
            channels: self.channels.clone(),
        })
    }
}

/// Serialize a v3 terrain document.
pub fn encode_regions(data: &RegionTerrainData) -> Option<Vec<u8>> {
    let channel_cells =
        (data.channel_resolution as usize).checked_mul(data.channel_resolution as usize)?;

    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION_V3.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&data.channel_resolution.to_le_bytes());
    out.extend_from_slice(&(data.channels.len() as u32).to_le_bytes());
    encode_channels(&mut out, &data.channels, channel_cells);

    out.extend_from_slice(&data.regions.region_size().get().to_le_bytes());
    out.extend_from_slice(&(data.regions.region_count() as u32).to_le_bytes());

    let texture_set = data.texture_set.as_deref().unwrap_or("");
    out.extend_from_slice(&(texture_set.len() as u32).to_le_bytes());
    out.extend_from_slice(texture_set.as_bytes());

    for (coord, region) in data.regions.iter_sorted() {
        out.extend_from_slice(&coord.x.to_le_bytes());
        out.extend_from_slice(&coord.z.to_le_bytes());
        let flags: u8 = if region.color().is_some() { 0x1 } else { 0 };
        out.push(flags);
        out.extend_from_slice(&[0u8; 3]);

        for h in region.heights() {
            out.extend_from_slice(&h.to_le_bytes());
        }
        for c in region.control_words() {
            out.extend_from_slice(&c.to_le_bytes());
        }
        if let Some(color) = region.color() {
            for px in color {
                out.extend_from_slice(px);
            }
        }
    }

    Some(out)
}

/// Deserialize a v3 terrain document.
pub fn decode_regions(bytes: &[u8]) -> Result<RegionTerrainData, SidecarError> {
    let mut r = Reader { bytes, at: 0 };

    if r.take(MAGIC.len())? != MAGIC {
        return Err(SidecarError::BadMagic);
    }
    let version = r.u16()?;
    if version != VERSION_V3 {
        return Err(SidecarError::UnsupportedVersion(version));
    }
    if r.u16()? != 0 {
        return Err(SidecarError::ReservedFieldSet);
    }

    let channel_resolution = r.u32()?;
    let channel_count = r.u32()?;
    let channel_cells = (channel_resolution as usize)
        .checked_mul(channel_resolution as usize)
        .ok_or(SidecarError::TooLarge)?;
    let channels = decode_channels(&mut r, channel_count, channel_cells)?;

    let region_size = r.u32()?;
    if region_size == 0 {
        return Err(SidecarError::InvalidRegionSize);
    }
    let region_count = r.u32()?;

    let texture_set_len = r.u32()? as usize;
    let texture_set_bytes = r.take(texture_set_len)?;
    let texture_set = if texture_set_bytes.is_empty() {
        None
    } else {
        let path = core::str::from_utf8(texture_set_bytes)
            .map_err(|_| SidecarError::BadName)?
            .to_string();
        validate_texture_set_ref(&path).map_err(SidecarError::InvalidTextureSetRef)?;
        Some(path)
    };

    let cells = (region_size as usize)
        .checked_mul(region_size as usize)
        .ok_or(SidecarError::TooLarge)?;

    let mut regions = TerrainRegions::new(RegionSize::new_unchecked(region_size));
    for _ in 0..region_count {
        let x = r.i32()?;
        let z = r.i32()?;
        let flags = r.u8()?;
        if flags & !0x1 != 0 {
            return Err(SidecarError::ReservedFieldSet);
        }
        if r.take(3)? != [0u8; 3] {
            return Err(SidecarError::ReservedFieldSet);
        }
        let has_color = flags & 0x1 != 0;

        let heights_bytes = r.take(cells.checked_mul(4).ok_or(SidecarError::TooLarge)?)?;
        let heights: Vec<f32> = heights_bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();

        let control_bytes = r.take(cells.checked_mul(4).ok_or(SidecarError::TooLarge)?)?;
        let mut control = Vec::with_capacity(cells);
        for chunk in control_bytes.chunks_exact(4) {
            let raw = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            if Control::from_raw(raw).reserved() != 0 {
                return Err(SidecarError::ReservedFieldSet);
            }
            control.push(raw);
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

        regions.insert_region(
            RegionCoord::new(x, z),
            Region::from_parts(region_size, heights, control, color),
        );
    }

    Ok(RegionTerrainData {
        channel_resolution,
        channels,
        regions,
        texture_set,
    })
}

/// Load a sidecar of either version, upgrading a v1 file into a v3
/// document in memory. Refuses a file written by a newer build than this
/// one understands, the same as [`decode`]/[`decode_regions`] each do for
/// their own version.
pub fn load(bytes: &[u8]) -> Result<RegionTerrainData, SidecarError> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(MAGIC.len())? != MAGIC {
        return Err(SidecarError::BadMagic);
    }
    let version = r.u16()?;

    match version {
        0 => Err(SidecarError::UnsupportedVersion(0)),
        VERSION => decode(bytes).map(|data| RegionTerrainData::from_legacy_v1(&data)),
        VERSION_V3 => decode_regions(bytes),
        other => Err(SidecarError::UnsupportedVersion(other)),
    }
}

/// Serialize a terrain document as the current version (v3). The inverse
/// of [`load`] for any file [`load`] can produce.
pub fn save(data: &RegionTerrainData) -> Option<Vec<u8>> {
    encode_regions(data)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

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

    /// M5: version 0 has never been written by any build and must not be
    /// accepted as if it were a real, if ancient, format -- most likely
    /// to show up from version bytes zeroed out by corruption.
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
        // A second, negatively-coordinate region, to exercise multi-region
        // and negative-coord encode/decode together.
        regions.set_height(-1, -4, 9.0);

        RegionTerrainData {
            channel_resolution: 4,
            channels: vec![{
                let mut c = ChannelData::new("biome", ChannelElement::U8, 4);
                c.values[0] = 7;
                c
            }],
            regions,
            texture_set: Some("textures/zone1.jdtextureset".to_string()),
        }
    }

    #[test]
    fn v3_round_trips_regions_channels_and_texture_set() {
        let data = sample_regions();
        let bytes = encode_regions(&data).expect("encodes");
        let back = decode_regions(&bytes).expect("decodes");
        assert_eq!(back, data);
    }

    #[test]
    fn v3_encoding_is_deterministic_regardless_of_edit_order() {
        let mut a = TerrainRegions::new(RegionSize::new(4).unwrap());
        a.set_height(0, 0, 1.0);
        a.set_height(20, 20, 2.0);
        a.set_height(-9, 4, 3.0);
        let mut b = TerrainRegions::new(RegionSize::new(4).unwrap());
        b.set_height(-9, 4, 3.0);
        b.set_height(20, 20, 2.0);
        b.set_height(0, 0, 1.0);

        let da = RegionTerrainData {
            channel_resolution: 0,
            channels: vec![],
            regions: a,
            texture_set: None,
        };
        let db = RegionTerrainData {
            channel_resolution: 0,
            channels: vec![],
            regions: b,
            texture_set: None,
        };
        assert_eq!(encode_regions(&da), encode_regions(&db));
    }

    #[test]
    fn v3_with_no_texture_set_round_trips_to_none() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, 1.0);
        let data = RegionTerrainData {
            channel_resolution: 0,
            channels: vec![],
            regions,
            texture_set: None,
        };
        let back = decode_regions(&encode_regions(&data).expect("encodes")).expect("decodes");
        assert_eq!(back.texture_set, None);
    }

    #[test]
    fn v3_with_an_empty_region_set_round_trips() {
        let data = RegionTerrainData {
            channel_resolution: 0,
            channels: vec![],
            regions: TerrainRegions::new(RegionSize::new(64).unwrap()),
            texture_set: None,
        };
        let back = decode_regions(&encode_regions(&data).expect("encodes")).expect("decodes");
        assert_eq!(back, data);
    }

    #[test]
    fn migrating_a_real_v1_file_produces_a_single_origin_region() {
        let migrated = RegionTerrainData::from_legacy_v1(&sample());
        assert_eq!(migrated.channel_resolution, 4);
        assert_eq!(migrated.channels, sample().channels);
        assert_eq!(migrated.regions.region_count(), 1);
        assert_eq!(migrated.texture_set, None);
        let region = migrated.regions.region(RegionCoord::ORIGIN).unwrap();
        assert_eq!(region.side(), 4);
        assert_eq!(region.heights(), sample().heights.as_slice());
        assert!(region.control_words().iter().all(|c| *c == 0));
        assert!(region.color().is_none());
    }

    #[test]
    fn migrating_an_empty_v1_terrain_allocates_no_regions() {
        let migrated = RegionTerrainData::from_legacy_v1(&TerrainData::default());
        assert_eq!(migrated.regions.region_count(), 0);
        assert_eq!(migrated.channel_resolution, 0);
    }

    #[test]
    fn v1_to_v3_migration_round_trips_through_load_and_save() {
        // Build a real v1 file with the existing encoder.
        let original = sample();
        let v1_bytes = encode(&original).expect("v1 encodes");

        // Load it: auto-upgrades to a v3 document, single implicit region.
        let migrated = load(&v1_bytes).expect("loads v1");
        assert_eq!(migrated, RegionTerrainData::from_legacy_v1(&original));
        assert_eq!(migrated.as_legacy(), Some(original.clone()));

        // Save it: always writes v3.
        let v3_bytes = save(&migrated).expect("v3 encodes");
        assert_eq!(u16::from_le_bytes([v3_bytes[8], v3_bytes[9]]), VERSION_V3);
        assert_ne!(v3_bytes[8..10], v1_bytes[8..10]);

        // Reload the v3 bytes: matches what was saved exactly.
        let reloaded = load(&v3_bytes).expect("loads v3");
        assert_eq!(reloaded, migrated);
    }

    #[test]
    fn as_legacy_refuses_once_control_has_been_painted() {
        let mut migrated = RegionTerrainData::from_legacy_v1(&sample());
        migrated
            .regions
            .set_control(0, 0, Control::default().with_base_id(1));
        assert_eq!(migrated.as_legacy(), None);
    }

    #[test]
    fn as_legacy_refuses_once_color_has_been_painted() {
        let mut migrated = RegionTerrainData::from_legacy_v1(&sample());
        migrated.regions.set_color(0, 0, [1, 2, 3, 4]);
        assert_eq!(migrated.as_legacy(), None);
    }

    #[test]
    fn as_legacy_refuses_once_a_terrain_goes_multi_region() {
        let mut migrated = RegionTerrainData::from_legacy_v1(&sample());
        migrated.regions.set_height(100, 100, 1.0);
        assert_eq!(migrated.as_legacy(), None);
    }

    #[test]
    fn as_legacy_succeeds_for_a_never_edited_v3_native_terrain() {
        let data = RegionTerrainData {
            channel_resolution: 3,
            channels: vec![],
            regions: TerrainRegions::new(RegionSize::new(256).unwrap()),
            texture_set: None,
        };
        assert_eq!(
            data.as_legacy(),
            Some(TerrainData {
                resolution: 3,
                heights: vec![0.0; 9],
                channels: vec![],
            })
        );
    }

    #[test]
    fn rejects_a_v3_file_written_by_a_newer_build() {
        let bytes = encode_regions(&sample_regions()).expect("encodes");
        let mut newer = bytes.clone();
        newer[8..10].copy_from_slice(&(VERSION_V3 + 1).to_le_bytes());
        assert_eq!(
            decode_regions(&newer),
            Err(SidecarError::UnsupportedVersion(VERSION_V3 + 1))
        );
        assert_eq!(
            load(&newer),
            Err(SidecarError::UnsupportedVersion(VERSION_V3 + 1))
        );
    }

    #[test]
    fn load_never_misparses_one_version_as_the_other() {
        let v1 = encode(&sample()).expect("encodes");
        let v3 = encode_regions(&sample_regions()).expect("encodes");
        assert_eq!(
            decode_regions(&v1),
            Err(SidecarError::UnsupportedVersion(VERSION))
        );
        assert_eq!(
            decode(&v3),
            Err(SidecarError::UnsupportedVersion(VERSION_V3))
        );
    }

    #[test]
    fn v3_rejects_version_zero() {
        let mut bytes = encode_regions(&sample_regions()).expect("encodes");
        bytes[8..10].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(
            decode_regions(&bytes),
            Err(SidecarError::UnsupportedVersion(0))
        );
        assert_eq!(load(&bytes), Err(SidecarError::UnsupportedVersion(0)));
    }

    #[test]
    fn v3_rejects_a_set_header_reserved_flag() {
        let mut bytes = encode_regions(&sample_regions()).expect("encodes");
        bytes[10..12].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(decode_regions(&bytes), Err(SidecarError::ReservedFieldSet));
    }

    #[test]
    fn v3_rejects_a_region_size_of_zero() {
        let data = RegionTerrainData {
            channel_resolution: 0,
            channels: vec![],
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            texture_set: None,
        };
        let mut bytes = encode_regions(&data).expect("encodes");
        // header(12) + channel_resolution(4) + channel_count(4) = offset 20
        // is region_size (no channels in this fixture).
        bytes[20..24].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(decode_regions(&bytes), Err(SidecarError::InvalidRegionSize));
    }

    #[test]
    fn v3_rejects_reserved_bits_set_in_a_control_word() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_control(0, 0, Control::default().with_base_id(1));
        let data = RegionTerrainData {
            channel_resolution: 0,
            channels: vec![],
            regions,
            texture_set: None,
        };
        let bytes = encode_regions(&data).expect("encodes");

        // Locate the control block: header(12) + channel_resolution(4) +
        // channel_count(4) + region_size(4) + region_count(4) +
        // texture_set_len(4) + coord(8) + flags+pad(4) + heights(4*4) = 60
        // is where the control words start for this 2x2 region.
        let control_offset = 60;
        let mut poisoned = bytes.clone();
        let word = u32::from_le_bytes([
            poisoned[control_offset],
            poisoned[control_offset + 1],
            poisoned[control_offset + 2],
            poisoned[control_offset + 3],
        ]);
        let with_reserved = word | (1 << 16);
        poisoned[control_offset..control_offset + 4].copy_from_slice(&with_reserved.to_le_bytes());

        assert_eq!(
            decode_regions(&poisoned),
            Err(SidecarError::ReservedFieldSet)
        );
    }

    #[test]
    fn v3_rejects_reserved_bits_set_in_region_flags() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, 1.0);
        let data = RegionTerrainData {
            channel_resolution: 0,
            channels: vec![],
            regions,
            texture_set: None,
        };
        let bytes = encode_regions(&data).expect("encodes");
        // header(12) + channel_resolution(4) + channel_count(4) +
        // region_size(4) + region_count(4) + texture_set_len(4) +
        // coord(8) = offset 40 is the region flags byte.
        let flags_offset = 40;
        let mut poisoned = bytes.clone();
        poisoned[flags_offset] |= 0b1000_0000;
        assert_eq!(
            decode_regions(&poisoned),
            Err(SidecarError::ReservedFieldSet)
        );
    }

    #[test]
    fn v3_rejects_nonzero_region_padding() {
        let mut regions = TerrainRegions::new(RegionSize::new(2).unwrap());
        regions.set_height(0, 0, 1.0);
        let data = RegionTerrainData {
            channel_resolution: 0,
            channels: vec![],
            regions,
            texture_set: None,
        };
        let bytes = encode_regions(&data).expect("encodes");
        let pad_offset = 41; // one byte after the flags byte at 40
        let mut poisoned = bytes.clone();
        poisoned[pad_offset] = 1;
        assert_eq!(
            decode_regions(&poisoned),
            Err(SidecarError::ReservedFieldSet)
        );
    }

    #[test]
    fn v3_rejects_an_invalid_texture_set_reference() {
        let data = RegionTerrainData {
            channel_resolution: 0,
            channels: vec![],
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            texture_set: Some("../escape.jdtextureset".to_string()),
        };
        let bytes = encode_regions(&data).expect("encodes");
        assert!(matches!(
            decode_regions(&bytes),
            Err(SidecarError::InvalidTextureSetRef(_))
        ));
    }

    #[test]
    fn validate_texture_set_ref_accepts_a_plain_relative_path_and_rejects_traversal() {
        assert!(validate_texture_set_ref("textures/zone1.jdtextureset").is_ok());
        for bad in ["", "../x", "/abs/x", "a/../b", r"a\b", "C:/x"] {
            assert!(
                validate_texture_set_ref(bad).is_err(),
                "{bad:?} must not validate"
            );
        }
    }

    #[test]
    fn v3_a_truncated_file_is_rejected_at_every_boundary() {
        let bytes = encode_regions(&sample_regions()).expect("encodes");
        for cut in [0, 8, 10, 12, 20, 40, 60, bytes.len() - 1] {
            assert_eq!(
                decode_regions(&bytes[..cut]),
                Err(SidecarError::Truncated),
                "cut at {cut} should be truncated"
            );
        }
    }

    #[test]
    fn v3_an_oversized_region_size_claim_over_a_tiny_file_is_rejected() {
        let data = RegionTerrainData {
            channel_resolution: 0,
            channels: vec![],
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            texture_set: None,
        };
        let mut bytes = encode_regions(&data).expect("encodes");
        // No regions were written, so forging a region_count of 1 against
        // an enormous region_size must not allocate that claim.
        bytes[20..24].copy_from_slice(&1_000_000u32.to_le_bytes()); // region_size
        bytes[24..28].copy_from_slice(&1u32.to_le_bytes()); // region_count
        assert_eq!(decode_regions(&bytes), Err(SidecarError::Truncated));
    }

    #[test]
    fn v3_an_oversized_channel_resolution_claim_is_rejected() {
        let data = RegionTerrainData {
            channel_resolution: 0,
            channels: vec![ChannelData::new("c", ChannelElement::U8, 0)],
            regions: TerrainRegions::new(RegionSize::new(4).unwrap()),
            texture_set: None,
        };
        let mut bytes = encode_regions(&data).expect("encodes");
        bytes[12..16].copy_from_slice(&1_000_000u32.to_le_bytes());
        assert_eq!(decode_regions(&bytes), Err(SidecarError::Truncated));
    }
}
