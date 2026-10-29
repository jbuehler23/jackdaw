use std::collections::BTreeMap;

use glam::UVec2;
use jackdaw_tilemap::{
    CellCoord, ChunkedLayer, GridLayout, Links, MatchMode, Peer, ProposedSet, RgbaImage,
    SheetInput, TerrainAnalysis, TerrainId, TerrainSet, TerrainTile, TileAddress, TileKind,
    TileRef, TileTerrains, TilesetProposal, find_terrains, fit_layouts, resolve_region,
};

struct Sheet {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    spacing: u32,
}

impl Sheet {
    fn load(name: &str, spacing: u32) -> Self {
        let path = format!("{}/tests/data/tiny_farm/{name}", env!("CARGO_MANIFEST_DIR"));
        let image = image::open(&path).expect("fixture decodes").to_rgba8();
        Self {
            width: image.width(),
            height: image.height(),
            pixels: image.into_raw(),
            spacing,
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
                columns: 12,
                rows: 11,
            },
        }
    }
}

fn packed() -> Sheet {
    Sheet::load("tilemap_packed.png", 0)
}

fn spaced() -> Sheet {
    Sheet::load("tilemap.png", 1)
}

fn tile(found: &TerrainAnalysis, sheet: usize, tile: u32) -> TileTerrains {
    found
        .tile(TileAddress { sheet, tile })
        .unwrap_or_else(|| panic!("tile {tile} labelled"))
        .terrains
}

/// The path tiles of the solver's farm fixture: index and the sides along
/// which the strip continues.
struct PathTiles {
    isolated: [u16; 2],
    vertical: [u16; 3],
    horizontal: [u16; 3],
}

const DRY_PATH: PathTiles = PathTiles {
    isolated: [0, 63],
    vertical: [12, 24, 36],
    horizontal: [60, 61, 62],
};
const TILLED_PATH: PathTiles = PathTiles {
    isolated: [1, 51],
    vertical: [13, 25, 37],
    horizontal: [48, 49, 50],
};

impl PathTiles {
    fn joins(&self) -> Vec<(u32, Vec<Peer>)> {
        let [i0, i1] = self.isolated;
        let [v0, v1, v2] = self.vertical;
        let [h0, h1, h2] = self.horizontal;
        [
            (i0, vec![]),
            (i1, vec![]),
            (v0, vec![Peer::Bottom]),
            (v1, vec![Peer::Top, Peer::Bottom]),
            (v2, vec![Peer::Top]),
            (h0, vec![Peer::Right]),
            (h1, vec![Peer::Left, Peer::Right]),
            (h2, vec![Peer::Left]),
        ]
        .into_iter()
        .map(|(index, joins)| (u32::from(index), joins))
        .collect()
    }

    /// Tiles whose strip fills rows 4 to 15, so their bottom edge shows the
    /// strip's lower lip rather than grass.
    fn lipped(&self) -> Vec<u32> {
        let [_, isolated] = self.isolated;
        self.horizontal
            .into_iter()
            .chain([isolated])
            .map(u32::from)
            .collect()
    }

    /// The raw label peers that show the lip: the bottom side, and the bottom
    /// corners where the strip runs on.
    fn lip(&self, index: u32, joins: &[Peer]) -> Vec<Peer> {
        if !self.lipped().contains(&index) {
            return Vec::new();
        }
        let mut lip = vec![Peer::Bottom];
        if joins.contains(&Peer::Left) {
            lip.push(Peer::BottomLeft);
        }
        if joins.contains(&Peer::Right) {
            lip.push(Peer::BottomRight);
        }
        lip
    }

    fn all(&self) -> Vec<u32> {
        self.joins().into_iter().map(|(i, _)| i).collect()
    }
}

struct Farm {
    grass: TerrainId,
    dry: TerrainId,
    tilled: TerrainId,
}

fn farm_terrains(found: &TerrainAnalysis) -> Farm {
    Farm {
        grass: tile(found, 0, 0).get(Peer::TopLeft),
        dry: tile(found, 0, 0).centre(),
        tilled: tile(found, 0, 1).centre(),
    }
}

#[test]
fn tiny_farm_has_grass_and_two_path_terrains() {
    let sheet = packed();
    let found = find_terrains(&[sheet.input("tiny_farm/tilemap_packed.png")]);
    let farm = farm_terrains(&found);
    assert_eq!(farm.grass, 1);
    assert_eq!(farm.dry, 2);
    assert_eq!(farm.tilled, 3);
    // The sheet draws grass only around paths and on roof tiles edged by an
    // outline, so no tile is all grass; a barn plank tile is the only pure one.
    let grass = found.terrain(farm.grass).expect("grass");
    assert!(grass.pure_tiles.is_empty());
    let pure: Vec<u32> = found
        .terrains
        .iter()
        .flat_map(|t| t.pure_tiles.iter().map(|p| p.tile))
        .collect();
    assert_eq!(pure, [91]);
    assert_eq!(grass.name, "grass");
    assert_eq!(found.terrain(farm.dry).expect("dry").name, "sand");
    assert_eq!(found.terrain(farm.tilled).expect("tilled").name, "dirt");
}

/// The fixture's logical peers: grass everywhere but the centre and the
/// joined sides.
fn path_shape(terrain: TerrainId, grass: TerrainId, joins: &[Peer]) -> TileTerrains {
    joins.iter().fold(
        TileTerrains::filled(grass).with(Peer::Centre, terrain),
        |t, side| t.with(*side, terrain),
    )
}

fn fit(sheet: &Sheet, name: &str) -> (TerrainAnalysis, TilesetProposal) {
    let input = sheet.input(name);
    let found = find_terrains(&[input]);
    let proposal = fit_layouts(&found, &[input]).expect("matching inputs");
    (found, proposal)
}

fn path_set(proposal: &TilesetProposal, inner: TerrainId) -> &ProposedSet {
    proposal
        .sets
        .iter()
        .find(|s| s.inner == inner)
        .unwrap_or_else(|| panic!("no set over {inner}: {:#?}", proposal.sets))
}

#[test]
fn tiny_farm_fitted_path_tiles_match_the_solver_fixture() {
    let sheet = packed();
    let (found, proposal) = fit(&sheet, "tilemap_packed.png");
    let farm = farm_terrains(&found);
    for (path, terrain) in [(DRY_PATH, farm.dry), (TILLED_PATH, farm.tilled)] {
        let set = path_set(&proposal, terrain);
        let mut tiles: Vec<u32> = set.tiles.iter().map(|t| t.address.tile).collect();
        let mut expected = path.all();
        tiles.sort_unstable();
        expected.sort_unstable();
        assert_eq!(tiles, expected);
        for (index, joins) in path.joins() {
            let fitted = set.tiles.iter().find(|t| t.address.tile == index);
            let fitted = fitted.unwrap_or_else(|| panic!("tile {index} fitted"));
            let shape = path_shape(terrain, farm.grass, &joins);
            assert_eq!(fitted.terrains, shape, "tile {index}");
            let lipped = path.lipped().contains(&index);
            assert_eq!(
                fitted.corrected[Peer::Bottom.index()],
                lipped,
                "tile {index}"
            );
        }
    }
    // The raw labels read the dry lip as tilled, the colour it shares.
    for index in DRY_PATH.lipped() {
        assert_eq!(tile(&found, 0, index).get(Peer::Bottom), farm.tilled);
    }
}

#[test]
fn tiny_farm_raw_path_labels_differ_from_the_fixture_only_on_the_lip() {
    let sheet = packed();
    let found = find_terrains(&[sheet.input("tilemap_packed.png")]);
    let farm = farm_terrains(&found);
    for (path, terrain) in [(DRY_PATH, farm.dry), (TILLED_PATH, farm.tilled)] {
        for (index, joins) in path.joins() {
            let labels = found.tile(TileAddress {
                sheet: 0,
                tile: index,
            });
            assert_eq!(
                labels.map(|t| t.kind),
                Some(TileKind::Terrain),
                "tile {index}"
            );
            let got = tile(&found, 0, index);
            let shape = path_shape(terrain, farm.grass, &joins);
            let lip = path.lip(index, &joins);
            for peer in Peer::ALL {
                if lip.contains(&peer) {
                    let lower = got.get(peer);
                    assert!(lower != farm.grass && lower != 0, "tile {index} {peer:?}");
                } else {
                    assert_eq!(got.get(peer), shape.get(peer), "tile {index} {peer:?}");
                }
            }
        }
    }
}

#[test]
fn tiny_farm_proposal_has_two_side_path_sets_over_background_grass() {
    let sheet = packed();
    let (found, proposal) = fit(&sheet, "tilemap_packed.png");
    let farm = farm_terrains(&found);
    assert_eq!(proposal.sets.len(), 2, "{:#?}", proposal.sets);
    for terrain in [farm.dry, farm.tilled] {
        let set = path_set(&proposal, terrain);
        assert_eq!((set.mode, set.outer), (MatchMode::Sides, farm.grass));
        assert_eq!((set.covered, set.total), (7, 16));
        assert_eq!(set.missing.len(), 9);
        assert_eq!(set.template, None);
    }
    let background: Vec<TerrainId> = proposal
        .terrains
        .iter()
        .filter(|t| t.background)
        .map(|t| t.id)
        .collect();
    assert_eq!(background, [farm.grass]);
    let unassigned: Vec<u32> = proposal.unassigned.iter().map(|a| a.tile).collect();
    let barn = [90, 92, 102, 103, 104, 114, 115, 116, 127];
    let roof = [93, 95, 105, 107, 117, 118, 119, 129, 131];
    let bench_and_trough = [98, 99, 110, 111, 112, 113];
    for index in barn.into_iter().chain(roof).chain(bench_and_trough) {
        assert!(unassigned.contains(&index), "tile {index}: {unassigned:?}");
    }
    // The plank tile is its terrain's only pure tile, so the terrain stays.
    assert!(!unassigned.contains(&91));
}

#[test]
fn tiny_farm_packed_and_spaced_sheets_give_the_same_proposal() {
    let (a, b) = (packed(), spaced());
    let (_, alone) = fit(&a, "tilemap_packed.png");
    let (_, spaced) = fit(&b, "tilemap.png");
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
fn tiny_farm_fitted_sets_paint_like_the_hand_fixture() {
    let sheet = packed();
    let (found, proposal) = fit(&sheet, "tilemap_packed.png");
    let farm = farm_terrains(&found);
    let runs = [
        ".......\n.=====.\n.......",
        "...\n.=.\n.=.\n.=.\n.=.\n.=.\n...",
        ".....\n.===.\n.=...\n.=...\n.....",
        ".....\n.=...\n.=...\n.===.\n.....",
    ];
    for (path, terrain) in [(DRY_PATH, farm.dry), (TILLED_PATH, farm.tilled)] {
        let [i0, i1] = path.isolated;
        let mut hand = TerrainSet::new(MatchMode::Sides);
        for (index, joins) in [(i0, vec![]), (i1, vec![])].into_iter().chain(
            path.joins()
                .into_iter()
                .filter(|(_, j)| !j.is_empty())
                .map(|(i, j)| (i as u16, j)),
        ) {
            let shape = path_shape(terrain, farm.grass, &joins);
            hand = hand.with_tile(TerrainTile::new(TileRef::new(index), shape, 1));
        }
        let fitted = path_set(&proposal, terrain)
            .terrain_set(|a| u16::try_from(a.tile).ok().map(TileRef::new));
        for rows in runs {
            let expected = paint(&hand, rows, farm.grass, terrain);
            assert!(expected.iter().any(|(_, t)| !t.is_empty()));
            assert_eq!(
                paint(&fitted, rows, farm.grass, terrain),
                expected,
                "{rows}"
            );
        }
    }
}

#[test]
fn tiny_farm_spaced_sheet_gives_the_same_labels() {
    let (a, b) = (packed(), spaced());
    let alone = find_terrains(&[a.input("tilemap_packed.png")]);
    let spaced = find_terrains(&[b.input("tilemap.png")]);
    assert_eq!(spaced.tiles, alone.tiles);
    let colours = |f: &TerrainAnalysis| f.terrains.iter().map(|t| t.oklab).collect::<Vec<_>>();
    assert_eq!(colours(&spaced), colours(&alone));

    let both = find_terrains(&[a.input("tilemap_packed.png"), b.input("tilemap.png")]);
    assert_eq!(both.terrains.len(), alone.terrains.len());
    for index in 0..132 {
        let pair = [0, 1].map(|s| {
            both.tile(TileAddress {
                sheet: s,
                tile: index,
            })
        });
        let alone = alone.tile(TileAddress {
            sheet: 0,
            tile: index,
        });
        assert_eq!(pair[0].map(|t| t.terrains), alone.map(|t| t.terrains));
        assert_eq!(pair[1].map(|t| t.terrains), alone.map(|t| t.terrains));
    }
}

#[test]
fn tiny_farm_objects_are_never_grass_path_transitions() {
    let sheet = packed();
    let found = find_terrains(&[sheet.input("tilemap_packed.png")]);
    let farm = farm_terrains(&found);
    let path: Vec<u32> = DRY_PATH
        .all()
        .into_iter()
        .chain(TILLED_PATH.all())
        .collect();
    let mut path_coloured = Vec::new();
    for index in (0..132).filter(|i| !path.contains(i)) {
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
        let pathlike = peers.contains(&farm.dry) || peers.contains(&farm.tilled);
        assert!(
            !(pathlike && peers.contains(&farm.grass)),
            "tile {index}: {peers:?}"
        );
        if pathlike {
            path_coloured.push(index);
        }
    }
    // Objects drawn in the path palette that run across a tile edge: a
    // crate, a tree trunk, a bench and two troughs.
    assert_eq!(path_coloured, [11, 14, 98, 99, 110, 111, 112, 113]);
    // Single-tile crops, tree crowns and animals.
    for index in [2, 4, 5, 6, 15, 16, 26, 27, 39, 42, 54, 66, 120, 121, 122] {
        let labels = found.tile(TileAddress {
            sheet: 0,
            tile: index,
        });
        assert_eq!(
            labels.map(|t| t.kind),
            Some(TileKind::Decor),
            "tile {index}"
        );
    }
}
