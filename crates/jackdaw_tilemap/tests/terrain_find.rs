//! Terrain finding and layout fitting over the overworld sheet, checked
//! against the wang sets its Tiled tileset ships with.

use std::collections::BTreeMap;

use glam::UVec2;
use jackdaw_tilemap::{
    CellCoord, ChunkedLayer, GridLayout, Links, MatchMode, Peer, ProposedSet, RgbaImage,
    SheetInput, TerrainAnalysis, TerrainId, TerrainSet, TileAddress, TileKind, TileRef,
    TileTerrains, TilesetProposal, find_terrains, fit_layouts, resolve_region,
};

const COLUMNS: u32 = 27;
const ROWS: u32 = 38;
const TILES: u32 = COLUMNS * ROWS;

struct Sheet {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    spacing: u32,
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
            spacing: 0,
        }
    }

    /// The packed sheet with a transparent pixel between neighbouring tiles.
    fn spaced() -> Self {
        let packed = Self::packed();
        let (w, h) = (packed.width, packed.height);
        let (sw, sh) = (w / 16 * 17 - 1, h / 16 * 17 - 1);
        let mut pixels = vec![0; (sw * sh * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let from = ((y * w + x) * 4) as usize;
                let to = (((y + y / 16) * sw + x + x / 16) * 4) as usize;
                pixels[to..to + 4].copy_from_slice(&packed.pixels[from..from + 4]);
            }
        }
        Self {
            width: sw,
            height: sh,
            pixels,
            spacing: 1,
        }
    }

    fn input<'a>(&'a self, name: &'a str) -> SheetInput<'a> {
        SheetInput {
            name,
            image: RgbaImage::new(self.width, self.height, &self.pixels).expect("sized"),
            layout: GridLayout {
                tile: UVec2::splat(16),
                margin: UVec2::ZERO,
                spacing: UVec2::splat(self.spacing),
                columns: COLUMNS,
                rows: ROWS,
            },
        }
    }
}

const GRASS: TerrainId = 1;
/// Dirt and sand share one colour cluster on this sheet.
const GROUND: TerrainId = 2;
const SHALLOW_SEA: TerrainId = 9;
const WATER: TerrainId = 11;
const DEEP_SEA: TerrainId = 12;

/// The terrain auto setup finds for a colour of the Tiled tileset's
/// `overworld` corner wang set, or `None` for colours it does not model
/// (cliffs, trees and air).
fn overworld_colour(colour: u8) -> Option<TerrainId> {
    match colour {
        1 => Some(GRASS),
        2 | 3 => Some(GROUND),
        8 => Some(SHALLOW_SEA),
        7 | 9 => Some(WATER),
        10 => Some(DEEP_SEA),
        _ => None,
    }
}

/// The terrain for a colour of the `pathways` edge wang set; 0 is the
/// grass the paths run over.
fn pathway_colour(colour: u8) -> Option<TerrainId> {
    match colour {
        0 => Some(GRASS),
        1 | 2 => Some(GROUND),
        3 => Some(WATER),
        _ => None,
    }
}

/// Tile id and wang id of every tile of the named wang set in the
/// fixture's Tiled tileset.
fn wang_tiles(set: &str) -> Vec<(u32, [u8; 8])> {
    let path = format!(
        "{}/tests/data/punyworld/punyworld-overworld-tiles.tsx",
        env!("CARGO_MANIFEST_DIR")
    );
    let tsx = std::fs::read_to_string(path).expect("the Tiled tileset reads");
    let attribute = |line: &str, name: &str| -> String {
        let start = line.find(&format!("{name}=\"")).expect("attribute") + name.len() + 2;
        line[start..]
            .split('"')
            .next()
            .unwrap_or_default()
            .to_string()
    };
    let body = tsx
        .split(&format!("<wangset name=\"{set}\""))
        .nth(1)
        .and_then(|rest| rest.split("</wangset>").next())
        .expect("the wang set");
    body.lines()
        .filter(|line| line.contains("<wangtile "))
        .map(|line| {
            let id = attribute(line, "tileid").parse().expect("tile id");
            let wang: Vec<u8> = (attribute(line, "wangid").split(','))
                .map(|v| v.parse().expect("wang colour"))
                .collect();
            (id, wang.try_into().expect("eight wang colours"))
        })
        .collect()
}

fn fit(sheet: &Sheet, name: &str) -> (TerrainAnalysis, TilesetProposal) {
    let input = sheet.input(name);
    let found = find_terrains(&[input]);
    let proposal = fit_layouts(&found, &[input]).expect("matching inputs");
    (found, proposal)
}

fn fitted(proposal: &TilesetProposal, tile: u32) -> Option<(&ProposedSet, TileTerrains)> {
    proposal.sets.iter().find_map(|set| {
        let found = set.tiles.iter().find(|t| t.address.tile == tile);
        found.map(|t| (set, t.terrains))
    })
}

fn path_set(proposal: &TilesetProposal, inner: TerrainId, outer: TerrainId) -> &ProposedSet {
    proposal
        .sets
        .iter()
        .find(|s| (s.inner, s.outer) == (inner, outer))
        .unwrap_or_else(|| panic!("no set {inner} over {outer}: {:#?}", proposal.sets))
}

#[test]
fn overworld_has_grass_and_one_ground_terrain_for_dirt_and_sand() {
    let sheet = Sheet::packed();
    let found = find_terrains(&[sheet.input("overworld.png")]);
    let at = |tile| found.tile(TileAddress { sheet: 0, tile });
    let centre = |tile| at(tile).map(|l| l.terrains.centre());
    assert_eq!(at(0).map(|l| l.terrains.get(Peer::TopLeft)), Some(GRASS));
    // Tiled's pure dirt and pure sand tiles.
    assert_eq!((centre(38), centre(50)), (Some(GROUND), Some(GROUND)));
    let pure = |id| -> Vec<u32> {
        let terrain = found.terrain(id).expect("terrain");
        terrain.pure_tiles.iter().map(|a| a.tile).collect()
    };
    assert_eq!(pure(GRASS), [0, 1, 2, 27, 28, 29, 54, 55, 56, 136, 147]);
    assert_eq!(pure(GROUND), [35, 38, 47, 50]);
    // The bare material by hue: the yellow-olive ground reads as grass too.
    let name = |id| found.terrain(id).map(|t| t.name.as_str());
    assert_eq!((name(GRASS), name(GROUND)), (Some("grass"), Some("grass")));
    assert_eq!(
        (name(WATER), name(DEEP_SEA)),
        (Some("water"), Some("water"))
    );
}

#[test]
fn overworld_fitted_corners_match_the_tiled_overworld_wang_set() {
    let sheet = Sheet::packed();
    let (_, proposal) = fit(&sheet, "overworld.png");
    let mut checked = 0;
    let mut unfitted = Vec::new();
    for (tile, wang) in wang_tiles("overworld") {
        // Tiled's corner order: top right, bottom right, bottom left, top left.
        let corners = [wang[7], wang[1], wang[5], wang[3]].map(overworld_colour);
        let Some(corners) = corners.into_iter().collect::<Option<Vec<_>>>() else {
            continue;
        };
        if corners.iter().all(|&c| c == GRASS) {
            continue;
        }
        let Some((set, terrains)) = fitted(&proposal, tile) else {
            unfitted.push(tile);
            continue;
        };
        assert!(
            matches!(set.mode, MatchMode::Corners | MatchMode::CornersAndSides),
            "tile {tile}"
        );
        let got = [
            Peer::TopLeft,
            Peer::TopRight,
            Peer::BottomLeft,
            Peer::BottomRight,
        ]
        .map(|p| terrains.get(p));
        assert_eq!(got.to_vec(), corners, "tile {tile}");
        checked += 1;
    }
    assert_eq!(checked, 81);
    // A river mouth corner that terrain finding labels as decor.
    assert_eq!(unfitted, [277]);
}

#[test]
fn overworld_fitted_sides_match_the_tiled_pathways_wang_set() {
    let sheet = Sheet::packed();
    let (_, proposal) = fit(&sheet, "overworld.png");
    let mut checked = 0;
    let mut unfitted = Vec::new();
    for (tile, wang) in wang_tiles("pathways") {
        let sides = [wang[0], wang[6], wang[2], wang[4]].map(pathway_colour);
        let sides: Vec<TerrainId> = sides.into_iter().flatten().collect();
        let Some((_, terrains)) = fitted(&proposal, tile) else {
            unfitted.push(tile);
            continue;
        };
        let got = [Peer::Top, Peer::Left, Peer::Right, Peer::Bottom].map(|p| terrains.get(p));
        assert_eq!(got.to_vec(), sides, "tile {tile}");
        checked += 1;
    }
    assert_eq!(checked, 43);
    assert_eq!(unfitted, [271, 324]);
}

#[test]
fn overworld_proposal_has_ground_and_water_sets_over_grass_and_the_sea_bands() {
    let sheet = Sheet::packed();
    let (_, proposal) = fit(&sheet, "overworld.png");
    let sets: Vec<(TerrainId, TerrainId, MatchMode, usize, usize)> = (proposal.sets.iter())
        .map(|s| (s.inner, s.outer, s.mode, s.covered, s.total))
        .collect();
    assert_eq!(
        sets,
        [
            (GROUND, GRASS, MatchMode::CornersAndSides, 35, 47),
            (WATER, GRASS, MatchMode::CornersAndSides, 30, 47),
            (SHALLOW_SEA, GROUND, MatchMode::Corners, 14, 16),
            (5, 6, MatchMode::Corners, 6, 16),
            (WATER, SHALLOW_SEA, MatchMode::Corners, 14, 16),
            (DEEP_SEA, WATER, MatchMode::Corners, 14, 16),
        ]
    );
    let background: Vec<TerrainId> = (proposal.terrains.iter())
        .filter(|t| t.background)
        .map(|t| t.id)
        .collect();
    assert_eq!(background, [6]);
    let unassigned: Vec<u32> = proposal.unassigned.iter().map(|a| a.tile).collect();
    // Cliff and tree tiles of the overworld wang set: colours no set models.
    for tile in [108, 110, 119, 121, 189, 216] {
        assert!(unassigned.contains(&tile), "tile {tile}: {unassigned:?}");
    }
}

#[test]
fn overworld_packed_and_spaced_sheets_give_the_same_proposal() {
    let (a, b) = (Sheet::packed(), Sheet::spaced());
    let (_, alone) = fit(&a, "overworld.png");
    let (_, spaced) = fit(&b, "overworld_spaced.png");
    assert_eq!(spaced, alone);
}

/// Resolves every cell of `rows` ('.' grass, '=' the path) with `set`.
fn paint(
    set: &TerrainSet,
    rows: &str,
    grass: TerrainId,
    path: TerrainId,
) -> Vec<(CellCoord, TileRef)> {
    let mut terrain = ChunkedLayer::new();
    for (row, line) in rows.lines().enumerate() {
        for (col, symbol) in line.chars().enumerate() {
            let id = if symbol == '=' { path } else { grass };
            terrain.set(CellCoord::new(col as i32, -(row as i32)), id);
        }
    }
    let dirty: Vec<CellCoord> = terrain.cells().map(|(c, _)| c).collect();
    let links = ChunkedLayer::<Links>::new();
    resolve_region(set, &terrain, &links, &BTreeMap::new(), dirty)
}

#[test]
fn overworld_fitted_ground_set_paints_paths_with_tiled_pathway_tiles() {
    let sheet = Sheet::packed();
    let (_, proposal) = fit(&sheet, "overworld.png");
    let set = path_set(&proposal, GROUND, GRASS)
        .terrain_set(|a| u16::try_from(a.tile).ok().map(TileRef::new));
    // Joined sides of each dirt or sand path tile: top, left, right, bottom.
    let pathways: BTreeMap<u32, [bool; 4]> = wang_tiles("pathways")
        .into_iter()
        .filter(|(_, w)| [w[0], w[2], w[4], w[6]].iter().all(|&c| c == 1 || c == 0))
        .chain(
            wang_tiles("pathways")
                .into_iter()
                .filter(|(_, w)| [w[0], w[2], w[4], w[6]].iter().all(|&c| c == 2 || c == 0)),
        )
        .map(|(tile, w)| (tile, [w[0], w[6], w[2], w[4]].map(|c| c != 0)))
        .collect();
    let runs = [
        ".......\n.=====.\n.......",
        "...\n.=.\n.=.\n.=.\n.=.\n.=.\n...",
        ".....\n.===.\n.=...\n.=...\n.....",
        ".....\n.=...\n.=...\n.===.\n.....",
    ];
    let mut unlisted = Vec::new();
    for rows in runs {
        let grid: Vec<Vec<char>> = rows.lines().map(|l| l.chars().collect()).collect();
        let is_path = |col: i32, row: i32| {
            let line = usize::try_from(row).ok().and_then(|r| grid.get(r));
            let cell = line.and_then(|l| usize::try_from(col).ok().and_then(|c| l.get(c)));
            cell == Some(&'=')
        };
        let painted = paint(&set, rows, GRASS, GROUND);
        for (cell, tile) in painted {
            let (col, row) = (cell.0.x, -cell.0.y);
            if !is_path(col, row) {
                continue;
            }
            let joins = [
                is_path(col, row - 1),
                is_path(col - 1, row),
                is_path(col + 1, row),
                is_path(col, row + 1),
            ];
            assert!(!tile.is_empty(), "{rows}: {cell:?}");
            let id = u32::from(tile.index());
            let (_, terrains) = fitted(&proposal, id).expect("a fitted tile");
            let sides = [Peer::Top, Peer::Left, Peer::Right, Peer::Bottom];
            assert_eq!(
                sides.map(|p| terrains.get(p) == GROUND),
                joins,
                "{rows}: tile {id}"
            );
            match pathways.get(&id) {
                Some(tiled) => assert_eq!(*tiled, joins, "{rows}: tile {id}"),
                None => unlisted.push(id),
            }
        }
    }
    // Path tiles the Tiled tileset leaves out of its wang sets.
    unlisted.sort_unstable();
    unlisted.dedup();
    assert_eq!(unlisted, [143, 145, 171]);
}

#[test]
fn overworld_spaced_sheet_gives_the_same_labels() {
    let (a, b) = (Sheet::packed(), Sheet::spaced());
    let alone = find_terrains(&[a.input("overworld.png")]);
    let spaced = find_terrains(&[b.input("overworld_spaced.png")]);
    assert_eq!(spaced.tiles, alone.tiles);
    let colours = |f: &TerrainAnalysis| f.terrains.iter().map(|t| t.oklab).collect::<Vec<_>>();
    assert_eq!(colours(&spaced), colours(&alone));

    // Analysed together, colours seeded by one tile per sheet reach the two
    // tiles a terrain needs, so the joint analysis finds more terrains; both
    // copies of every tile still get the same labels.
    let both = find_terrains(&[a.input("overworld.png"), b.input("overworld_spaced.png")]);
    assert_eq!((alone.terrains.len(), both.terrains.len()), (17, 20));
    for index in 0..TILES {
        let pair = [0, 1].map(|s| {
            both.tile(TileAddress {
                sheet: s,
                tile: index,
            })
        });
        assert_eq!(pair[0].map(|t| t.terrains), pair[1].map(|t| t.terrains));
    }
}

#[test]
fn overworld_objects_are_never_grass_ground_transitions() {
    let sheet = Sheet::packed();
    let (found, proposal) = fit(&sheet, "overworld.png");
    let mut mixed = Vec::new();
    for index in 0..TILES {
        if fitted(&proposal, index).is_some() {
            continue;
        }
        let Some(labels) = found.tile(TileAddress {
            sheet: 0,
            tile: index,
        }) else {
            continue;
        };
        if labels.kind == TileKind::Decor {
            continue;
        }
        let peers = labels.terrains.peers;
        if peers.contains(&GROUND) && peers.contains(&GRASS) {
            mixed.push(index);
        }
    }
    assert_eq!(mixed, Vec::<u32>::new());
}
