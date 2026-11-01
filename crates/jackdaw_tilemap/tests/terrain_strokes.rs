//! Dirt strokes over a bounded 9x7 grass map, rendered as text and compared
//! with golden grids. Coordinates are (column, row) with row 0 at the top.
//!
//! `Corners` maps draw tiles between cells. They render one character per
//! cell, on the rows of cells, for the corner every tile touching the cell
//! gives it, `!` where those tiles disagree. The rows between show each
//! drawn tile, the cropped ring past the bounds included: its terrain when
//! its corners agree, `+` for a transition. Other modes
//! render each cell's nine peers, corners blank when unused.

use std::collections::BTreeMap;

use glam::IVec2;
use jackdaw_tilemap::{
    CellCoord, CellRect, ChunkedLayer, LayoutTemplate, Links, MatchMode, Peer, TerrainId,
    TerrainSet, TerrainTile, TileRef, TileTerrains, apply_connect_stroke, apply_path_stroke,
    resolve_region_within,
};

const GRASS: TerrainId = 1;
const DIRT: TerrainId = 2;
const WIDTH: i32 = 9;
const HEIGHT: i32 = 7;

/// Dirt-over-grass template tiles plus a plain grass tile. The `Corners` set
/// drops the dirt tile with four grass corners, as sheets cut from art do.
fn set(template: LayoutTemplate) -> TerrainSet {
    let mut set = template.terrain_set(DIRT, GRASS).with_order([GRASS, DIRT]);
    set.tiles.retain(|tile| {
        template != LayoutTemplate::Corner16
            || Peer::CORNERS
                .iter()
                .any(|corner| tile.terrains.get(*corner) == DIRT)
    });
    let plain = TileTerrains::filled(GRASS);
    set.with_tile(TerrainTile::new(TileRef::new(100), plain, 1))
}

fn cell(x: i32, y: i32) -> CellCoord {
    CellCoord::new(x, -y)
}

fn bounds() -> CellRect {
    CellRect {
        min: IVec2::new(0, 1 - HEIGHT),
        max: IVec2::new(WIDTH, 1),
    }
}

struct Map {
    set: TerrainSet,
    terrain: ChunkedLayer<TerrainId>,
    links: ChunkedLayer<Links>,
    tiles: BTreeMap<CellCoord, TileRef>,
}

impl Map {
    fn new(set: TerrainSet) -> Self {
        let mut map = Map {
            set,
            terrain: ChunkedLayer::new(),
            links: ChunkedLayer::new(),
            tiles: BTreeMap::new(),
        };
        let all = all_cells();
        map.connect(&all, GRASS);
        map
    }

    fn update(&mut self, dirty: Vec<CellCoord>) {
        let out = resolve_region_within(
            &self.set,
            &self.terrain,
            &self.links,
            &BTreeMap::new(),
            dirty,
            Some(bounds()),
        );
        self.tiles.extend(out);
    }

    fn connect(&mut self, cells: &[(i32, i32)], id: TerrainId) {
        let cells: Vec<_> = cells.iter().map(|&(x, y)| cell(x, y)).collect();
        let dirty = apply_connect_stroke(&mut self.terrain, &mut self.links, &cells, id);
        self.update(dirty);
    }

    fn path(&mut self, cells: &[(i32, i32)], id: TerrainId) {
        let cells: Vec<_> = cells.iter().map(|&(x, y)| cell(x, y)).collect();
        let dirty = apply_path_stroke(&mut self.terrain, &mut self.links, &cells, id)
            .expect("contiguous stroke");
        self.update(dirty);
    }

    /// Peer terrains of the tile keyed at `key`, `None` when empty.
    fn tile(&self, key: CellCoord) -> Option<TileTerrains> {
        let tile = self.tiles.get(&key).filter(|t| !t.is_empty())?;
        let found = self.set.tiles.iter().find(|t| t.tile == *tile)?;
        Some(found.terrains)
    }

    /// Peer terrains of the tile on cell `(x, y)`, `None` off the map or empty.
    fn shape(&self, x: i32, y: i32) -> Option<TileTerrains> {
        if !(0..WIDTH).contains(&x) || !(0..HEIGHT).contains(&y) {
            return None;
        }
        self.tile(cell(x, y))
    }

    fn render(&self) -> String {
        if self.set.mode == MatchMode::Corners {
            self.render_between()
        } else {
            self.render_peers()
        }
    }

    /// The terrain the tiles touching cell `(x, y)` give it, `!` when they
    /// disagree, blank when none does.
    fn vertex(&self, x: i32, y: i32) -> char {
        let at = cell(x, y).0;
        let around = [
            (IVec2::new(0, 0), Peer::BottomLeft),
            (IVec2::new(1, 0), Peer::BottomRight),
            (IVec2::new(0, 1), Peer::TopLeft),
            (IVec2::new(1, 1), Peer::TopRight),
        ];
        let mut seen: Vec<TerrainId> = (around.iter())
            .filter_map(|&(d, peer)| self.tile(CellCoord(at - d)).map(|t| t.get(peer)))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        match seen.as_slice() {
            [] => ' ',
            [one] => symbol(*one),
            _ => '!',
        }
    }

    fn render_between(&self) -> String {
        let mut lines = Vec::new();
        for row in -1..HEIGHT {
            let tiles: String = (-1..WIDTH)
                .map(|x| {
                    let key = CellCoord::new(x, -row - 1);
                    let c = match self.tile(key).map(|t| Peer::CORNERS.map(|c| t.get(c))) {
                        None => ' ',
                        Some([a, b, c, d]) if a == b && b == c && c == d => symbol(a),
                        Some(_) => '+',
                    };
                    format!("{c} ")
                })
                .collect();
            lines.push(tiles.trim_end().to_string());
            if row + 1 < HEIGHT {
                let cells: String = (0..WIDTH)
                    .map(|x| format!(" {}", self.vertex(x, row + 1)))
                    .collect();
                lines.push(cells);
            }
        }
        lines.join("\n")
    }

    /// The painted cells as the vertex rows render them.
    fn painted(&self) -> Vec<String> {
        (0..HEIGHT)
            .map(|y| {
                (0..WIDTH)
                    .map(|x| format!(" {}", symbol(self.terrain.get(cell(x, y)))))
                    .collect()
            })
            .collect()
    }

    fn render_peers(&self) -> String {
        let mut lines = Vec::new();
        for y in 0..HEIGHT {
            for band in 0..3 {
                let mut line = String::new();
                for x in 0..WIDTH {
                    for peer in &Peer::ALL[band * 3..band * 3 + 3] {
                        let used = self.set.mode.uses(*peer);
                        let c = match self.shape(x, y) {
                            Some(s) if used => symbol(s.get(*peer)),
                            _ => ' ',
                        };
                        line.push(c);
                    }
                    line.push('|');
                }
                lines.push(line);
            }
            lines.push(String::new());
        }
        lines.join("\n")
    }
}

fn symbol(terrain: TerrainId) -> char {
    match terrain {
        GRASS => '.',
        DIRT => '#',
        _ => '?',
    }
}

fn all_cells() -> Vec<(i32, i32)> {
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .collect()
}

fn block(xs: std::ops::Range<i32>, ys: std::ops::Range<i32>) -> Vec<(i32, i32)> {
    ys.flat_map(|y| xs.clone().map(move |x| (x, y))).collect()
}

/// A stroke case: its strokes in order and whether each can be a Path stroke.
fn cases() -> Vec<(&'static str, Vec<Vec<(i32, i32)>>, bool)> {
    let line = |y| (2..7).map(|x| (x, y)).collect::<Vec<_>>();
    vec![
        ("single", vec![vec![(4, 3)]], true),
        ("line5", vec![line(3)], true),
        (
            "L",
            vec![vec![(2, 1), (2, 2), (2, 3), (3, 3), (4, 3)]],
            true,
        ),
        ("block2x2", vec![vec![(3, 2), (4, 2), (4, 3), (3, 3)]], true),
        ("edge-line", vec![vec![(0, 3), (1, 3), (2, 3)]], true),
        ("edge-single", vec![vec![(0, 3)]], true),
        ("edge-vertical", vec![vec![(4, 0), (4, 1), (4, 2)]], true),
        ("parallel", vec![line(2), line(3)], true),
        ("band3", vec![block(2..6, 2..5)], false),
        ("edge-band3", vec![block(0..4, 2..5)], false),
        ("all", vec![all_cells()], false),
    ]
}

fn golden(name: &str) -> Option<&'static str> {
    GOLDEN
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, text)| *text)
}

fn check(prefix: &str, template: LayoutTemplate) {
    let mut failures = Vec::new();
    for (name, strokes, pathable) in cases() {
        let connect_name = format!("{prefix}-{name}");
        let expected = golden(&connect_name).expect("connect golden");
        let mut kinds = vec![(connect_name.clone(), expected, false)];
        if pathable {
            let path_name = format!("{connect_name}-path");
            kinds.push((
                path_name.clone(),
                golden(&path_name).unwrap_or(expected),
                true,
            ));
        }
        for (label, expected, path) in kinds {
            let mut map = Map::new(set(template));
            for stroke in &strokes {
                if path {
                    map.path(stroke, DIRT);
                } else {
                    map.connect(stroke, DIRT);
                }
            }
            let got = map.render();
            if got.trim_end() != expected.trim_end() {
                failures.push(format!("{label}\nexpected:\n{expected}\ngot:\n{got}\n"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn sides_strokes_match_golden() {
    check("sides", LayoutTemplate::Side16);
}

#[test]
fn corners_and_sides_strokes_match_golden() {
    check("blob", LayoutTemplate::Blob47);
}

#[test]
fn corners_strokes_match_golden() {
    check("corners", LayoutTemplate::Corner16);
}

#[test]
fn corner_cells_show_their_painted_terrain() {
    for (name, strokes, _) in cases() {
        let mut map = Map::new(set(LayoutTemplate::Corner16));
        for stroke in &strokes {
            map.connect(stroke, DIRT);
        }
        let rows: Vec<String> = (0..HEIGHT)
            .map(|y| {
                (0..WIDTH)
                    .map(|x| format!(" {}", map.vertex(x, y)))
                    .collect()
            })
            .collect();
        assert_eq!(rows, map.painted(), "{name}");
    }
}

#[test]
fn corner_tiles_run_one_ring_past_the_bounds() {
    let map = Map::new(set(LayoutTemplate::Corner16));
    let keys: Vec<CellCoord> = map
        .tiles
        .iter()
        .filter(|(_, t)| !t.is_empty())
        .map(|(k, _)| *k)
        .collect();
    let min = keys.iter().map(|k| k.0).reduce(IVec2::min);
    let max = keys.iter().map(|k| k.0).reduce(IVec2::max);
    assert_eq!(min, Some(bounds().min - IVec2::ONE));
    assert_eq!(max, Some(bounds().max - IVec2::ONE));
    assert_eq!(keys.len() as i32, (WIDTH + 1) * (HEIGHT + 1));
}

const GOLDEN: &[(&str, &str)] = &[
    (
        "sides-single",
        include_str!("golden/terrain_strokes/sides-single.txt"),
    ),
    (
        "sides-line5",
        include_str!("golden/terrain_strokes/sides-line5.txt"),
    ),
    (
        "sides-L",
        include_str!("golden/terrain_strokes/sides-L.txt"),
    ),
    (
        "sides-block2x2",
        include_str!("golden/terrain_strokes/sides-block2x2.txt"),
    ),
    (
        "sides-block2x2-path",
        include_str!("golden/terrain_strokes/sides-block2x2-path.txt"),
    ),
    (
        "sides-edge-line",
        include_str!("golden/terrain_strokes/sides-edge-line.txt"),
    ),
    (
        "sides-edge-single",
        include_str!("golden/terrain_strokes/sides-edge-single.txt"),
    ),
    (
        "sides-edge-vertical",
        include_str!("golden/terrain_strokes/sides-edge-vertical.txt"),
    ),
    (
        "sides-parallel",
        include_str!("golden/terrain_strokes/sides-parallel.txt"),
    ),
    (
        "sides-parallel-path",
        include_str!("golden/terrain_strokes/sides-parallel-path.txt"),
    ),
    (
        "sides-band3",
        include_str!("golden/terrain_strokes/sides-band3.txt"),
    ),
    (
        "sides-edge-band3",
        include_str!("golden/terrain_strokes/sides-edge-band3.txt"),
    ),
    (
        "sides-all",
        include_str!("golden/terrain_strokes/sides-all.txt"),
    ),
    (
        "blob-single",
        include_str!("golden/terrain_strokes/blob-single.txt"),
    ),
    (
        "blob-line5",
        include_str!("golden/terrain_strokes/blob-line5.txt"),
    ),
    ("blob-L", include_str!("golden/terrain_strokes/blob-L.txt")),
    (
        "blob-block2x2",
        include_str!("golden/terrain_strokes/blob-block2x2.txt"),
    ),
    (
        "blob-block2x2-path",
        include_str!("golden/terrain_strokes/blob-block2x2-path.txt"),
    ),
    (
        "blob-edge-line",
        include_str!("golden/terrain_strokes/blob-edge-line.txt"),
    ),
    (
        "blob-edge-single",
        include_str!("golden/terrain_strokes/blob-edge-single.txt"),
    ),
    (
        "blob-edge-vertical",
        include_str!("golden/terrain_strokes/blob-edge-vertical.txt"),
    ),
    (
        "blob-parallel",
        include_str!("golden/terrain_strokes/blob-parallel.txt"),
    ),
    (
        "blob-parallel-path",
        include_str!("golden/terrain_strokes/blob-parallel-path.txt"),
    ),
    (
        "blob-band3",
        include_str!("golden/terrain_strokes/blob-band3.txt"),
    ),
    (
        "blob-edge-band3",
        include_str!("golden/terrain_strokes/blob-edge-band3.txt"),
    ),
    (
        "blob-all",
        include_str!("golden/terrain_strokes/blob-all.txt"),
    ),
    (
        "corners-single",
        include_str!("golden/terrain_strokes/corners-single.txt"),
    ),
    (
        "corners-line5",
        include_str!("golden/terrain_strokes/corners-line5.txt"),
    ),
    (
        "corners-L",
        include_str!("golden/terrain_strokes/corners-L.txt"),
    ),
    (
        "corners-block2x2",
        include_str!("golden/terrain_strokes/corners-block2x2.txt"),
    ),
    (
        "corners-edge-line",
        include_str!("golden/terrain_strokes/corners-edge-line.txt"),
    ),
    (
        "corners-edge-single",
        include_str!("golden/terrain_strokes/corners-edge-single.txt"),
    ),
    (
        "corners-edge-vertical",
        include_str!("golden/terrain_strokes/corners-edge-vertical.txt"),
    ),
    (
        "corners-parallel",
        include_str!("golden/terrain_strokes/corners-parallel.txt"),
    ),
    (
        "corners-band3",
        include_str!("golden/terrain_strokes/corners-band3.txt"),
    ),
    (
        "corners-edge-band3",
        include_str!("golden/terrain_strokes/corners-edge-band3.txt"),
    ),
    (
        "corners-all",
        include_str!("golden/terrain_strokes/corners-all.txt"),
    ),
];
