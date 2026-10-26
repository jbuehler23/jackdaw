use glam::UVec2;
use jackdaw_tilemap::{GridLayout, RgbaImage, detect_grid, empty_tiles};

/// Packed sheets of seamless terrain have cell boundaries only modestly
/// sharper than the art inside cells, so a correct packed detection scores
/// near 0.35; gutter sheets score near 1. Either way the wrong sizes score
/// below half of that.
const PACKED_MIN_CONFIDENCE: f32 = 0.3;
const SPACED_MIN_CONFIDENCE: f32 = 0.8;

fn load(name: &str) -> (u32, u32, Vec<u8>) {
    let path = format!("{}/tests/data/tiny_farm/{name}", env!("CARGO_MANIFEST_DIR"));
    let image = image::open(&path).expect("fixture decodes").to_rgba8();
    (image.width(), image.height(), image.into_raw())
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
fn tiny_farm_packed_is_16px_12_by_11() {
    let (w, h, pixels) = load("tilemap_packed.png");
    assert_eq!((w, h), (192, 176));
    let image = RgbaImage::new(w, h, &pixels).expect("sized");
    let found = detect_grid(&image).expect("detected");
    assert_eq!(found.layout, grid(16, 0, 12, 11));
    assert!(
        found.confidence > PACKED_MIN_CONFIDENCE,
        "{}",
        found.confidence
    );
    assert!(empty_tiles(&image, &found.layout).iter().all(|e| !e));
}

#[test]
fn tiny_farm_spaced_is_16px_with_1px_spacing() {
    let (w, h, pixels) = load("tilemap.png");
    assert_eq!((w, h), (203, 186));
    let image = RgbaImage::new(w, h, &pixels).expect("sized");
    let found = detect_grid(&image).expect("detected");
    assert_eq!(found.layout, grid(16, 1, 12, 11));
    assert!(
        found.confidence > SPACED_MIN_CONFIDENCE,
        "{}",
        found.confidence
    );
    assert!(empty_tiles(&image, &found.layout).iter().all(|e| !e));
}
