//! Binary sidecar format for a tilemap's per-cell data.
//!
//! Every tile and terrain layer of a tilemap lives in one `.jdtiles` file beside
//! the scene. Integers are little-endian and encoding is a pure function of the
//! data: chunks and overrides are written in ascending row-major order, so the
//! same map always produces the same bytes. Decode rejects anything encode
//! would never write, and the file must end exactly where the declared
//! structure ends.
//!
//! # Format version 2
//!
//! ```text
//! size      field
//! 8         magic, b"JDTILES\0"
//! 2         format version, u16 == 2
//! 2         flags, u16 (bit 0 = authoring data present, other bits must be 0)
//! 4         layer_count, u32
//! then, per layer:
//! 1         kind, u8 (0 = tile layer, 1 = terrain layer)
//! 3         reserved, must be 0
//! 4         chunk_count, u32
//! then, per chunk in ascending ChunkCoord order:
//! 4 + 4     chunk x, chunk y, i32
//! 1024 * 3  cells in local index order: index u16, orientation u8
//! then, only for a terrain layer when flags bit 0 is set:
//! 4         terrain_chunk_count, u32
//! then, per terrain chunk in ascending ChunkCoord order:
//! 4 + 4     chunk x, chunk y, i32
//! 1024      terrain id per cell, u8, in local index order
//! 4         override_count, u32
//! then, per override in ascending CellCoord order:
//! 4 + 4     cell x, cell y, i32
//! 2 + 1     index u16, orientation u8
//! 4         link_chunk_count, u32
//! then, per link chunk in ascending ChunkCoord order:
//! 4 + 4     chunk x, chunk y, i32
//! 1024      Links bits per cell, u8, in local index order
//! ```
//!
//! An empty cell is index `0xFFFF` with orientation 0. Stored chunks hold at
//! least one non-empty cell, overrides are never empty, and orientation and
//! link bytes never set reserved bits. A stored link cell has the Path bit and
//! a non-zero terrain id; encode prunes any other link data.
//!
//! Version 1 is version 2 without the link chunk list; decode reads it with
//! empty links and encode always writes version 2.
//!
//! Flags bit 0 is set exactly when some terrain layer has authoring data; then
//! every terrain layer carries an authoring block, written empty for a layer
//! whose authoring is `None`. Tile layers never carry one: encode ignores their
//! authoring. So `encode(decode(b)) == b` for every accepted version 2 file
//! (a version 1 file re-encodes as version 2), and
//! `decode(encode(d)) == d` once `d` is normalised: tile layer authoring is
//! `None`, empty overrides and invalid links are dropped, and when any terrain
//! layer has authoring the others hold `Some` empty authoring.

use std::collections::BTreeMap;

use glam::IVec2;

use crate::grid::{CHUNK_AREA, CHUNK_SIZE, CellCoord, ChunkCoord};
use crate::layer::{CellValue, ChunkedLayer};
use crate::path::Links;
use crate::tile::{TerrainId, TileRef};

pub const MAGIC: [u8; 8] = *b"JDTILES\0";
pub const VERSION: u16 = 2;
const FLAG_AUTHORING: u16 = 1 << 0;
const LAYER_HEADER: usize = 8;
const OVERRIDE_RECORD: usize = 11;
const MIN_CHUNK: i32 = i32::MIN / CHUNK_SIZE;
const MAX_CHUNK: i32 = i32::MAX / CHUNK_SIZE;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerKind {
    Tile,
    Terrain,
}

/// The painted intent behind a terrain layer's resolved tiles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TerrainAuthoring {
    pub terrain: ChunkedLayer<TerrainId>,
    /// Hand-placed tiles that win over the resolved terrain tile. Never empty.
    pub overrides: BTreeMap<CellCoord, TileRef>,
    /// Links written by Path strokes; see [`Links`].
    pub links: ChunkedLayer<Links>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LayerData {
    pub kind: LayerKind,
    pub tiles: ChunkedLayer<TileRef>,
    /// Only meaningful for [`LayerKind::Terrain`].
    pub authoring: Option<TerrainAuthoring>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TilemapData {
    pub layers: Vec<LayerData>,
}

impl TilemapData {
    /// Drops every layer's authoring data, leaving only resolved tiles.
    pub fn strip_authoring(&mut self) {
        for layer in &mut self.layers {
            layer.authoring = None;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidecarError {
    /// The file does not start with [`MAGIC`].
    BadMagic,
    /// Version 0, or written by a newer jackdaw than this one.
    UnsupportedVersion(u16),
    /// A reserved flag bit, reserved byte or reserved link bit was non-zero.
    ReservedBits,
    /// The file ended before the declared data did.
    Truncated,
    /// Bytes remained after every declared field was read.
    TrailingBytes,
    UnknownLayerKind(u8),
    /// A cell or override held a value encode never writes.
    InvalidCell,
    /// A link cell lacked the Path bit or sat on a cell without terrain.
    InvalidLinks,
    /// A stored chunk held only empty cells.
    EmptyChunk,
    /// Chunks or overrides were out of order or duplicated.
    Unordered,
}

impl core::fmt::Display for SidecarError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BadMagic => write!(f, "not a tilemap sidecar (bad magic)"),
            Self::UnsupportedVersion(v) => {
                write!(f, "tilemap sidecar version {v} is not supported")
            }
            Self::ReservedBits => write!(f, "tilemap sidecar sets a reserved field"),
            Self::Truncated => write!(f, "tilemap sidecar is truncated"),
            Self::TrailingBytes => write!(f, "tilemap sidecar has trailing bytes"),
            Self::UnknownLayerKind(k) => write!(f, "unknown tilemap layer kind {k}"),
            Self::InvalidCell => write!(f, "tilemap sidecar holds an invalid cell"),
            Self::InvalidLinks => write!(f, "tilemap sidecar holds invalid link data"),
            Self::EmptyChunk => write!(f, "tilemap sidecar stores an empty chunk"),
            Self::Unordered => write!(f, "tilemap sidecar entries are out of order"),
        }
    }
}

impl std::error::Error for SidecarError {}

/// Serialize a tilemap. See the module docs for what is normalised away.
pub fn encode(data: &TilemapData) -> Vec<u8> {
    let with_authoring = data
        .layers
        .iter()
        .any(|layer| layer.kind == LayerKind::Terrain && layer.authoring.is_some());
    let flags = if with_authoring { FLAG_AUTHORING } else { 0 };
    let empty = TerrainAuthoring::default();

    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    put_u32(&mut out, data.layers.len());
    for layer in &data.layers {
        let kind = match layer.kind {
            LayerKind::Tile => 0,
            LayerKind::Terrain => 1,
        };
        out.extend_from_slice(&[kind, 0, 0, 0]);
        write_chunks(&mut out, &layer.tiles, put_tile);
        if layer.kind == LayerKind::Terrain && with_authoring {
            let authoring = layer.authoring.as_ref().unwrap_or(&empty);
            write_chunks(&mut out, &authoring.terrain, Vec::push);
            let mut overrides = authoring
                .overrides
                .iter()
                .filter(|(_, tile)| !tile.is_empty());
            put_u32(&mut out, overrides.clone().count());
            for (cell, tile) in overrides.by_ref() {
                put_ivec(&mut out, cell.0);
                put_tile(&mut out, *tile);
            }
            let mut links = ChunkedLayer::new();
            for (cell, l) in authoring.links.cells() {
                if l.is_path() && authoring.terrain.get(cell) != 0 {
                    links.set(cell, l);
                }
            }
            write_chunks(&mut out, &links, |out, l| out.push(l.bits()));
        }
    }
    out
}

/// Deserialize a tilemap, rejecting anything [`encode`] would not produce.
pub fn decode(bytes: &[u8]) -> Result<TilemapData, SidecarError> {
    let mut r = Reader { bytes, at: 0 };
    if r.take(MAGIC.len())? != MAGIC {
        return Err(SidecarError::BadMagic);
    }
    let version = r.u16()?;
    if version == 0 || version > VERSION {
        return Err(SidecarError::UnsupportedVersion(version));
    }
    let flags = r.u16()?;
    if flags & !FLAG_AUTHORING != 0 {
        return Err(SidecarError::ReservedBits);
    }
    let with_authoring = flags & FLAG_AUTHORING != 0;

    let layer_count = r.count(LAYER_HEADER)?;
    let mut layers = Vec::with_capacity(layer_count);
    for _ in 0..layer_count {
        let header = r.take(4)?;
        let kind = match header[0] {
            0 => LayerKind::Tile,
            1 => LayerKind::Terrain,
            other => return Err(SidecarError::UnknownLayerKind(other)),
        };
        if header[1..] != [0, 0, 0] {
            return Err(SidecarError::ReservedBits);
        }
        let tiles = read_chunks(&mut r, 3, read_tile)?;
        let mut authoring = None;
        if kind == LayerKind::Terrain && with_authoring {
            let terrain = read_chunks(&mut r, 1, |cell| Ok(cell[0]))?;
            let mut overrides = BTreeMap::new();
            let mut last: Option<CellCoord> = None;
            for _ in 0..r.count(OVERRIDE_RECORD)? {
                let cell = CellCoord(r.ivec()?);
                let tile = read_tile(r.take(3)?)?;
                if tile.is_empty() {
                    return Err(SidecarError::InvalidCell);
                }
                check_ascending(&mut last, cell)?;
                overrides.insert(cell, tile);
            }
            let links = if version >= 2 {
                read_chunks(&mut r, 1, |cell| {
                    Links::from_bits(cell[0]).ok_or(SidecarError::ReservedBits)
                })?
            } else {
                ChunkedLayer::new()
            };
            if links
                .cells()
                .any(|(cell, l)| !l.is_path() || terrain.get(cell) == 0)
            {
                return Err(SidecarError::InvalidLinks);
            }
            authoring = Some(TerrainAuthoring {
                terrain,
                overrides,
                links,
            });
        }
        layers.push(LayerData {
            kind,
            tiles,
            authoring,
        });
    }
    if with_authoring && !layers.iter().any(|layer| layer.kind == LayerKind::Terrain) {
        return Err(SidecarError::ReservedBits);
    }

    if r.at != bytes.len() {
        return Err(SidecarError::TrailingBytes);
    }
    Ok(TilemapData { layers })
}

fn put_u32(out: &mut Vec<u8>, value: usize) {
    let value = u32::try_from(value).expect("tilemap count exceeds u32");
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_ivec(out: &mut Vec<u8>, value: IVec2) {
    out.extend_from_slice(&value.x.to_le_bytes());
    out.extend_from_slice(&value.y.to_le_bytes());
}

fn put_tile(out: &mut Vec<u8>, tile: TileRef) {
    out.extend_from_slice(&tile.index().to_le_bytes());
    out.push(tile.orientation());
}

fn write_chunks<T: CellValue>(
    out: &mut Vec<u8>,
    layer: &ChunkedLayer<T>,
    put: impl Fn(&mut Vec<u8>, T),
) {
    put_u32(out, layer.chunk_count());
    for (coord, cells) in layer.chunks() {
        put_ivec(out, coord.0);
        for &cell in cells {
            put(out, cell);
        }
    }
}

fn read_tile(cell: &[u8]) -> Result<TileRef, SidecarError> {
    let index = u16::from_le_bytes([cell[0], cell[1]]);
    let orientation = cell[2];
    let tile = TileRef::from_raw(index, orientation);
    if tile.orientation() != orientation {
        return Err(SidecarError::InvalidCell);
    }
    Ok(tile)
}

/// Reads a chunk table whose cells are `cell_size` bytes each.
fn read_chunks<T: CellValue>(
    r: &mut Reader<'_>,
    cell_size: usize,
    parse: impl Fn(&[u8]) -> Result<T, SidecarError>,
) -> Result<ChunkedLayer<T>, SidecarError> {
    let mut layer = ChunkedLayer::new();
    let mut last: Option<ChunkCoord> = None;
    for _ in 0..r.count(8 + cell_size * CHUNK_AREA)? {
        let coord = ChunkCoord(r.ivec()?);
        if !(MIN_CHUNK..=MAX_CHUNK).contains(&coord.0.x)
            || !(MIN_CHUNK..=MAX_CHUNK).contains(&coord.0.y)
        {
            return Err(SidecarError::InvalidCell);
        }
        check_ascending(&mut last, coord)?;
        let bytes = r.take(cell_size * CHUNK_AREA)?;
        let mut cells = Box::new([T::EMPTY; CHUNK_AREA]);
        for (slot, cell) in cells.iter_mut().zip(bytes.chunks_exact(cell_size)) {
            *slot = parse(cell)?;
        }
        if cells.iter().all(CellValue::is_empty) {
            return Err(SidecarError::EmptyChunk);
        }
        layer.insert_chunk(coord, cells);
    }
    Ok(layer)
}

fn check_ascending<K: Ord + Copy>(last: &mut Option<K>, key: K) -> Result<(), SidecarError> {
    if last.is_some_and(|last| last >= key) {
        return Err(SidecarError::Unordered);
    }
    *last = Some(key);
    Ok(())
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], SidecarError> {
        let end = self.at.checked_add(n).ok_or(SidecarError::Truncated)?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or(SidecarError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16, SidecarError> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, SidecarError> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn ivec(&mut self) -> Result<IVec2, SidecarError> {
        let x = self.u32()? as i32;
        let y = self.u32()? as i32;
        Ok(IVec2::new(x, y))
    }

    /// Reads a u32 count of records at least `min_size` bytes each, failing
    /// before any allocation when the remaining bytes cannot hold them.
    fn count(&mut self, min_size: usize) -> Result<usize, SidecarError> {
        let count = self.u32()? as usize;
        let needed = count.checked_mul(min_size).ok_or(SidecarError::Truncated)?;
        if needed > self.bytes.len() - self.at {
            return Err(SidecarError::Truncated);
        }
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Peer;

    const CHUNK_RECORD: usize = 8 + 3 * CHUNK_AREA;

    fn tile_layer(cells: &[(i32, i32, TileRef)]) -> LayerData {
        let mut tiles = ChunkedLayer::new();
        for &(x, y, tile) in cells {
            tiles.set(CellCoord::new(x, y), tile);
        }
        LayerData {
            kind: LayerKind::Tile,
            tiles,
            authoring: None,
        }
    }

    fn sample_map() -> TilemapData {
        let ground = tile_layer(&[
            (0, 0, TileRef::new(1)),
            (-1, -1, TileRef::new(2).with_flip_x(true)),
            (
                40,
                -70,
                TileRef::new(3).with_transpose(true).with_visible(false),
            ),
        ]);
        let mut terrain = tile_layer(&[(-33, 5, TileRef::new(9)), (2, 2, TileRef::new(4))]);
        terrain.kind = LayerKind::Terrain;
        let mut authoring = TerrainAuthoring::default();
        authoring.terrain.set(CellCoord::new(-33, 5), 1);
        authoring.terrain.set(CellCoord::new(-64, -64), 2);
        authoring.terrain.set(CellCoord::new(2, 2), 3);
        authoring
            .overrides
            .insert(CellCoord::new(-33, 5), TileRef::new(9));
        authoring.overrides.insert(
            CellCoord::new(-1000, 1000),
            TileRef::new(7).with_flip_y(true),
        );
        let path = Links::EMPTY.with_path();
        authoring.terrain.set(CellCoord::new(-32, 5), 1);
        authoring.terrain.set(CellCoord::new(-70, -2), 2);
        authoring
            .links
            .set(CellCoord::new(-33, 5), path.with(Peer::Right));
        authoring
            .links
            .set(CellCoord::new(-32, 5), path.with(Peer::Left));
        authoring.links.set(CellCoord::new(-70, -2), path);
        terrain.authoring = Some(authoring);
        TilemapData {
            layers: vec![ground, terrain],
        }
    }

    #[test]
    fn round_trip_with_authoring_at_negative_coords() {
        let map = sample_map();
        let bytes = encode(&map);
        let decoded = decode(&bytes).unwrap();
        assert_eq!(decoded, map);
        assert_eq!(encode(&decoded), bytes);
    }

    #[test]
    fn encoding_is_deterministic_across_insertion_order() {
        let map = sample_map();
        assert_eq!(encode(&map), encode(&map));

        let mut ground = tile_layer(&[
            (
                40,
                -70,
                TileRef::new(3).with_transpose(true).with_visible(false),
            ),
            (-1, -1, TileRef::new(2).with_flip_x(true)),
            (5, 5, TileRef::new(8)),
            (0, 0, TileRef::new(1)),
        ]);
        ground.tiles.clear(CellCoord::new(5, 5));
        let mut reordered = map.clone();
        reordered.layers[0] = ground;
        assert_eq!(encode(&reordered), encode(&map));
    }

    #[test]
    fn empty_map_round_trips() {
        let bytes = encode(&TilemapData::default());
        assert_eq!(bytes.len(), 16);
        assert_eq!(decode(&bytes).unwrap(), TilemapData::default());
    }

    #[test]
    fn stripped_authoring_round_trips_without_authoring() {
        let mut map = sample_map();
        let full = encode(&map);
        map.strip_authoring();
        assert!(map.layers.iter().all(|layer| layer.authoring.is_none()));
        let bytes = encode(&map);
        assert!(bytes.len() < full.len());
        assert_eq!(u16::from_le_bytes([bytes[10], bytes[11]]), 0);
        assert_eq!(decode(&bytes).unwrap(), map);
    }

    #[test]
    fn missing_terrain_authoring_normalises_to_empty_when_flagged() {
        let mut map = sample_map();
        let mut second = map.layers[1].clone();
        second.authoring = None;
        map.layers.push(second);
        map.layers[0].authoring = Some(TerrainAuthoring::default());
        let decoded = decode(&encode(&map)).unwrap();
        assert_eq!(decoded.layers[0].authoring, None);
        assert_eq!(
            decoded.layers[2].authoring,
            Some(TerrainAuthoring::default())
        );
        assert_eq!(decoded.layers[1], map.layers[1]);
    }

    #[test]
    fn every_truncation_is_an_error() {
        let mut map = TilemapData {
            layers: vec![tile_layer(&[(0, 0, TileRef::new(1))])],
        };
        let mut terrain = tile_layer(&[(1, 1, TileRef::new(2))]);
        terrain.kind = LayerKind::Terrain;
        let mut authoring = TerrainAuthoring::default();
        authoring.terrain.set(CellCoord::new(1, 1), 1);
        authoring
            .overrides
            .insert(CellCoord::new(1, 1), TileRef::new(2));
        authoring
            .links
            .set(CellCoord::new(1, 1), Links::EMPTY.with_path());
        terrain.authoring = Some(authoring);
        map.layers.push(terrain);
        let bytes = encode(&map);
        for len in 0..bytes.len() {
            assert!(decode(&bytes[..len]).is_err(), "prefix {len} decoded");
        }
    }

    #[test]
    fn rejects_bad_header() {
        let bytes = encode(&TilemapData::default());

        let mut bad = bytes.clone();
        bad[0] = b'X';
        assert_eq!(decode(&bad), Err(SidecarError::BadMagic));

        let mut newer = bytes.clone();
        newer[8..10].copy_from_slice(&3u16.to_le_bytes());
        assert_eq!(decode(&newer), Err(SidecarError::UnsupportedVersion(3)));

        let mut zero = bytes.clone();
        zero[8..10].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(decode(&zero), Err(SidecarError::UnsupportedVersion(0)));

        let mut flags = bytes.clone();
        flags[10] = 0b10;
        assert_eq!(decode(&flags), Err(SidecarError::ReservedBits));

        let mut trailing = bytes;
        trailing.push(0);
        assert_eq!(decode(&trailing), Err(SidecarError::TrailingBytes));
    }

    #[test]
    fn rejects_reserved_layer_bytes_and_unknown_kind() {
        let map = TilemapData {
            layers: vec![tile_layer(&[])],
        };
        let bytes = encode(&map);
        let mut kind = bytes.clone();
        kind[16] = 7;
        assert_eq!(decode(&kind), Err(SidecarError::UnknownLayerKind(7)));
        let mut reserved = bytes;
        reserved[18] = 1;
        assert_eq!(decode(&reserved), Err(SidecarError::ReservedBits));
    }

    #[test]
    fn huge_declared_counts_fail_without_allocating() {
        let mut bytes = encode(&TilemapData::default());
        bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&bytes), Err(SidecarError::Truncated));

        let mut bytes = encode(&TilemapData {
            layers: vec![tile_layer(&[])],
        });
        bytes[20..24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(decode(&bytes), Err(SidecarError::Truncated));
    }

    #[test]
    fn rejects_out_of_order_and_duplicate_chunks() {
        let map = TilemapData {
            layers: vec![tile_layer(&[
                (0, 0, TileRef::new(1)),
                (32, 0, TileRef::new(2)),
            ])],
        };
        let bytes = encode(&map);
        let first = 24..24 + CHUNK_RECORD;
        let second = 24 + CHUNK_RECORD..24 + 2 * CHUNK_RECORD;

        let mut swapped = bytes[..24].to_vec();
        swapped.extend_from_slice(&bytes[second.clone()]);
        swapped.extend_from_slice(&bytes[first.clone()]);
        assert_eq!(decode(&swapped), Err(SidecarError::Unordered));

        let mut duplicated = bytes[..24].to_vec();
        duplicated.extend_from_slice(&bytes[first.clone()]);
        duplicated.extend_from_slice(&bytes[first]);
        assert_eq!(decode(&duplicated), Err(SidecarError::Unordered));
    }

    #[test]
    fn rejects_invalid_cells() {
        let map = TilemapData {
            layers: vec![tile_layer(&[(0, 0, TileRef::new(1))])],
        };
        let bytes = encode(&map);
        let cell = 32;

        let mut orientation = bytes.clone();
        orientation[cell + 2] = 0x80;
        assert_eq!(decode(&orientation), Err(SidecarError::InvalidCell));

        let mut empty_oriented = bytes.clone();
        empty_oriented[cell + 3 + 2] = TileRef::FLIP_X;
        assert_eq!(decode(&empty_oriented), Err(SidecarError::InvalidCell));

        let mut empty_chunk = bytes;
        empty_chunk[cell..cell + 2].copy_from_slice(&TileRef::EMPTY_INDEX.to_le_bytes());
        assert_eq!(decode(&empty_chunk), Err(SidecarError::EmptyChunk));
    }

    #[test]
    fn rejects_empty_override() {
        let mut terrain = tile_layer(&[]);
        terrain.kind = LayerKind::Terrain;
        let mut authoring = TerrainAuthoring::default();
        authoring
            .overrides
            .insert(CellCoord::new(0, 0), TileRef::new(1));
        terrain.authoring = Some(authoring);
        let mut bytes = encode(&TilemapData {
            layers: vec![terrain],
        });
        let index = bytes.len() - 4 - 3;
        bytes[index..index + 2].copy_from_slice(&TileRef::EMPTY_INDEX.to_le_bytes());
        assert_eq!(decode(&bytes), Err(SidecarError::InvalidCell));
    }

    #[test]
    fn golden_single_cell_map() {
        let map = TilemapData {
            layers: vec![tile_layer(&[(0, 0, TileRef::new(5).with_flip_x(true))])],
        };

        let header = [2, 0, 0, 0, 1, 0, 0, 0];
        let layer_header = [0, 0, 0, 0];
        let chunk_count = [1, 0, 0, 0];
        let chunk_coord = [0; 8];
        let mut expected = Vec::new();
        expected.extend_from_slice(b"JDTILES\0");
        expected.extend_from_slice(&header);
        expected.extend_from_slice(&layer_header);
        expected.extend_from_slice(&chunk_count);
        expected.extend_from_slice(&chunk_coord);
        expected.extend_from_slice(&[5, 0, TileRef::FLIP_X]);
        for _ in 1..CHUNK_AREA {
            expected.extend_from_slice(&[0xFF, 0xFF, 0]);
        }

        assert_eq!(encode(&map), expected);
        assert_eq!(decode(&expected).unwrap(), map);
    }

    #[test]
    fn rejects_chunk_coords_that_overflow_cells() {
        let map = TilemapData {
            layers: vec![tile_layer(&[(0, 0, TileRef::new(1))])],
        };
        let mut bytes = encode(&map);
        bytes[24..28].copy_from_slice(&0x4000_0000i32.to_le_bytes());
        assert_eq!(decode(&bytes), Err(SidecarError::InvalidCell));
    }

    #[test]
    fn accepts_extreme_in_range_chunk() {
        let corner = CellCoord::new(i32::MAX, i32::MIN);
        let map = TilemapData {
            layers: vec![tile_layer(&[(corner.0.x, corner.0.y, TileRef::new(1))])],
        };
        let bytes = encode(&map);
        assert_eq!(decode(&bytes).unwrap(), map);
    }

    #[test]
    fn rejects_authoring_flag_without_terrain_layer() {
        let mut bytes = encode(&TilemapData {
            layers: vec![tile_layer(&[])],
        });
        bytes[10] = 1;
        assert_eq!(decode(&bytes), Err(SidecarError::ReservedBits));
        let mut bytes = encode(&TilemapData::default());
        bytes[10] = 1;
        assert_eq!(decode(&bytes), Err(SidecarError::ReservedBits));
    }

    #[test]
    fn decoded_layers_have_no_dirty_chunks() {
        let mut decoded = decode(&encode(&sample_map())).unwrap();
        for layer in &mut decoded.layers {
            assert!(layer.tiles.take_dirty().is_empty());
            if let Some(authoring) = &mut layer.authoring {
                assert!(authoring.terrain.take_dirty().is_empty());
                assert!(authoring.links.take_dirty().is_empty());
            }
        }
    }

    /// One terrain layer with one tile, one terrain cell and the given links.
    fn linked_layer(links: &[(i32, i32, Links)]) -> TilemapData {
        let mut terrain = tile_layer(&[(0, 0, TileRef::new(5))]);
        terrain.kind = LayerKind::Terrain;
        let mut authoring = TerrainAuthoring::default();
        authoring.terrain.set(CellCoord::new(0, 0), 2);
        for &(x, y, l) in links {
            authoring.terrain.set(CellCoord::new(x, y), 2);
            authoring.links.set(CellCoord::new(x, y), l);
        }
        terrain.authoring = Some(authoring);
        TilemapData {
            layers: vec![terrain],
        }
    }

    #[test]
    fn golden_terrain_layer_with_links() {
        let path = Links::EMPTY.with_path().with(Peer::Bottom);
        let map = linked_layer(&[(-1, 0, path)]);

        let mut expected = Vec::new();
        expected.extend_from_slice(b"JDTILES\0");
        expected.extend_from_slice(&[2, 0, 1, 0, 1, 0, 0, 0]);
        expected.extend_from_slice(&[1, 0, 0, 0]);
        expected.extend_from_slice(&[1, 0, 0, 0]);
        expected.extend_from_slice(&[0; 8]);
        expected.extend_from_slice(&[5, 0, 0]);
        for _ in 1..CHUNK_AREA {
            expected.extend_from_slice(&[0xFF, 0xFF, 0]);
        }
        expected.extend_from_slice(&[2, 0, 0, 0]);
        expected.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]);
        let mut west = [0; CHUNK_AREA];
        west[31] = 2;
        expected.extend_from_slice(&west);
        expected.extend_from_slice(&[0; 8]);
        expected.push(2);
        expected.extend_from_slice(&[0; CHUNK_AREA - 1]);
        expected.extend_from_slice(&[0, 0, 0, 0]);
        expected.extend_from_slice(&[1, 0, 0, 0]);
        expected.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF, 0, 0, 0, 0]);
        let mut links = [0; CHUNK_AREA];
        links[31] = Links::PATH | Links::BOTTOM;
        expected.extend_from_slice(&links);

        assert_eq!(encode(&map), expected);
        assert_eq!(decode(&expected).unwrap(), map);
    }

    #[test]
    fn version_1_decodes_with_empty_links_and_re_encodes_as_version_2() {
        let map = linked_layer(&[]);
        let v2 = encode(&map);
        let mut v1 = v2[..v2.len() - 4].to_vec();
        v1[8..10].copy_from_slice(&1u16.to_le_bytes());
        let decoded = decode(&v1).unwrap();
        assert_eq!(decoded, map);
        assert_eq!(encode(&decoded), v2);
        assert_eq!(decode(&v2[..]).unwrap(), map);
        let mut v1_with_links = v2.clone();
        v1_with_links[8..10].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(decode(&v1_with_links), Err(SidecarError::TrailingBytes));
    }

    #[test]
    fn rejects_reserved_empty_and_unordered_link_chunks() {
        let map = linked_layer(&[(0, 0, Links::EMPTY.with_path())]);
        let bytes = encode(&map);
        let cell = bytes.len() - CHUNK_AREA;

        let mut reserved = bytes.clone();
        reserved[cell] = 0x20;
        assert_eq!(decode(&reserved), Err(SidecarError::ReservedBits));

        let mut empty = bytes.clone();
        empty[cell] = 0;
        assert_eq!(decode(&empty), Err(SidecarError::EmptyChunk));

        let two = linked_layer(&[
            (0, 0, Links::EMPTY.with_path()),
            (32, 0, Links::EMPTY.with_path()),
        ]);
        let bytes = encode(&two);
        let record = 8 + CHUNK_AREA;
        let start = bytes.len() - 2 * record;
        let mut swapped = bytes[..start].to_vec();
        swapped.extend_from_slice(&bytes[start + record..]);
        swapped.extend_from_slice(&bytes[start..start + record]);
        assert_eq!(decode(&swapped), Err(SidecarError::Unordered));
    }

    #[test]
    fn rejects_links_without_path_bit_or_terrain() {
        let map = linked_layer(&[(1, 0, Links::EMPTY.with_path())]);
        let bytes = encode(&map);
        let link = bytes.len() - CHUNK_AREA + 1;

        let mut side_only = bytes.clone();
        side_only[link] = Links::BOTTOM;
        assert_eq!(decode(&side_only), Err(SidecarError::InvalidLinks));

        let terrain_cells = 16 + 4 + 4 + CHUNK_RECORD + 4 + 8;
        let mut bare = bytes;
        assert_eq!(bare[terrain_cells + 1], 2);
        bare[terrain_cells + 1] = 0;
        assert_eq!(decode(&bare), Err(SidecarError::InvalidLinks));
    }

    #[test]
    fn encode_prunes_invalid_links() {
        let map = linked_layer(&[(1, 0, Links::EMPTY.with_path())]);
        let mut stale = map.clone();
        let authoring = stale.layers[0].authoring.as_mut().unwrap();
        authoring
            .links
            .set(CellCoord::new(-5, 3), Links::EMPTY.with_path());
        authoring
            .links
            .set(CellCoord::new(0, 0), Links::EMPTY.with(Peer::Top));
        assert_eq!(encode(&stale), encode(&map));
        assert_eq!(decode(&encode(&stale)).unwrap(), map);
    }
}
