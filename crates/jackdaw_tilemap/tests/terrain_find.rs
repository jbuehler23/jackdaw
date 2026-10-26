use glam::UVec2;
use jackdaw_tilemap::{
    GridLayout, Peer, RgbaImage, SheetInput, TerrainAnalysis, TerrainId, TileAddress, TileKind,
    TileTerrains, find_terrains,
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

    /// Where the pixels disagree with the fixture: the horizontal strip and
    /// the second isolated tile fill rows 4 to 15 of their tiles, so their
    /// bottom side, and the bottom corners where the strip runs on, show the
    /// strip's lower lip rather than grass.
    fn lip(&self, index: u32, joins: &[Peer]) -> Vec<Peer> {
        let low =
            self.horizontal.map(u32::from).contains(&index) || index == u32::from(self.isolated[1]);
        if !low {
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
    assert_eq!(grass.name, "tilemap_packed green");
    assert_eq!(
        found.terrain(farm.dry).expect("dry").name,
        "tilemap_packed sand"
    );
    assert_eq!(
        found.terrain(farm.tilled).expect("tilled").name,
        "tilemap_packed brown"
    );
}

#[test]
fn tiny_farm_path_tiles_match_the_solver_fixture() {
    let sheet = packed();
    let found = find_terrains(&[sheet.input("tilemap_packed.png")]);
    let farm = farm_terrains(&found);
    for (path, terrain) in [(DRY_PATH, farm.dry), (TILLED_PATH, farm.tilled)] {
        for (index, joins) in path.joins() {
            let lip = path.lip(index, &joins);
            let got = tile(&found, 0, index);
            let kind = found.tile(TileAddress {
                sheet: 0,
                tile: index,
            });
            assert_eq!(
                kind.map(|t| t.kind),
                Some(TileKind::Terrain),
                "tile {index}"
            );
            for peer in Peer::ALL {
                let joined = peer == Peer::Centre || joins.contains(&peer);
                let expected = if joined { terrain } else { farm.grass };
                if lip.contains(&peer) {
                    let lower = got.get(peer);
                    assert!(lower != farm.grass && lower != 0, "tile {index} {peer:?}");
                } else {
                    assert_eq!(got.get(peer), expected, "tile {index} {peer:?}: {got:?}");
                }
            }
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
