use glam::UVec2;
use jackdaw_tilemap::{
    GridLayout, LOOSE_FRAME_MATCH, RgbaImage, detect_grid, empty_tiles, pair_animations,
};

/// Packed sheets of seamless terrain have cell boundaries only modestly
/// sharper than the art inside cells. A gutter sheet scores its share of
/// content lines that are not uniform, which blank cells pull down from 1
/// (to near 0.72 here). Either way the wrong sizes score below half of that.
const PACKED_MIN_CONFIDENCE: f32 = 0.3;
const SPACED_MIN_CONFIDENCE: f32 = 0.7;
/// Rows of the overworld sheet down to its last art; the rows below are
/// blank.
const ROWS: u32 = 38;
const EMPTY_TILES: usize = 331;

fn load() -> (u32, u32, Vec<u8>) {
    let path = format!(
        "{}/tests/data/punyworld/punyworld-overworld-tileset.png",
        env!("CARGO_MANIFEST_DIR")
    );
    let image = image::open(&path).expect("fixture decodes").to_rgba8();
    (image.width(), image.height(), image.into_raw())
}

/// The sheet re-packed with a transparent pixel between neighbouring tiles.
fn load_spaced() -> (u32, u32, Vec<u8>) {
    let (w, h, packed) = load();
    let (sw, sh) = (w / 16 * 17 - 1, h / 16 * 17 - 1);
    let mut spaced = vec![0; (sw * sh * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = (x + x / 16, y + y / 16);
            let from = ((y * w + x) * 4) as usize;
            let to = ((sy * sw + sx) * 4) as usize;
            spaced[to..to + 4].copy_from_slice(&packed[from..from + 4]);
        }
    }
    (sw, sh, spaced)
}

fn grid(tile: u32, spacing: u32, columns: u32, rows: u32) -> GridLayout {
    GridLayout {
        tile: UVec2::splat(tile),
        margin: UVec2::ZERO,
        spacing: UVec2::splat(spacing),
        columns,
        rows,
    }
}

#[test]
fn overworld_packed_is_16px_27_columns_down_to_its_last_art() {
    let (w, h, pixels) = load();
    assert_eq!((w, h), (432, 1040));
    let image = RgbaImage::new(w, h, &pixels).expect("sized");
    let found = detect_grid(&image).expect("detected");
    assert_eq!(found.layout, grid(16, 0, 27, ROWS));
    assert!(
        found.confidence > PACKED_MIN_CONFIDENCE,
        "{}",
        found.confidence
    );
    let empty = empty_tiles(&image, &found.layout);
    assert_eq!(empty.iter().filter(|e| **e).count(), EMPTY_TILES);
}

#[test]
fn overworld_spaced_is_16px_with_1px_spacing() {
    let (w, h, pixels) = load_spaced();
    assert_eq!((w, h), (458, 1104));
    let image = RgbaImage::new(w, h, &pixels).expect("sized");
    let found = detect_grid(&image).expect("detected");
    assert_eq!(found.layout, grid(16, 1, 27, ROWS));
    assert!(
        found.confidence > SPACED_MIN_CONFIDENCE,
        "{}",
        found.confidence
    );
    let empty = empty_tiles(&image, &found.layout);
    assert_eq!(empty.iter().filter(|e| **e).count(), EMPTY_TILES);
}

#[test]
fn overworld_sheets_are_not_animations_of_each_other() {
    let (pw, ph, packed) = load();
    let (sw, sh, spaced) = load_spaced();
    let images = [
        RgbaImage::new(pw, ph, &packed).expect("sized"),
        RgbaImage::new(sw, sh, &spaced).expect("sized"),
    ];
    let layouts = [grid(16, 0, 27, ROWS), grid(16, 1, 27, ROWS)];
    assert!(pair_animations(&images, &layouts, LOOSE_FRAME_MATCH).is_empty());
}
