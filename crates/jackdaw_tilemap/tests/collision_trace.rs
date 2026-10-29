use std::collections::BTreeMap;

use glam::UVec2;
use jackdaw_tilemap::{
    GridLayout, RgbaImage, SheetInput, SolidMask, TerrainAnalysis, TerrainId, TileAddress,
    TileKind, TileShape, TilesetProposal, find_terrains, fit_layouts, solid_mask, trace_proposal,
};

struct Sheet {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Sheet {
    fn packed() -> Self {
        let path = format!(
            "{}/tests/data/punyworld/punyworld-overworld-tileset.png",
            env!("CARGO_MANIFEST_DIR")
        );
        let image = image::open(&path).expect("fixture decodes").to_rgba8();
        Self {
            width: image.width(),
            height: image.height(),
            pixels: image.into_raw(),
        }
    }

    fn input(&self) -> SheetInput<'_> {
        SheetInput {
            name: "overworld.png",
            image: RgbaImage::new(self.width, self.height, &self.pixels).expect("sized"),
            layout: GridLayout {
                tile: UVec2::splat(16),
                margin: UVec2::ZERO,
                spacing: UVec2::ZERO,
                columns: 27,
                rows: 38,
            },
        }
    }
}

/// Dirt path tiles: the top end of a vertical run and a four-way crossing.
const TOP_CAP: u32 = 3;
const CROSSING: u32 = 32;
/// Part of an object that terrain finding labels as a terrain tile.
const PROP: u32 = 723;
/// A small sprite on a transparent background.
const SPRITE: u32 = 818;
/// Largest difference between a shape's area and its solid pixels' area.
const AREA_TOLERANCE: f32 = 0.05;

fn at(tile: u32) -> TileAddress {
    TileAddress { sheet: 0, tile }
}

/// Vertices of the pixel-precise outline: lattice points where the four
/// pixels around them make one corner, or two at a diagonal.
fn pixel_corners(mask: &SolidMask) -> usize {
    let size = mask.size().as_ivec2();
    let points = (0..=size.y).flat_map(|y| (0..=size.x).map(move |x| (x, y)));
    points
        .map(|(x, y)| {
            let [a, b, c, d] =
                [(-1, -1), (0, -1), (-1, 0), (0, 0)].map(|(dx, dy)| mask.get(x + dx, y + dy));
            match [a, b, c, d].iter().filter(|s| **s).count() {
                1 | 3 => 1,
                2 if a == d => 2,
                _ => 0,
            }
        })
        .sum()
}

fn snapped_to_eighths(shape: &TileShape) -> bool {
    let on = |v: f32| (v * 8.0 - (v * 8.0).round()).abs() < 1e-5;
    shape.polygons.iter().flatten().all(|v| on(v.x) && on(v.y))
}

/// The packed sheet traced with the dirt and sand ground Solid (for this
/// test only) and the two decor tiles marked Solid.
struct Traced<'a> {
    sheet: &'a Sheet,
    analysis: TerrainAnalysis,
    proposal: TilesetProposal,
    ground: TerrainId,
    shapes: BTreeMap<TileAddress, TileShape>,
}

impl<'a> Traced<'a> {
    fn new(sheet: &'a Sheet) -> Self {
        let input = sheet.input();
        let analysis = find_terrains(&[input]);
        let proposal = fit_layouts(&analysis, &[input]).expect("matching inputs");
        let ground = analysis
            .tile(at(TOP_CAP))
            .expect("ground path tile")
            .terrains
            .centre();
        let decor = [at(PROP), at(SPRITE)];
        let shapes = trace_proposal(&analysis, &[input], &proposal, |id| id == ground, &decor)
            .expect("matching inputs");
        Self {
            sheet,
            analysis,
            proposal,
            ground,
            shapes,
        }
    }

    /// The traced shape and its solid mask, read as the trace did.
    fn shape(&self, tile: u32) -> (&TileShape, SolidMask) {
        let mut fitted = self.proposal.sets.iter().flat_map(|s| &s.tiles);
        let peers = fitted.find(|t| t.address == at(tile)).map(|t| &t.terrains);
        let decor = [PROP, SPRITE].contains(&tile);
        let input = [self.sheet.input()];
        let ground = self.ground;
        let mask = solid_mask(
            &self.analysis,
            &input,
            at(tile),
            peers,
            |id| id == ground,
            decor,
        );
        let shape = self.shapes.get(&at(tile));
        (
            shape.unwrap_or_else(|| panic!("tile {tile} traced")),
            (mask.expect("matching inputs")).unwrap_or_else(|| panic!("tile {tile} analysed")),
        )
    }
}

#[expect(clippy::print_stdout, reason = "reports the traced shapes")]
fn report(tile: u32, shape: &TileShape, mask: &SolidMask) {
    println!(
        "tile {tile}: {} polygons, {} vertices (pixel outline {}), area {} (pixels {}): {:?}",
        shape.polygons.len(),
        shape.vertex_count(),
        pixel_corners(mask),
        shape.area(),
        mask.count() as f32 / 256.0,
        shape.polygons
    );
}

fn check_simple(tile: u32, shape: &TileShape, mask: &SolidMask) {
    report(tile, shape, mask);
    assert_eq!(shape.polygons.len(), 1, "tile {tile}");
    assert!(!shape.half_tile, "tile {tile}");
    assert!(snapped_to_eighths(shape), "tile {tile}");
    let pixel_area = mask.count() as f32 / 256.0;
    assert!(
        (shape.area() - pixel_area).abs() <= AREA_TOLERANCE,
        "tile {tile}"
    );
    assert!(
        shape.vertex_count() * 2 <= pixel_corners(mask),
        "tile {tile}"
    );
}

#[test]
fn overworld_dirt_path_traces_to_snapped_simple_shapes() {
    let sheet = Sheet::packed();
    let traced = Traced::new(&sheet);
    for tile in [TOP_CAP, CROSSING] {
        let (shape, mask) = traced.shape(tile);
        check_simple(tile, shape, &mask);
    }
    // The cap's ragged hand-drawn end simplifies from a 50 vertex pixel
    // outline to 15; the crossing's grass corners leave it 8.
    assert_eq!(traced.shape(TOP_CAP).0.vertex_count(), 15);
    let (crossing, _) = traced.shape(CROSSING);
    assert_eq!(crossing.vertex_count(), 8);
    assert_eq!(crossing.area(), 0.921875);
}

#[test]
fn overworld_decor_marked_solid_traces_its_opaque_pixels() {
    let sheet = Sheet::packed();
    let traced = Traced::new(&sheet);
    let kind = |tile| traced.analysis.tile(at(tile)).map(|l| l.kind);
    assert_eq!(kind(PROP), Some(TileKind::Terrain));
    assert_eq!(kind(SPRITE), Some(TileKind::Decor));
    // Pixel-precise outlines: 16 vertices for the prop (8 at a 2 px
    // simplification tolerance), 32 for the sprite (10 at 2 px).
    for (tile, outline, most) in [(PROP, 16, 8), (SPRITE, 32, 10)] {
        let (shape, mask) = traced.shape(tile);
        check_simple(tile, shape, &mask);
        assert_eq!(pixel_corners(&mask), outline, "tile {tile}");
        assert!(shape.vertex_count() <= most, "tile {tile}");
    }
}

#[test]
fn overworld_tiles_without_solid_pixels_have_no_shape() {
    let sheet = Sheet::packed();
    let traced = Traced::new(&sheet);
    // Water edged with grass stays passable, as does unmarked decor.
    for tile in [278, 331, 811] {
        assert!(!traced.shapes.contains_key(&at(tile)), "tile {tile}");
    }
}

#[test]
fn overworld_trace_is_deterministic() {
    let sheet = Sheet::packed();
    assert_eq!(Traced::new(&sheet).shapes, Traced::new(&sheet).shapes);
}
