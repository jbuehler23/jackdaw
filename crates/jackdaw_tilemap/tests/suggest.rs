//! Suggestions over the overworld sheet, with terrains named by icon tiles.

use std::collections::BTreeMap;

use glam::UVec2;
use jackdaw_tilemap::{
    GridLayout, LayoutTemplate, MIN_OBJECT, Placement, RgbaImage, SetupTile, SheetInput,
    Suggestion, SuggestionKind, TerrainId, TileAddress, TileTerrains, TilesetSetup, suggest_blocks,
    taken_tiles,
};

const COLUMNS: u32 = 27;
const TILE: u32 = 16;
const GRASS: (TerrainId, u32) = (1, 0);
const DIRT: (TerrainId, u32) = (2, 38);
const WATER: (TerrainId, u32) = (3, 305);
const DIRT_BLOCK: UVec2 = UVec2::new(10, 0);
const WATER_BLOCK: UVec2 = UVec2::new(7, 10);
const GROUND: UVec2 = UVec2::new(5, 3);

struct Sheet {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Sheet {
    fn load() -> Self {
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
            name: "overworld",
            image: RgbaImage::new(self.width, self.height, &self.pixels).expect("sized"),
            layout: GridLayout {
                tile: UVec2::splat(TILE),
                margin: UVec2::ZERO,
                spacing: UVec2::ZERO,
                columns: COLUMNS,
                rows: self.height / TILE,
            },
        }
    }
}

fn icons() -> Vec<(TerrainId, TileAddress)> {
    [GRASS, DIRT, WATER]
        .map(|(id, tile)| (id, TileAddress { sheet: 0, tile }))
        .to_vec()
}

fn setup() -> TilesetSetup {
    TilesetSetup {
        sheets: vec!["overworld".into()],
        ..Default::default()
    }
}

fn suggest(setup: &TilesetSetup) -> Vec<Suggestion> {
    suggest_with(setup, &icons())
}

fn suggest_with(setup: &TilesetSetup, icons: &[(TerrainId, TileAddress)]) -> Vec<Suggestion> {
    let sheet = Sheet::load();
    suggest_blocks(&[sheet.input()], setup, icons).expect("suggests")
}

fn objects(found: &[Suggestion]) -> Vec<&Suggestion> {
    (found.iter())
        .filter(|s| matches!(s.kind, SuggestionKind::Object { .. }))
        .collect()
}

fn pair(s: &Suggestion) -> (Option<TerrainId>, Option<TerrainId>) {
    match &s.kind {
        SuggestionKind::Block { inner, outer, .. } => (*inner, *outer),
        SuggestionKind::Object { .. } => (None, None),
    }
}

fn block(found: &[Suggestion], origin: UVec2) -> Option<&Suggestion> {
    found
        .iter()
        .find(|s| s.origin == origin && matches!(s.kind, SuggestionKind::Block { .. }))
}

fn overlaps(s: &Suggestion, origin: UVec2, size: UVec2) -> bool {
    let (lo, hi) = (s.origin.max(origin), (s.origin + s.size).min(origin + size));
    s.sheet == 0 && lo.cmplt(hi).all()
}

#[test]
fn the_dirt_and_water_blocks_are_suggested_with_their_terrains() {
    let found = suggest(&setup());
    for (origin, inner) in [(DIRT_BLOCK, DIRT.0), (WATER_BLOCK, WATER.0)] {
        let s = block(&found, origin).unwrap_or_else(|| panic!("a block at {origin}: {found:#?}"));
        assert_eq!(s.size, GROUND);
        let SuggestionKind::Block {
            layouts,
            inner: found,
            outer,
        } = &s.kind
        else {
            unreachable!()
        };
        assert_eq!(layouts[0].template, LayoutTemplate::Minimal3x3Inner);
        assert_eq!((*found, *outer), (Some(inner), Some(GRASS.0)), "{origin}");
    }
}

#[test]
fn objects_are_suggested_after_the_blocks() {
    let found = suggest(&setup());
    let objects: Vec<&Suggestion> = (found.iter())
        .filter(|s| matches!(s.kind, SuggestionKind::Object { .. }))
        .collect();
    assert!(objects.len() >= 3, "{}", objects.len());
    let first = found
        .iter()
        .position(|s| matches!(s.kind, SuggestionKind::Object { .. }));
    let blocks = found.len() - objects.len();
    assert_eq!(first, Some(blocks));
}

#[test]
fn suggestions_are_most_confident_first_and_never_overlap() {
    let found = suggest(&setup());
    let blocks: Vec<&Suggestion> = (found.iter())
        .filter(|s| matches!(s.kind, SuggestionKind::Block { .. }))
        .collect();
    assert!(
        blocks
            .windows(2)
            .all(|w| w[0].named_terrains() > w[1].named_terrains()
                || w[0].confidence() >= w[1].confidence())
    );
    for (i, a) in found.iter().enumerate() {
        for b in &found[i + 1..] {
            assert!(!overlaps(a, b.origin, b.size), "{a:?} overlaps {b:?}");
        }
    }
    assert_eq!(found, suggest(&setup()));
}

#[test]
fn tiles_already_in_a_set_are_not_suggested() {
    let mut setup = setup();
    let tiles: BTreeMap<TileAddress, SetupTile> = (0..GROUND.y)
        .flat_map(|y| (0..GROUND.x).map(move |x| (DIRT_BLOCK.y + y) * COLUMNS + DIRT_BLOCK.x + x))
        .map(|tile| {
            let peers = TileTerrains::filled(GRASS.0).with(jackdaw_tilemap::Peer::Centre, DIRT.0);
            let held = SetupTile {
                terrains: peers,
                set: Some((DIRT.0, GRASS.0)),
                ..Default::default()
            };
            (TileAddress { sheet: 0, tile }, held)
        })
        .collect();
    setup.tiles = tiles;
    let found = suggest(&setup);
    assert!(
        !found.iter().any(|s| overlaps(s, DIRT_BLOCK, GROUND)),
        "{found:#?}"
    );
    assert!(block(&found, WATER_BLOCK).is_some());
}

#[test]
fn blocks_naming_user_terrains_come_first() {
    let dirt_only = [(
        DIRT.0,
        TileAddress {
            sheet: 0,
            tile: DIRT.1,
        },
    )];
    let found = suggest_with(&setup(), &dirt_only);
    let blocks: Vec<bool> = (found.iter())
        .filter(|s| matches!(s.kind, SuggestionKind::Block { .. }))
        .map(Suggestion::named)
        .collect();
    assert!(blocks.contains(&false), "some blocks are unnamed");
    let first_unnamed = blocks.iter().position(|n| !n).unwrap_or(blocks.len());
    assert!(blocks[first_unnamed..].iter().all(|n| !n), "{blocks:?}");
}

#[test]
fn objects_have_three_tiles_or_more() {
    let found = suggest(&setup());
    let sizes: Vec<usize> = (objects(&found).into_iter())
        .map(|s| match &s.kind {
            SuggestionKind::Object { cells } => cells.len(),
            SuggestionKind::Block { .. } => 0,
        })
        .collect();
    assert!(!sizes.is_empty());
    assert!(sizes.iter().all(|n| *n >= MIN_OBJECT), "{sizes:?}");
}

#[test]
fn an_object_whose_bounds_hold_a_taken_tile_is_not_suggested() {
    let found = suggest(&setup());
    let (object, gap) = (objects(&found).into_iter())
        .find_map(|s| {
            let SuggestionKind::Object { cells } = &s.kind else {
                return None;
            };
            let gap = (0..s.size.y)
                .flat_map(|y| (0..s.size.x).map(move |x| UVec2::new(x, y)))
                .find(|at| !cells.contains(at))?;
            Some((s.clone(), s.origin + gap))
        })
        .expect("an object with a gap");
    let mut setup = setup();
    let held = SetupTile {
        set: Some((DIRT.0, GRASS.0)),
        ..Default::default()
    };
    let tile = gap.y * COLUMNS + gap.x;
    setup.tiles.insert(TileAddress { sheet: 0, tile }, held);
    let again = suggest(&setup);
    assert!(!again.contains(&object), "{object:?}");
    let taken = taken_tiles(&setup);
    assert!(again.iter().all(|s| s.free_in(&taken, COLUMNS)));
}

#[test]
fn a_block_with_one_known_terrain_carries_it() {
    let dirt_only = [(
        DIRT.0,
        TileAddress {
            sheet: 0,
            tile: DIRT.1,
        },
    )];
    let found = suggest_with(&setup(), &dirt_only);
    let s = block(&found, DIRT_BLOCK).expect("the dirt block");
    assert_eq!(pair(s), (Some(DIRT.0), None));
}

/// A sheet of a 16 corner layout of grass over transparent, then plain
/// grass tiles on the row below.
fn over_transparent() -> (Vec<u8>, UVec2) {
    let size = UVec2::new(4, 5);
    let (w, h) = (size.x * TILE, size.y * TILE);
    let mut pixels = vec![0; (w * h * 4) as usize];
    let shapes = LayoutTemplate::Corner16.placed_tiles(Placement::BetweenCells, 2, 1);
    let plain = std::iter::repeat_n(Some(TileTerrains::filled(2)), 4);
    for (i, shape) in shapes.into_iter().map(Some).chain(plain).enumerate() {
        let Some(shape) = shape else { continue };
        let cell = UVec2::new(i as u32 % size.x, i as u32 / size.x) * TILE;
        for y in 0..TILE {
            for x in 0..TILE {
                if shape.peers[((y / 8) * 6 + (x / 8) * 2) as usize] != 2 {
                    continue;
                }
                let noise = ((x * 7 + y * 13 + i as u32 * 5) % 9) as u8;
                let at = (((cell.y + y) * w + cell.x + x) * 4) as usize;
                pixels[at..at + 4].copy_from_slice(&[80 + noise, 140 + noise, 70, 255]);
            }
        }
    }
    (pixels, size)
}

#[test]
fn a_block_over_transparent_names_its_outer_empty() {
    let (pixels, size) = over_transparent();
    let input = SheetInput {
        name: "art",
        image: RgbaImage::new(size.x * TILE, size.y * TILE, &pixels).expect("sized"),
        layout: GridLayout {
            tile: UVec2::splat(TILE),
            margin: UVec2::ZERO,
            spacing: UVec2::ZERO,
            columns: size.x,
            rows: size.y,
        },
    };
    let grass = [(GRASS.0, TileAddress { sheet: 0, tile: 16 })];
    let found = suggest_blocks(&[input], &setup(), &grass).expect("suggests");
    let s = block(&found, UVec2::ZERO).unwrap_or_else(|| panic!("{found:#?}"));
    assert_eq!(pair(s), (Some(GRASS.0), Some(0)));
}

#[test]
fn blocks_naming_both_terrains_come_before_blocks_naming_one() {
    let found = suggest(&setup());
    let counts: Vec<usize> = (found.iter())
        .filter(|s| matches!(s.kind, SuggestionKind::Block { .. }))
        .map(Suggestion::named_terrains)
        .collect();
    assert!(counts.contains(&1) && counts.contains(&2), "{counts:?}");
    assert!(counts.windows(2).all(|w| w[0] >= w[1]), "{counts:?}");
}
