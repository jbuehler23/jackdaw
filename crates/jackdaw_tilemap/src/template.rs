//! Layout templates: peer terrains of common sheet layouts for an inner
//! terrain over an outer one. Tile index = grid position; every centre is
//! inner; ignored peers and wildcard blob corners hold the outer terrain.
//! - `Corner16` (16, `Corners`): mask of outer corners, bits TL, TR, BL, BR.
//! - `Side16` (16, `Sides`): mask of outer sides, bits T, L, R, B.
//! - `Minimal3x3` (9): a filled block row by row from the top, corners and
//!   edges around the centre.
//! - `Corner2x2` (4): one outer corner each, TL, TR, BL, BR.
//! - `Blob47` (47): per `Side16` side mask, a mask over the enclosed corners
//!   (both sides inner) in TL, TR, BL, BR order.
//! - `Minimal3x3Inner` (13): `Minimal3x3`, then `Corner2x2` as a 2x2 block to
//!   its right.
//!
//! `Minimal3x3`, `Corner2x2` and `Minimal3x3Inner` read either way: on cells
//! as drawn, or between cells by their four corners alone.

use std::collections::BTreeMap;

use glam::{IVec2, UVec2};

use crate::solver::{
    MatchMode, Peer, TerrainSet, TerrainTile, TileTerrains, resolve_region_within,
};
use crate::{CellCoord, CellRect, ChunkedLayer, Links, Placement, TerrainId, TileRef};

/// A tile sheet layout for an inner terrain over an outer terrain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutTemplate {
    Corner16,
    Side16,
    Minimal3x3,
    Corner2x2,
    Blob47,
    Minimal3x3Inner,
}

impl LayoutTemplate {
    pub const ALL: [LayoutTemplate; 6] = [
        LayoutTemplate::Corner16,
        LayoutTemplate::Side16,
        LayoutTemplate::Minimal3x3,
        LayoutTemplate::Corner2x2,
        LayoutTemplate::Blob47,
        LayoutTemplate::Minimal3x3Inner,
    ];

    pub const fn mode(self) -> MatchMode {
        match self {
            LayoutTemplate::Corner16 => MatchMode::Corners,
            LayoutTemplate::Side16 => MatchMode::Sides,
            LayoutTemplate::Minimal3x3
            | LayoutTemplate::Corner2x2
            | LayoutTemplate::Blob47
            | LayoutTemplate::Minimal3x3Inner => MatchMode::CornersAndSides,
        }
    }

    pub const fn tile_count(self) -> usize {
        match self {
            LayoutTemplate::Corner16 | LayoutTemplate::Side16 => 16,
            LayoutTemplate::Minimal3x3 => 9,
            LayoutTemplate::Corner2x2 => 4,
            LayoutTemplate::Blob47 => 47,
            LayoutTemplate::Minimal3x3Inner => 13,
        }
    }

    /// Columns and rows of the block its tiles are drawn in; `Blob47` takes
    /// rows of eight.
    pub const fn shape(self) -> UVec2 {
        match self {
            LayoutTemplate::Corner16 | LayoutTemplate::Side16 => UVec2::new(4, 4),
            LayoutTemplate::Minimal3x3 => UVec2::new(3, 3),
            LayoutTemplate::Corner2x2 => UVec2::new(2, 2),
            LayoutTemplate::Blob47 => UVec2::new(8, 6),
            LayoutTemplate::Minimal3x3Inner => UVec2::new(5, 3),
        }
    }

    /// Where its tiles can be drawn: corner layouts between cells, edge and
    /// blob layouts on cells, the 3x3 and 2x2 blocks either way.
    pub const fn placements(self) -> &'static [Placement] {
        match self {
            LayoutTemplate::Corner16 => &[Placement::BetweenCells],
            LayoutTemplate::Side16 | LayoutTemplate::Blob47 => &[Placement::OnCells],
            LayoutTemplate::Minimal3x3
            | LayoutTemplate::Corner2x2
            | LayoutTemplate::Minimal3x3Inner => &[Placement::BetweenCells, Placement::OnCells],
        }
    }

    /// The match mode of a set of its tiles drawn at `placement`.
    pub const fn mode_at(self, placement: Placement) -> MatchMode {
        match placement {
            Placement::BetweenCells => MatchMode::Corners,
            Placement::OnCells => self.mode(),
        }
    }

    /// Whether a block of `size` holds its tiles: in its own shape, or for
    /// layouts of 16 or more, row by row over two rows or more with only the
    /// last row short.
    pub fn fits(self, size: UVec2) -> bool {
        let (area, count) = (size.x * size.y, self.tile_count() as u32);
        let flows = count >= 16 && size.y > 1 && area >= count && area - count < size.x;
        size == self.shape() || flows
    }

    /// The cell of each tile in a block of `size`: in its own shape as drawn,
    /// otherwise row by row; `None` past the block.
    pub fn slots(self, size: UVec2) -> Vec<Option<UVec2>> {
        let width = size.x.max(1);
        (0..self.tile_count() as u32)
            .map(|i| {
                if size == self.shape() && self == LayoutTemplate::Minimal3x3Inner {
                    return Some(match i {
                        0..9 => UVec2::new(i % 3, i / 3),
                        _ => UVec2::new(3 + (i - 9) % 2, (i - 9) / 2),
                    });
                }
                let cell = UVec2::new(i % width, i / width);
                cell.cmplt(size).all().then_some(cell)
            })
            .collect()
    }

    /// Its tiles read at `placement`: between cells only the corners count,
    /// so the centre is inner and the sides outer, as in `Corner16`.
    pub fn placed_tiles(
        self,
        placement: Placement,
        inner: TerrainId,
        outer: TerrainId,
    ) -> Vec<TileTerrains> {
        let tiles = self.tiles(inner, outer);
        if placement == Placement::OnCells {
            return tiles;
        }
        (tiles.into_iter())
            .map(|t| {
                let corners = TileTerrains::filled(outer).with(Peer::Centre, inner);
                (Peer::CORNERS.into_iter()).fold(corners, |c, p| c.with(p, t.get(p)))
            })
            .collect()
    }

    /// A set of the tiles a block of `size` holds read at `placement`, tile
    /// index = template index, weight 1.
    pub fn placed_set(
        self,
        placement: Placement,
        inner: TerrainId,
        outer: TerrainId,
        size: UVec2,
    ) -> TerrainSet {
        let mut set = TerrainSet::new(self.mode_at(placement));
        set.tiles = (self.placed_tiles(placement, inner, outer).into_iter())
            .zip(self.slots(size))
            .enumerate()
            .filter(|(_, (_, slot))| slot.is_some())
            .map(|(i, (terrains, _))| TerrainTile::new(TileRef::new(i as u16), terrains, 1))
            .collect();
        set
    }

    /// Peer terrains of each tile in grid order; see the module docs.
    pub fn tiles(self, inner: TerrainId, outer: TerrainId) -> Vec<TileTerrains> {
        let masked = |peers: [Peer; 4], mask: usize| {
            let mut t = TileTerrains::filled(outer).with(Peer::Centre, inner);
            for (bit, peer) in peers.iter().enumerate() {
                if mask & (1 << bit) == 0 {
                    t = t.with(*peer, inner);
                }
            }
            t
        };
        match self {
            LayoutTemplate::Corner16 => (0..16).map(|m| masked(Peer::CORNERS, m)).collect(),
            LayoutTemplate::Side16 => (0..16).map(|m| masked(Peer::SIDES, m)).collect(),
            LayoutTemplate::Minimal3x3 => [
                [Peer::Top, Peer::Left].as_slice(),
                &[Peer::Top],
                &[Peer::Top, Peer::Right],
                &[Peer::Left],
                &[],
                &[Peer::Right],
                &[Peer::Bottom, Peer::Left],
                &[Peer::Bottom],
                &[Peer::Bottom, Peer::Right],
            ]
            .iter()
            .map(|sides| blob_tile(inner, outer, sides, &[]))
            .collect(),
            LayoutTemplate::Corner2x2 => Peer::CORNERS
                .iter()
                .map(|corner| blob_tile(inner, outer, &[], &[*corner]))
                .collect(),
            LayoutTemplate::Blob47 => {
                let mut tiles = Vec::with_capacity(47);
                for side_mask in 0..16 {
                    let sides = masked(Peer::SIDES, side_mask);
                    let outer_sides: Vec<Peer> = Peer::SIDES
                        .into_iter()
                        .filter(|p| sides.get(*p) == outer)
                        .collect();
                    let free: Vec<Peer> = Peer::CORNERS
                        .into_iter()
                        .filter(|c| sides.corner_enclosed(*c))
                        .collect();
                    for corner_mask in 0..1usize << free.len() {
                        let outer_corners: Vec<Peer> = free
                            .iter()
                            .enumerate()
                            .filter(|(bit, _)| corner_mask & (1 << bit) != 0)
                            .map(|(_, c)| *c)
                            .collect();
                        tiles.push(blob_tile(inner, outer, &outer_sides, &outer_corners));
                    }
                }
                tiles
            }
            LayoutTemplate::Minimal3x3Inner => {
                let mut tiles = LayoutTemplate::Minimal3x3.tiles(inner, outer);
                tiles.extend(LayoutTemplate::Corner2x2.tiles(inner, outer));
                tiles
            }
        }
    }

    /// A set of this template's tiles, tile index = grid position, weight 1.
    pub fn terrain_set(self, inner: TerrainId, outer: TerrainId) -> TerrainSet {
        let mut set = TerrainSet::new(self.mode());
        set.tiles = self
            .tiles(inner, outer)
            .into_iter()
            .enumerate()
            .map(|(i, terrains)| TerrainTile::new(TileRef::new(i as u16), terrains, 1))
            .collect();
        set
    }
}

/// A small map a set is shown painting: one inner cell, a thin path, a turn
/// or an area, on a square of outer [`PaintPreview::SIZE`] cells across.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PaintPreview {
    Spot,
    ThinPath,
    Turn,
    Area,
}

impl PaintPreview {
    pub const ALL: [PaintPreview; 4] = [
        PaintPreview::Spot,
        PaintPreview::ThinPath,
        PaintPreview::Turn,
        PaintPreview::Area,
    ];
    pub const SIZE: i32 = 5;

    pub const fn name(self) -> &'static str {
        match self {
            PaintPreview::Spot => "spot",
            PaintPreview::ThinPath => "thin path",
            PaintPreview::Turn => "turn",
            PaintPreview::Area => "area",
        }
    }

    /// The inner cells, y up.
    pub fn cells(self) -> Vec<IVec2> {
        let line = |cells: &[(i32, i32)]| cells.iter().map(|&(x, y)| IVec2::new(x, y)).collect();
        match self {
            PaintPreview::Spot => line(&[(2, 2)]),
            PaintPreview::ThinPath => line(&[(1, 2), (2, 2), (3, 2)]),
            PaintPreview::Turn => line(&[(1, 3), (1, 2), (1, 1), (2, 1), (3, 1)]),
            PaintPreview::Area => (1..4)
                .flat_map(|y| (1..4).map(move |x| IVec2::new(x, y)))
                .collect(),
        }
    }

    /// The tiles `set` draws for it with `inner` painted over `outer`, by
    /// key, bounded by the square. Plain outer ground draws nothing, as a
    /// layout holds no tile for it.
    pub fn resolve(
        self,
        set: &TerrainSet,
        inner: TerrainId,
        outer: TerrainId,
    ) -> Vec<(CellCoord, TileRef)> {
        let mut terrain = ChunkedLayer::new();
        let all: Vec<CellCoord> = (0..Self::SIZE)
            .flat_map(|y| (0..Self::SIZE).map(move |x| CellCoord::new(x, y)))
            .collect();
        for cell in &all {
            terrain.set(*cell, outer);
        }
        for cell in self.cells() {
            terrain.set(CellCoord(cell), inner);
        }
        let bounds = CellRect {
            min: IVec2::ZERO,
            max: IVec2::splat(Self::SIZE),
        };
        let inside = self.cells();
        let touches = |key: CellCoord| {
            let corners = [IVec2::ZERO, IVec2::X, IVec2::Y, IVec2::ONE];
            corners.iter().any(|c| inside.contains(&(key.0 + *c)))
        };
        let between = set.mode.placement() == Placement::BetweenCells;
        let links = ChunkedLayer::<Links>::new();
        (resolve_region_within(set, &terrain, &links, &BTreeMap::new(), all, Some(bounds)))
            .into_iter()
            .filter(|(key, tile)| !tile.is_empty() && (!between || touches(*key)))
            .collect()
    }
}

/// A `CornersAndSides` tile with the given outer sides and outer free corners;
/// corners next to an outer side are wildcards and hold the outer terrain.
fn blob_tile(
    inner: TerrainId,
    outer: TerrainId,
    outer_sides: &[Peer],
    outer_corners: &[Peer],
) -> TileTerrains {
    let mut t = TileTerrains::filled(inner);
    for side in outer_sides {
        t = t.with(*side, outer);
    }
    for corner in Peer::CORNERS {
        if !t.corner_enclosed(corner) || outer_corners.contains(&corner) {
            t = t.with(corner, outer);
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::{match_score, required_peers, resolve_cell};
    use crate::{CellCoord, ChunkedLayer};
    use glam::IVec2;

    const NEIGHBOURS: [Peer; 8] = [
        Peer::TopLeft,
        Peer::Top,
        Peer::TopRight,
        Peer::Left,
        Peer::Right,
        Peer::BottomLeft,
        Peer::Bottom,
        Peer::BottomRight,
    ];
    const PAIRS: [(TerrainId, TerrainId); 2] = [(2, 1), (1, 0)];

    /// Centre `centre` at the origin, neighbour `k` outer when bit `k` of `mask` is set.
    fn neighbourhood(
        mask: u8,
        centre: TerrainId,
        inner: TerrainId,
        outer: TerrainId,
    ) -> (ChunkedLayer<TerrainId>, [TerrainId; 9]) {
        let mut layer = ChunkedLayer::new();
        let mut cells = [inner; 9];
        cells[Peer::Centre.index()] = centre;
        layer.set(CellCoord::new(0, 0), centre);
        for (k, peer) in NEIGHBOURS.iter().enumerate() {
            let t = if mask & (1 << k) != 0 { outer } else { inner };
            cells[peer.index()] = t;
            layer.set(CellCoord(peer.offset()), t);
        }
        (layer, cells)
    }

    /// The blob configuration of a neighbourhood: wildcard corners become outer.
    fn blob_config(cells: &[TerrainId; 9], outer: TerrainId) -> TileTerrains {
        let mut t = TileTerrains { peers: *cells };
        for corner in Peer::CORNERS {
            if !t.corner_enclosed(corner) {
                t = t.with(corner, outer);
            }
        }
        t
    }

    fn side_mask(cells: &[TerrainId; 9], outer: TerrainId) -> u16 {
        let mut index = 0;
        for (bit, peer) in Peer::SIDES.iter().enumerate() {
            if cells[peer.index()] == outer {
                index |= 1 << bit;
            }
        }
        index
    }

    /// Outer-corner mask of the tile between cells keyed at the origin: its
    /// corners are the origin and its +x, +y and +xy neighbours.
    fn corner_mask(cells: &[TerrainId; 9], outer: TerrainId) -> u16 {
        let corners = [Peer::Top, Peer::TopRight, Peer::Centre, Peer::Right];
        let mut index = 0;
        for (bit, peer) in corners.iter().enumerate() {
            if cells[peer.index()] == outer {
                index |= 1 << bit;
            }
        }
        index
    }

    fn origin() -> CellCoord {
        CellCoord::new(0, 0)
    }

    #[test]
    fn tile_counts_match_layout() {
        for template in LayoutTemplate::ALL {
            for (inner, outer) in PAIRS {
                let tiles = template.tiles(inner, outer);
                assert_eq!(tiles.len(), template.tile_count(), "{template:?}");
                assert!(tiles.iter().all(|t| t.centre() == inner));
                for t in &tiles {
                    for peer in Peer::ALL {
                        if !template.mode().uses(peer) {
                            assert_eq!(t.get(peer), outer, "{template:?} {peer:?}");
                        }
                    }
                }
                for (i, a) in tiles.iter().enumerate() {
                    assert!(!tiles[i + 1..].contains(a), "{template:?} duplicate {i}");
                }
            }
        }
    }

    #[test]
    fn corner16_resolves_exhaustively() {
        for (inner, outer) in PAIRS {
            let set = LayoutTemplate::Corner16.terrain_set(inner, outer);
            for (mask, centre) in (0..=255u8).flat_map(|m| [(m, inner), (m, outer)]) {
                let (layer, cells) = neighbourhood(mask, centre, inner, outer);
                let r = resolve_cell(&set, &layer, &ChunkedLayer::new(), origin());
                assert!(r.is_exact(), "mask {mask}");
                let expected = match corner_mask(&cells, outer) {
                    15 if outer == 0 => TileRef::EMPTY,
                    bits => TileRef::new(bits),
                };
                assert_eq!(r.tile, expected, "mask {mask}");
            }
        }
    }

    #[test]
    fn side16_resolves_exhaustively() {
        for (inner, outer) in PAIRS {
            let set = LayoutTemplate::Side16.terrain_set(inner, outer);
            for mask in 0..=255u8 {
                let (layer, cells) = neighbourhood(mask, inner, inner, outer);
                let r = resolve_cell(&set, &layer, &ChunkedLayer::new(), origin());
                assert!(r.is_exact(), "mask {mask}");
                let expected = side_mask(&cells, outer);
                assert_eq!(r.tile, TileRef::new(expected), "mask {mask}");
            }
        }
    }

    #[test]
    fn blob47_resolves_exhaustively() {
        for (inner, outer) in PAIRS {
            let tiles = LayoutTemplate::Blob47.tiles(inner, outer);
            let set = LayoutTemplate::Blob47.terrain_set(inner, outer);
            for mask in 0..=255u8 {
                let (layer, cells) = neighbourhood(mask, inner, inner, outer);
                let r = resolve_cell(&set, &layer, &ChunkedLayer::new(), origin());
                assert!(r.is_exact(), "mask {mask}");
                let config = blob_config(&cells, outer);
                let expected = tiles.iter().position(|t| *t == config).unwrap();
                assert_eq!(r.tile, TileRef::new(expected as u16), "mask {mask}");
            }
        }
    }

    #[test]
    fn orders_with_direct_transitions_match_unordered_results() {
        for (inner, outer) in PAIRS {
            for template in [LayoutTemplate::Blob47, LayoutTemplate::Side16] {
                let plain = template.terrain_set(inner, outer);
                for order in [vec![outer, 5, inner], vec![inner, outer], vec![]] {
                    let ordered = plain.clone().with_order(order);
                    for mask in 0..=255u8 {
                        let (layer, _) = neighbourhood(mask, inner, inner, outer);
                        let a = resolve_cell(&plain, &layer, &ChunkedLayer::new(), origin());
                        assert_eq!(
                            resolve_cell(&ordered, &layer, &ChunkedLayer::new(), origin()),
                            a,
                            "mask {mask}"
                        );
                        assert!(a.is_exact());
                    }
                }
            }
        }
    }

    #[test]
    fn corner16_fallback_picks_one_shape_everywhere() {
        let mut set = LayoutTemplate::Corner16.terrain_set(2, 1);
        set.tiles.remove(1);
        let mut layer = ChunkedLayer::new();
        for i in 0..200 {
            for peer in Peer::ALL {
                let t = if peer == Peer::Top { 1 } else { 2 };
                layer.set(CellCoord(IVec2::new(i * 4, 0) + peer.offset()), t);
            }
        }
        for i in 0..200 {
            let r = resolve_cell(&set, &layer, &ChunkedLayer::new(), CellCoord::new(i * 4, 0));
            assert_eq!((r.score, r.max_score), (3, 4));
            assert_eq!(r.tile, TileRef::new(0), "cell {i}");
        }
    }

    /// Exact where the set has the neighbourhood's blob configuration, otherwise
    /// the best available score, never empty.
    fn assert_partial_blob(set: &TerrainSet, inner: TerrainId, outer: TerrainId) -> usize {
        let mut exact = 0;
        for mask in 0..=255u8 {
            let (layer, cells) = neighbourhood(mask, inner, inner, outer);
            let r = resolve_cell(set, &layer, &ChunkedLayer::new(), origin());
            assert!(!r.tile.is_empty(), "mask {mask}");
            let config = blob_config(&cells, outer);
            let required = required_peers(set, &layer, &ChunkedLayer::new(), origin());
            let best = set
                .tiles
                .iter()
                .map(|t| match_score(&required, &t.terrains))
                .max()
                .unwrap();
            assert_eq!(r.score, best, "mask {mask}");
            let chosen = &set.tiles[r.tile.index() as usize];
            assert_eq!(match_score(&required, &chosen.terrains), r.score);
            match set.tiles.iter().position(|t| t.terrains == config) {
                Some(i) => {
                    assert!(r.is_exact(), "mask {mask}");
                    assert_eq!(r.tile, TileRef::new(i as u16), "mask {mask}");
                    exact += 1;
                }
                None => assert!(!r.is_exact(), "mask {mask}"),
            }
        }
        exact
    }

    #[test]
    fn minimal_3x3_and_corner_2x2_fall_back_by_score() {
        for (inner, outer) in PAIRS {
            let minimal = LayoutTemplate::Minimal3x3.terrain_set(inner, outer);
            let corners = LayoutTemplate::Corner2x2.terrain_set(inner, outer);
            let mut combined = minimal.clone();
            for (i, t) in corners.tiles.iter().enumerate() {
                let tile = TileRef::new((minimal.tiles.len() + i) as u16);
                combined.tiles.push(TerrainTile::new(tile, t.terrains, 1));
            }
            let a = assert_partial_blob(&minimal, inner, outer);
            let b = assert_partial_blob(&corners, inner, outer);
            let c = assert_partial_blob(&combined, inner, outer);
            assert!(a > 0 && b > 0, "{a} {b}");
            assert_eq!(c, a + b);
        }
    }

    #[test]
    fn outer_centre_without_tiles_resolves_empty() {
        let on_cells = |t: &LayoutTemplate| t.mode().placement() == crate::Placement::OnCells;
        for template in LayoutTemplate::ALL.iter().copied().filter(on_cells) {
            let set = template.terrain_set(1, 2);
            for mask in [0u8, 0x5a, 0xff] {
                let (layer, _) = neighbourhood(mask, 2, 1, 2);
                let r = resolve_cell(&set, &layer, &ChunkedLayer::new(), origin());
                assert_eq!(r.tile, TileRef::EMPTY);
                assert_eq!(r.score, 0);
            }
        }
    }

    fn fitting(size: UVec2) -> Vec<LayoutTemplate> {
        (LayoutTemplate::ALL.into_iter())
            .filter(|t| t.fits(size))
            .collect()
    }

    #[test]
    fn a_block_size_fits_the_layouts_it_can_hold() {
        use LayoutTemplate::*;
        assert_eq!(fitting(UVec2::splat(4)), [Corner16, Side16]);
        assert_eq!(fitting(UVec2::splat(3)), [Minimal3x3]);
        assert_eq!(fitting(UVec2::new(12, 4)), [Blob47]);
        assert_eq!(fitting(UVec2::new(8, 6)), [Blob47]);
        assert_eq!(fitting(UVec2::new(5, 3)), [Minimal3x3Inner]);
        assert!(fitting(UVec2::new(3, 2)).is_empty());
        assert!(fitting(UVec2::new(47, 1)).is_empty(), "a strip is no blob");
        assert!(fitting(UVec2::new(16, 1)).is_empty());
    }

    #[test]
    fn the_inner_corners_sit_beside_the_3x3() {
        let slots = LayoutTemplate::Minimal3x3Inner.slots(UVec2::new(5, 3));
        assert_eq!(slots[8], Some(UVec2::new(2, 2)));
        assert_eq!(
            slots[9..],
            [
                Some(UVec2::new(3, 0)),
                Some(UVec2::new(4, 0)),
                Some(UVec2::new(3, 1)),
                Some(UVec2::new(4, 1))
            ]
        );
        let flowed = LayoutTemplate::Blob47.slots(UVec2::new(12, 4));
        assert_eq!(flowed[13], Some(UVec2::new(1, 1)));
        let short = LayoutTemplate::Corner16.slots(UVec2::new(4, 3));
        assert_eq!(short.iter().filter(|s| s.is_none()).count(), 4);
    }

    #[test]
    fn ground_draws_a_thin_path_that_walls_cannot() {
        let size = UVec2::splat(3);
        let minimal = LayoutTemplate::Minimal3x3;
        let exact = |placement: Placement| {
            let set = minimal.placed_set(placement, 2, 1, size);
            let drawn = PaintPreview::ThinPath.resolve(&set, 2, 1);
            let mut terrain = ChunkedLayer::new();
            for y in 0..PaintPreview::SIZE {
                for x in 0..PaintPreview::SIZE {
                    terrain.set(CellCoord::new(x, y), 1);
                }
            }
            for cell in PaintPreview::ThinPath.cells() {
                terrain.set(CellCoord(cell), 2);
            }
            assert!(!drawn.is_empty());
            (drawn.iter()).all(|(key, tile)| {
                resolve_cell(&set, &terrain, &ChunkedLayer::new(), *key).is_exact()
                    && !tile.is_empty()
            })
        };
        assert!(exact(Placement::BetweenCells), "ground draws it");
        assert!(!exact(Placement::OnCells), "walls lack a 1-wide tile");
    }
}
