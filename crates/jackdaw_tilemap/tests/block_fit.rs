//! Block fitting over the overworld sheet, checked against the corner
//! terrains its own tileset file gives each tile.

use glam::UVec2;
use jackdaw_tilemap::{
    GridLayout, LayoutTemplate, Peer, Placement, RgbaImage, SheetInput, TerrainLook, fit_block,
};

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/punyworld");
const COLUMNS: u32 = 27;
const TILE: u32 = 16;
const GRASS: u32 = 1;
const PLAIN_GRASS: u32 = 0;
const LAYOUT: (LayoutTemplate, Placement) =
    (LayoutTemplate::Minimal3x3Inner, Placement::BetweenCells);

struct Sheet {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Sheet {
    fn load() -> Self {
        let path = format!("{DIR}/punyworld-overworld-tileset.png");
        let image = image::open(&path).expect("fixture decodes").to_rgba8();
        Self {
            width: image.width(),
            height: image.height(),
            pixels: image.into_raw(),
        }
    }

    fn input(&self) -> SheetInput<'_> {
        SheetInput {
            name: "overworld",
            image: RgbaImage::new(self.width, self.height, &self.pixels).expect("sized"),
            layout: GridLayout {
                tile: UVec2::splat(TILE),
                margin: UVec2::ZERO,
                spacing: UVec2::ZERO,
                columns: self.width / TILE,
                rows: self.height / TILE,
            },
        }
    }

    /// A sheet `columns` tiles wide of the overworld tiles `tiles`, row by row.
    fn of_tiles(&self, tiles: &[u32], columns: u32) -> Self {
        let rows = (tiles.len() as u32).div_ceil(columns);
        let (width, height) = (columns * TILE, rows * TILE);
        let mut pixels = vec![0; (width * height * 4) as usize];
        for (i, tile) in tiles.iter().enumerate() {
            let from = UVec2::new(tile % COLUMNS, tile / COLUMNS) * TILE;
            let to = UVec2::new(i as u32 % columns, i as u32 / columns) * TILE;
            for y in 0..TILE {
                let row = |at: UVec2, w: u32| (((at.y + y) * w + at.x) * 4) as usize;
                let (src, dst) = (row(from, self.width), row(to, width));
                let len = (TILE * 4) as usize;
                pixels[dst..dst + len].copy_from_slice(&self.pixels[src..src + len]);
            }
        }
        Self {
            width,
            height,
            pixels,
        }
    }
}

/// The corner colours of `tile` in TL, TR, BL, BR order, from the tileset
/// file's corner set, if it is in it.
fn wang_corners(tile: u32) -> Option<[u32; 4]> {
    let tsx = std::fs::read_to_string(format!("{DIR}/punyworld-overworld-tiles.tsx"))
        .expect("tileset file");
    let key = format!("<wangtile tileid=\"{tile}\" wangid=\"");
    let start = tsx.find(&key)? + key.len();
    let ids: Vec<u32> = (tsx[start..]
        .split('"')
        .next()
        .unwrap_or_default()
        .split(','))
    .map(|n| n.parse().expect("a number"))
    .collect();
    Some([ids[7], ids[1], ids[5], ids[3]])
}

/// The overworld tiles of the 5x3 ground block at `origin`, shuffled when
/// `shuffle` is set, then the plain tiles `plain` and of grass.
fn block_tiles(origin: UVec2, plain: u32, shuffle: bool) -> Vec<u32> {
    let size = LAYOUT.0.shape();
    let mut tiles: Vec<u32> = (0..size.x * size.y)
        .map(|i| (origin.y + i / size.x) * COLUMNS + origin.x + i % size.x)
        .collect();
    if shuffle {
        let mut seed = 0x9e37_79b9_u32;
        for i in (1..tiles.len()).rev() {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            tiles.swap(i, (seed >> 8) as usize % (i + 1));
        }
    }
    tiles.extend([plain, PLAIN_GRASS]);
    tiles
}

struct Outcome {
    fitted_right: usize,
    canonical_right: usize,
    unsure: usize,
}

/// Fits the ground block of `inner` over grass laid out as `tiles`,
/// counting template tiles whose cell has their corner colours.
fn fit_ground(tiles: &[u32], inner: u32) -> Outcome {
    let sheet = Sheet::load().of_tiles(tiles, LAYOUT.0.shape().x);
    let input = sheet.input();
    let size = LAYOUT.0.shape();
    let look = |cell| TerrainLook::of_tile(&input, cell).expect("a tile");
    let looks = (&look(15), &look(16));
    let fit = fit_block(&input, (UVec2::ZERO, size), LAYOUT, looks).expect("fits");
    let shapes = LAYOUT.0.placed_tiles(LAYOUT.1, 2, 1);
    let right = |order: &[Option<u32>]| {
        (order.iter().zip(&shapes))
            .filter(|(cell, shape)| {
                let Some(cell) = cell else { return false };
                let wang = wang_corners(tiles[*cell as usize]);
                let colour = |p: Peer| if shape.get(p) == 2 { inner } else { GRASS };
                Some(Peer::CORNERS.map(colour)) == wang
            })
            .count()
    };
    Outcome {
        fitted_right: right(&fit.order),
        canonical_right: right(&LAYOUT.0.canonical_order(size)),
        unsure: fit.unsure.len(),
    }
}

const DIRT: (UVec2, u32, u32) = (UVec2::new(10, 0), 38, 2);
const WATER: (UVec2, u32, u32) = (UVec2::new(7, 10), 305, 7);

#[test]
fn the_ground_blocks_fit_every_tile_as_drawn() {
    for (origin, plain, inner) in [DIRT, WATER] {
        let fit = fit_ground(&block_tiles(origin, plain, false), inner);
        assert_eq!(fit.fitted_right, 13, "terrain {inner}");
        assert_eq!(fit.canonical_right, 13, "terrain {inner}");
        assert_eq!(fit.unsure, 0, "terrain {inner}");
    }
}

#[test]
fn the_ground_blocks_fit_every_tile_when_shuffled() {
    for (origin, plain, inner) in [DIRT, WATER] {
        let fit = fit_ground(&block_tiles(origin, plain, true), inner);
        assert_eq!(fit.fitted_right, 13, "terrain {inner}");
        assert!(fit.canonical_right < 13, "terrain {inner} is shuffled");
        assert_eq!(fit.unsure, 0, "terrain {inner}");
    }
}
