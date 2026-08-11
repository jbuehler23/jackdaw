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

use std::path::{Component, Path, PathBuf};

use crate::channel::{ChannelData, ChannelElement};

/// Magic bytes at the head of every sidecar.
pub const MAGIC: [u8; 8] = *b"JDTERRN\0";

/// Format version this build writes.
pub const VERSION: u16 = 1;

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

/// Validate `data_path` and resolve it beneath `scene_dir`.
///
/// Scene metadata is portable, so validation is deliberately stricter than
/// the host platform: backslashes and Windows drive prefixes are rejected on
/// every platform, as are empty, `.` and `..` components.
pub fn resolve_path(scene_dir: &Path, data_path: &str) -> Result<PathBuf, SidecarPathError> {
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

    if path.extension().and_then(|extension| extension.to_str()) != Some(EXTENSION) {
        return Err(SidecarPathError::WrongExtension);
    }

    Ok(scene_dir.join(path))
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
}
