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

use crate::solver::{MatchMode, Peer, TerrainSet, TerrainTile, TileTerrains};
use crate::{TerrainId, TileRef};

/// A tile sheet layout for an inner terrain over an outer terrain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LayoutTemplate {
    Corner16,
    Side16,
    Minimal3x3,
    Corner2x2,
    Blob47,
}

impl LayoutTemplate {
    pub const ALL: [LayoutTemplate; 5] = [
        LayoutTemplate::Corner16,
        LayoutTemplate::Side16,
        LayoutTemplate::Minimal3x3,
        LayoutTemplate::Corner2x2,
        LayoutTemplate::Blob47,
    ];

    pub const fn mode(self) -> MatchMode {
        match self {
            LayoutTemplate::Corner16 => MatchMode::Corners,
            LayoutTemplate::Side16 => MatchMode::Sides,
            LayoutTemplate::Minimal3x3 | LayoutTemplate::Corner2x2 | LayoutTemplate::Blob47 => {
                MatchMode::CornersAndSides
            }
        }
    }

    pub const fn tile_count(self) -> usize {
        match self {
            LayoutTemplate::Corner16 | LayoutTemplate::Side16 => 16,
            LayoutTemplate::Minimal3x3 => 9,
            LayoutTemplate::Corner2x2 => 4,
            LayoutTemplate::Blob47 => 47,
        }
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

    fn mask_index(cells: &[TerrainId; 9], outer: TerrainId, peers: [Peer; 4], corner: bool) -> u16 {
        let mut index = 0;
        for (bit, peer) in peers.iter().enumerate() {
            let hit = if corner {
                let [a, b] = peer.corner_sides().unwrap();
                [*peer, a, b].iter().any(|p| cells[p.index()] == outer)
            } else {
                cells[peer.index()] == outer
            };
            if hit {
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
            for mask in 0..=255u8 {
                let (layer, cells) = neighbourhood(mask, inner, inner, outer);
                let r = resolve_cell(&set, &layer, origin());
                assert!(r.is_exact(), "mask {mask}");
                let expected = mask_index(&cells, outer, Peer::CORNERS, true);
                assert_eq!(r.tile, TileRef::new(expected), "mask {mask}");
            }
        }
    }

    #[test]
    fn side16_resolves_exhaustively() {
        for (inner, outer) in PAIRS {
            let set = LayoutTemplate::Side16.terrain_set(inner, outer);
            for mask in 0..=255u8 {
                let (layer, cells) = neighbourhood(mask, inner, inner, outer);
                let r = resolve_cell(&set, &layer, origin());
                assert!(r.is_exact(), "mask {mask}");
                let expected = mask_index(&cells, outer, Peer::SIDES, false);
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
                let r = resolve_cell(&set, &layer, origin());
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
                        let a = resolve_cell(&plain, &layer, origin());
                        assert_eq!(resolve_cell(&ordered, &layer, origin()), a, "mask {mask}");
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
                let t = if peer == Peer::TopLeft { 1 } else { 2 };
                layer.set(CellCoord(IVec2::new(i * 4, 0) + peer.offset()), t);
            }
        }
        for i in 0..200 {
            let r = resolve_cell(&set, &layer, CellCoord::new(i * 4, 0));
            assert_eq!((r.score, r.max_score), (4, 5));
            assert_eq!(r.tile, TileRef::new(0), "cell {i}");
        }
    }

    /// Exact where the set has the neighbourhood's blob configuration, otherwise
    /// the best available score, never empty.
    fn assert_partial_blob(set: &TerrainSet, inner: TerrainId, outer: TerrainId) -> usize {
        let mut exact = 0;
        for mask in 0..=255u8 {
            let (layer, cells) = neighbourhood(mask, inner, inner, outer);
            let r = resolve_cell(set, &layer, origin());
            assert!(!r.tile.is_empty(), "mask {mask}");
            let config = blob_config(&cells, outer);
            let required = required_peers(set, &layer, origin());
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
        for template in LayoutTemplate::ALL {
            let set = template.terrain_set(1, 2);
            for mask in [0u8, 0x5a, 0xff] {
                let (layer, _) = neighbourhood(mask, 2, 1, 2);
                let r = resolve_cell(&set, &layer, origin());
                assert_eq!(r.tile, TileRef::EMPTY);
                assert_eq!(r.score, 0);
            }
        }
    }
}
