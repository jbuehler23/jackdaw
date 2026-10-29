//! A tilemap layer as Bevy tilemap chunks: one 32 x 32 block of cells per
//! chunk, laid out the same way wherever a map is drawn.

use std::collections::HashSet;

use bevy::prelude::*;
use bevy::sprite_render::{
    AlphaMode2d, TileData, TileOrientation, TilemapChunk, TilemapChunkTileData,
};
use jackdaw_tilemap::{CHUNK_AREA, CHUNK_SIZE, ChunkCoord, TileRef};

use crate::Tileset;

/// One layer's 32 x 32 block of the tilemap it is a child of, as drawn.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TilemapChunkOf {
    /// Index into the map's layers.
    pub layer: usize,
    pub chunk: ChunkCoord,
}

/// The tile a cell's [`TileRef`] draws, or `None` for an empty cell.
///
/// Both orient the tile image the same way: transpose across its top-left
/// diagonal, then mirror, so the bits map one to one.
pub fn tile_data(tile: TileRef) -> Option<TileData> {
    (!tile.is_empty()).then(|| TileData {
        tileset_index: tile.index(),
        visible: tile.visible(),
        orientation: TileOrientation::from_bools(tile.flip_x(), tile.flip_y(), tile.transpose()),
        ..default()
    })
}

/// The z of layer `layer` under its map. Layers stack one whole unit apart,
/// so y-sorted entities, which move less than half a unit, never cross from
/// one layer to the next.
pub fn layer_z(layer: usize) -> f32 {
    layer as f32
}

/// Where the chunk at `coord` of layer `layer` sits under its map: the chunk
/// mesh is centred on its entity, and cell (0,0) has its min corner at the
/// map's origin.
pub fn chunk_transform(coord: ChunkCoord, tile_size: UVec2, layer: usize) -> Transform {
    let centre = (coord.0 * CHUNK_SIZE).as_vec2() + CHUNK_SIZE as f32 / 2.0;
    Transform::from_translation((centre * tile_size.as_vec2()).extend(layer_z(layer)))
}

/// What a layer's chunks need from its tileset.
#[derive(Clone, Debug)]
pub struct ChunkDraw {
    /// The packed array image.
    pub image: Handle<Image>,
    /// The packed layers the tileset has a tile in; a cell naming another
    /// draws nothing.
    pub known: HashSet<u16>,
}

impl ChunkDraw {
    pub fn new(tileset: &Tileset, image: Handle<Image>) -> Self {
        let known = (tileset.sheets.iter())
            .flat_map(|sheet| sheet.first_layer..sheet.first_layer + sheet.columns * sheet.rows)
            .filter_map(|layer| u16::try_from(layer).ok())
            .collect();
        Self { image, known }
    }

    /// The chunk component for tiles drawn `tile_size` world units wide.
    pub fn chunk(&self, tile_size: UVec2) -> TilemapChunk {
        TilemapChunk {
            chunk_size: UVec2::splat(CHUNK_SIZE as u32),
            tile_display_size: tile_size,
            tileset: self.image.clone(),
            alpha_mode: AlphaMode2d::Blend,
        }
    }

    /// The 1024 drawn tiles of one chunk's cells.
    pub fn tiles(&self, cells: &[TileRef; CHUNK_AREA]) -> TilemapChunkTileData {
        TilemapChunkTileData(
            (cells.iter())
                .map(|&tile| tile_data(tile).filter(|_| self.known.contains(&tile.index())))
                .collect(),
        )
    }

    /// Everything the chunk at `coord` of layer `layer` spawns with.
    pub fn bundle(
        &self,
        coord: ChunkCoord,
        cells: &[TileRef; CHUNK_AREA],
        tile_size: UVec2,
        layer: usize,
    ) -> (TilemapChunk, TilemapChunkTileData, Transform) {
        (
            self.chunk(tile_size),
            self.tiles(cells),
            chunk_transform(coord, tile_size, layer),
        )
    }
}

impl Tileset {
    /// World size of one drawn tile: the tile size, or the map's cell size
    /// for a tileset that names none.
    pub fn display_size(&self, cell_size: UVec2) -> UVec2 {
        match self.tile_size.min_element() {
            0 => cell_size,
            _ => self.tile_size,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: usize = 3;
    type Pixels = [[u8; N]; N];

    fn source() -> Pixels {
        [[0, 1, 2], [3, 4, 5], [6, 7, 8]]
    }

    /// The tile as the kernel defines its bits, rows top down: transpose,
    /// then flip x, then flip y.
    fn kernel_oriented(tile: TileRef) -> Pixels {
        let image = source();
        let mut out = [[0; N]; N];
        for (y, row) in image.iter().enumerate() {
            for (x, &pixel) in row.iter().enumerate() {
                let (mut tx, mut ty) = if tile.transpose() { (y, x) } else { (x, y) };
                if tile.flip_x() {
                    tx = N - 1 - tx;
                }
                if tile.flip_y() {
                    ty = N - 1 - ty;
                }
                out[ty][tx] = pixel;
            }
        }
        out
    }

    /// The tile as the chunk shader samples it, in uv (y down): mirror h,
    /// then v, then swap the coordinates.
    fn shader_oriented(orientation: TileOrientation) -> Pixels {
        let image = source();
        let mut out = [[0; N]; N];
        for (y, row) in out.iter_mut().enumerate() {
            for (x, pixel) in row.iter_mut().enumerate() {
                let (mut u, mut v) = (x, y);
                if orientation.mirror_h() {
                    u = N - 1 - u;
                }
                if orientation.mirror_v() {
                    v = N - 1 - v;
                }
                if orientation.mirror_d() {
                    (u, v) = (v, u);
                }
                *pixel = image[v][u];
            }
        }
        out
    }

    #[test]
    fn every_orientation_draws_the_tile_the_kernel_means() {
        let mut seen = HashSet::new();
        for bits in 0..8u8 {
            let tile = TileRef::from_raw(5, bits);
            let data = tile_data(tile).expect("a filled cell draws");
            assert_eq!(data.tileset_index, 5);
            assert!(data.visible);
            assert_eq!(
                shader_oriented(data.orientation),
                kernel_oriented(tile),
                "bits {bits:03b} as {:?}",
                data.orientation
            );
            seen.insert(data.orientation);
        }
        assert_eq!(seen.len(), 8, "the eight orientations stay distinct");
    }

    #[test]
    fn hidden_and_empty_cells() {
        let hidden = tile_data(TileRef::new(2).with_visible(false)).expect("hidden still draws");
        assert!(!hidden.visible);
        assert!(tile_data(TileRef::EMPTY).is_none());
    }

    #[test]
    fn cell_zero_sits_at_the_map_origin() {
        let size = UVec2::new(16, 8);
        let at = chunk_transform(ChunkCoord(IVec2::new(-1, 2)), size, 3);
        let half = Vec2::new(16.0 * 16.0, 16.0 * 8.0);
        let min = at.translation.truncate() - half;
        assert_eq!(min, Vec2::new(-32.0 * 16.0, 64.0 * 8.0));
        assert_eq!(at.translation.z, 3.0);
    }

    #[test]
    fn a_chunk_holds_every_cell_and_drops_unknown_tiles() {
        let tileset = Tileset {
            sheets: vec![crate::TilesetSheet {
                columns: 2,
                rows: 2,
                ..default()
            }],
            ..default()
        };
        let draw = ChunkDraw::new(&tileset, Handle::default());
        let mut cells = [TileRef::EMPTY; CHUNK_AREA];
        cells[0] = TileRef::new(3);
        cells[1] = TileRef::new(4);
        let tiles = draw.tiles(&cells);
        assert_eq!(tiles.len(), CHUNK_AREA);
        assert_eq!(tiles[0].map(|t| t.tileset_index), Some(3));
        assert!(tiles[1].is_none(), "layer 4 is past the sheet");
        assert_eq!(tileset.display_size(UVec2::splat(8)), UVec2::splat(8));
    }
}
