//! Terrain solver: turns the terrain ids around a cell into a tile.
//!
//! Tiles carry nine peers, row-major with the top (+y) row first. Required
//! peers come from the cell's 3x3 block; cells off the map or unpainted read 0.
//! - Centre: the cell's terrain. Side: the neighbour across that side.
//! - Corner, `CornersAndSides`: the diagonal neighbour when both adjacent sides
//!   equal the centre, otherwise a wildcard.
//! - Corner, `Corners`: the vertex terrain, the lowest of the four cells
//!   sharing that vertex by [`TerrainSet::rank`].
//!
//! In-between terrains are visual only. When A and B have no transition tiles
//! either way on that peer class (side or corner), a cell of A matches peer B
//! as the terrain next to A in the order towards B, if A has tiles for it.
//! Cells sharing a vertex agree on its terrain before this per-cell step, so
//! in `Corners` mode only the higher cell draws the band.
//!
//! Candidates share the cell's centre and score one point per matched required
//! peer. Ties go to more matched sides, then a positive weight, then set order;
//! tiles with that exact shape are then picked by weight from a cell hash.
//! Hand overrides are returned unchanged by [`resolve_region`].

use std::collections::{BTreeMap, BTreeSet};

use glam::IVec2;

use crate::{CellCoord, ChunkedLayer, TerrainId, TileRef};

/// One of the nine peer positions of a tile, row-major with the top (+y) row first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Peer {
    TopLeft,
    Top,
    TopRight,
    Left,
    Centre,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

impl Peer {
    pub const ALL: [Peer; 9] = [
        Peer::TopLeft,
        Peer::Top,
        Peer::TopRight,
        Peer::Left,
        Peer::Centre,
        Peer::Right,
        Peer::BottomLeft,
        Peer::Bottom,
        Peer::BottomRight,
    ];
    pub const CORNERS: [Peer; 4] = [
        Peer::TopLeft,
        Peer::TopRight,
        Peer::BottomLeft,
        Peer::BottomRight,
    ];
    pub const SIDES: [Peer; 4] = [Peer::Top, Peer::Left, Peer::Right, Peer::Bottom];

    pub const fn index(self) -> usize {
        self as usize
    }

    /// Offset from the cell to the neighbour this peer faces.
    pub const fn offset(self) -> IVec2 {
        let i = self as i32;
        IVec2::new(i % 3 - 1, 1 - i / 3)
    }

    /// The two side peers next to a corner, or `None` for a side or the centre.
    pub const fn corner_sides(self) -> Option<[Peer; 2]> {
        match self {
            Peer::TopLeft => Some([Peer::Top, Peer::Left]),
            Peer::TopRight => Some([Peer::Top, Peer::Right]),
            Peer::BottomLeft => Some([Peer::Bottom, Peer::Left]),
            Peer::BottomRight => Some([Peer::Bottom, Peer::Right]),
            Peer::Top | Peer::Left | Peer::Centre | Peer::Right | Peer::Bottom => None,
        }
    }
}

/// Terrain on each of a tile's nine peers, indexed by [`Peer::index`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TileTerrains {
    pub peers: [TerrainId; 9],
}

impl TileTerrains {
    /// Every peer set to `terrain`.
    pub const fn filled(terrain: TerrainId) -> Self {
        Self {
            peers: [terrain; 9],
        }
    }

    pub const fn get(&self, peer: Peer) -> TerrainId {
        self.peers[peer.index()]
    }

    pub const fn with(mut self, peer: Peer, terrain: TerrainId) -> Self {
        self.peers[peer.index()] = terrain;
        self
    }

    pub const fn centre(&self) -> TerrainId {
        self.get(Peer::Centre)
    }

    /// True for a corner whose two adjacent sides both equal the centre.
    pub const fn corner_enclosed(&self, corner: Peer) -> bool {
        match corner.corner_sides() {
            Some([a, b]) => self.get(a) == self.centre() && self.get(b) == self.centre(),
            None => false,
        }
    }
}

/// Which peers take part in matching.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MatchMode {
    Corners,
    Sides,
    CornersAndSides,
}

impl MatchMode {
    pub const fn uses_sides(self) -> bool {
        matches!(self, MatchMode::Sides | MatchMode::CornersAndSides)
    }

    pub const fn uses_corners(self) -> bool {
        matches!(self, MatchMode::Corners | MatchMode::CornersAndSides)
    }

    pub const fn uses(self, peer: Peer) -> bool {
        if peer.corner_sides().is_some() {
            self.uses_corners()
        } else if matches!(peer, Peer::Centre) {
            true
        } else {
            self.uses_sides()
        }
    }
}

/// A tile in a terrain set with its peer terrains and variant weight.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainTile {
    pub tile: TileRef,
    pub terrains: TileTerrains,
    /// Relative chance among tiles of the same shape; 0 is never auto-picked
    /// while a positive tile ties.
    pub weight: u32,
}

impl TerrainTile {
    pub const fn new(tile: TileRef, terrains: TileTerrains, weight: u32) -> Self {
        Self {
            tile,
            terrains,
            weight,
        }
    }
}

/// The tiles the solver chooses from, matched under one mode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerrainSet {
    pub mode: MatchMode,
    pub tiles: Vec<TerrainTile>,
    /// Terrain stacking, bottom first. Unlisted terrains rank above every
    /// listed one, by id; empty (0) is always lowest.
    pub order: Vec<TerrainId>,
}

impl TerrainSet {
    pub const fn new(mode: MatchMode) -> Self {
        Self {
            mode,
            tiles: Vec::new(),
            order: Vec::new(),
        }
    }

    /// Sets the stacking order, bottom first; 0 and repeated ids are dropped,
    /// keeping the first occurrence.
    pub fn with_order(mut self, order: impl IntoIterator<Item = TerrainId>) -> Self {
        self.order.clear();
        for terrain in order {
            if terrain != 0 && !self.order.contains(&terrain) {
                self.order.push(terrain);
            }
        }
        self
    }

    /// Stacking rank of `terrain`; lower ranks sit underneath.
    pub fn rank(&self, terrain: TerrainId) -> usize {
        rank_at(
            self.order.iter().position(|t| *t == terrain),
            self.order.len(),
            terrain,
        )
    }

    pub fn with_tile(mut self, tile: TerrainTile) -> Self {
        self.tiles.push(tile);
        self
    }
}

fn rank_at(position: Option<usize>, len: usize, terrain: TerrainId) -> usize {
    if terrain == 0 {
        return 0;
    }
    match position {
        Some(i) => i + 1,
        None => len + usize::from(terrain),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum PeerClass {
    Side,
    Corner,
}

impl PeerClass {
    const fn of(peer: Peer) -> Self {
        if peer.corner_sides().is_some() {
            PeerClass::Corner
        } else {
            PeerClass::Side
        }
    }
}

/// Order positions and transitions of a set, built once per resolve.
struct Lookup<'a> {
    set: &'a TerrainSet,
    positions: BTreeMap<TerrainId, usize>,
    /// (centre, carried terrain, class) for every used non-centre peer.
    transitions: BTreeSet<(TerrainId, TerrainId, PeerClass)>,
}

impl<'a> Lookup<'a> {
    fn new(set: &'a TerrainSet) -> Self {
        let mut positions = BTreeMap::new();
        for (i, terrain) in set.order.iter().enumerate() {
            positions.entry(*terrain).or_insert(i);
        }
        let mut transitions = BTreeSet::new();
        for tile in &set.tiles {
            for peer in Peer::ALL {
                if peer != Peer::Centre && set.mode.uses(peer) {
                    let centre = tile.terrains.centre();
                    transitions.insert((centre, tile.terrains.get(peer), PeerClass::of(peer)));
                }
            }
        }
        Self {
            set,
            positions,
            transitions,
        }
    }

    fn rank(&self, terrain: TerrainId) -> usize {
        let position = self.positions.get(&terrain).copied();
        rank_at(position, self.set.order.len(), terrain)
    }

    fn carries(&self, centre: TerrainId, terrain: TerrainId, class: PeerClass) -> bool {
        self.transitions.contains(&(centre, terrain, class))
    }

    /// What a cell of `centre` matches for a `class` peer holding `peer`; see
    /// the module docs.
    fn substitute(&self, centre: TerrainId, peer: TerrainId, class: PeerClass) -> TerrainId {
        if centre == peer || centre == 0 || peer == 0 {
            return peer;
        }
        let (Some(&a), Some(&b)) = (self.positions.get(&centre), self.positions.get(&peer)) else {
            return peer;
        };
        let step = if a < b { a + 1 } else { a - 1 };
        let Some(&between) = self.set.order.get(step) else {
            return peer;
        };
        let direct = self.carries(centre, peer, class) || self.carries(peer, centre, class);
        if step == b || direct || !self.carries(centre, between, class) {
            return peer;
        }
        between
    }
}

/// Required terrain per peer; `None` is a wildcard.
pub type RequiredPeers = [Option<TerrainId>; 9];

/// The tile chosen for a cell and how well it matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resolved {
    pub tile: TileRef,
    /// Required peers the tile matches, the centre included.
    pub score: u8,
    /// Number of required peers.
    pub max_score: u8,
}

impl Resolved {
    pub const fn is_exact(&self) -> bool {
        self.score == self.max_score
    }
}

/// Required peers for `cell`; see the module docs for the rules. Builds the
/// set's lookups per call; [`resolve_region`] builds them once.
pub fn required_peers(
    set: &TerrainSet,
    terrain: &ChunkedLayer<TerrainId>,
    cell: CellCoord,
) -> RequiredPeers {
    required_from_block(&Lookup::new(set), &read_block(terrain, cell))
}

fn neighbour(cell: CellCoord, offset: IVec2) -> Option<CellCoord> {
    cell.0.checked_add(offset).map(CellCoord)
}

/// The 3x3 block around `cell`, indexed by [`Peer::index`].
fn read_block(terrain: &ChunkedLayer<TerrainId>, cell: CellCoord) -> TileTerrains {
    let mut block = TileTerrains::filled(0);
    for peer in Peer::ALL {
        if let Some(n) = neighbour(cell, peer.offset()) {
            block = block.with(peer, terrain.get(n));
        }
    }
    block
}

/// Terrain of the vertex at `corner`: the lowest ranked of the four cells sharing it.
fn vertex_terrain(lookup: &Lookup, block: &TileTerrains, corner: Peer) -> TerrainId {
    let sides = corner.corner_sides().unwrap_or([corner; 2]);
    [Peer::Centre, sides[0], sides[1], corner]
        .into_iter()
        .map(|p| block.get(p))
        .min_by_key(|t| lookup.rank(*t))
        .unwrap_or(0)
}

fn required_from_block(lookup: &Lookup, block: &TileTerrains) -> RequiredPeers {
    let mode = lookup.set.mode;
    let mut required = [None; 9];
    required[Peer::Centre.index()] = Some(block.centre());
    if mode.uses_sides() {
        for side in Peer::SIDES {
            required[side.index()] = Some(block.get(side));
        }
    }
    for corner in Peer::CORNERS {
        required[corner.index()] = match mode {
            MatchMode::Sides => None,
            MatchMode::Corners => Some(vertex_terrain(lookup, block, corner)),
            MatchMode::CornersAndSides => {
                block.corner_enclosed(corner).then_some(block.get(corner))
            }
        };
    }
    for peer in Peer::ALL {
        let class = PeerClass::of(peer);
        let slot = &mut required[peer.index()];
        *slot = slot.map(|t| lookup.substitute(block.centre(), t, class));
    }
    required
}

/// Number of required peers `tile` matches.
pub fn match_score(required: &RequiredPeers, tile: &TileTerrains) -> u8 {
    count_matches(required, tile, &Peer::ALL)
}

fn count_matches(required: &RequiredPeers, tile: &TileTerrains, peers: &[Peer]) -> u8 {
    let hits = peers
        .iter()
        .filter(|p| required[p.index()] == Some(tile.get(**p)))
        .count();
    hits as u8
}

/// Resolves one cell from its neighbourhood. Builds the set's lookups per
/// call; [`resolve_region`] builds them once.
pub fn resolve_cell(
    set: &TerrainSet,
    terrain: &ChunkedLayer<TerrainId>,
    cell: CellCoord,
) -> Resolved {
    resolve_with(&Lookup::new(set), terrain, cell)
}

fn resolve_with(lookup: &Lookup, terrain: &ChunkedLayer<TerrainId>, cell: CellCoord) -> Resolved {
    let set = lookup.set;
    let block = read_block(terrain, cell);
    let centre = block.centre();
    if centre == 0 {
        return Resolved {
            tile: TileRef::EMPTY,
            score: 0,
            max_score: 0,
        };
    }
    let required = required_from_block(lookup, &block);
    let max_score = required.iter().filter(|r| r.is_some()).count() as u8;
    let mut best: Option<(&TerrainTile, (u8, u8, bool))> = None;
    for tile in set.tiles.iter().filter(|t| t.terrains.centre() == centre) {
        let key = (
            match_score(&required, &tile.terrains),
            count_matches(&required, &tile.terrains, &Peer::SIDES),
            tile.weight > 0,
        );
        if best.is_none_or(|(_, k)| key > k) {
            best = Some((tile, key));
        }
    }
    let Some((shape, (score, _, _))) = best else {
        return Resolved {
            tile: TileRef::EMPTY,
            score: 0,
            max_score,
        };
    };
    let variants = || set.tiles.iter().filter(|t| t.terrains == shape.terrains);
    let total: u64 = variants().map(|t| u64::from(t.weight)).sum();
    let mut tile = shape.tile;
    if total > 0 {
        let mut roll = cell_hash(cell) % total;
        for variant in variants() {
            let weight = u64::from(variant.weight);
            if roll < weight {
                tile = variant.tile;
                break;
            }
            roll -= weight;
        }
    }
    Resolved {
        tile,
        score,
        max_score,
    }
}

/// Stable per-cell hash (splitmix64 of the packed coordinate).
fn cell_hash(cell: CellCoord) -> u64 {
    let packed = (u64::from(cell.0.x as u32) << 32) | u64::from(cell.0.y as u32);
    let mut z = packed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Resolves every dirty cell and its eight neighbours, deduplicated and sorted.
/// Neighbours past the coordinate range are skipped; override cells return the override.
pub fn resolve_region(
    set: &TerrainSet,
    terrain: &ChunkedLayer<TerrainId>,
    overrides: &BTreeMap<CellCoord, TileRef>,
    dirty: impl IntoIterator<Item = CellCoord>,
) -> Vec<(CellCoord, TileRef)> {
    let lookup = Lookup::new(set);
    let mut cells = BTreeSet::new();
    for cell in dirty {
        for peer in Peer::ALL {
            cells.extend(neighbour(cell, peer.offset()));
        }
    }
    cells
        .into_iter()
        .map(|cell| {
            let tile = overrides
                .get(&cell)
                .copied()
                .unwrap_or_else(|| resolve_with(&lookup, terrain, cell).tile);
            (cell, tile)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LayoutTemplate;

    fn layer(cells: &[(i32, i32, TerrainId)]) -> ChunkedLayer<TerrainId> {
        let mut layer = ChunkedLayer::new();
        for &(x, y, t) in cells {
            layer.set(CellCoord::new(x, y), t);
        }
        layer
    }

    fn variant_set(weights: &[u32]) -> TerrainSet {
        let mut set = TerrainSet::new(MatchMode::Sides);
        for (i, &w) in weights.iter().enumerate() {
            set = set.with_tile(TerrainTile::new(
                TileRef::new(i as u16),
                TileTerrains::filled(1).with(Peer::Top, 0),
                w,
            ));
        }
        set
    }

    fn pick_counts(set: &TerrainSet, n: usize) -> Vec<usize> {
        let mut counts = vec![0; set.tiles.len()];
        let mut terrain = ChunkedLayer::new();
        for i in 0..n {
            terrain.set(CellCoord::new(i as i32 * 2, 0), 1);
        }
        for i in 0..n {
            let r = resolve_cell(set, &terrain, CellCoord::new(i as i32 * 2, 0));
            counts[r.tile.index() as usize] += 1;
        }
        counts
    }

    #[test]
    fn peer_offsets_are_y_up_row_major() {
        assert_eq!(Peer::TopLeft.offset(), IVec2::new(-1, 1));
        assert_eq!(Peer::Top.offset(), IVec2::new(0, 1));
        assert_eq!(Peer::Centre.offset(), IVec2::ZERO);
        assert_eq!(Peer::Right.offset(), IVec2::new(1, 0));
        assert_eq!(Peer::BottomRight.offset(), IVec2::new(1, -1));
        for (i, p) in Peer::ALL.iter().enumerate() {
            assert_eq!(p.index(), i);
        }
    }

    #[test]
    fn mode_peer_usage() {
        let count = |m: MatchMode| Peer::ALL.iter().filter(|p| m.uses(**p)).count();
        assert_eq!(count(MatchMode::Corners), 5);
        assert_eq!(count(MatchMode::Sides), 5);
        assert_eq!(count(MatchMode::CornersAndSides), 9);
        assert!(!MatchMode::Corners.uses(Peer::Top));
        assert!(!MatchMode::Sides.uses(Peer::TopLeft));
        assert!(MatchMode::Sides.uses_sides() && !MatchMode::Sides.uses_corners());
        assert!(MatchMode::Corners.uses_corners() && !MatchMode::Corners.uses_sides());
    }

    #[test]
    fn empty_neighbours_and_map_edge_read_as_zero() {
        let terrain = layer(&[(0, 0, 3)]);
        let req = required_peers(
            &TerrainSet::new(MatchMode::Sides),
            &terrain,
            CellCoord::new(0, 0),
        );
        assert_eq!(
            req,
            [
                None,
                Some(0),
                None,
                Some(0),
                Some(3),
                Some(0),
                None,
                Some(0),
                None
            ]
        );
        let far = CellCoord::new(i32::MAX, i32::MIN);
        let req = required_peers(&TerrainSet::new(MatchMode::CornersAndSides), &terrain, far);
        assert_eq!(req, [Some(0); 9]);
    }

    #[test]
    fn sides_read_orthogonal_neighbours() {
        let terrain = layer(&[(0, 0, 1), (0, 1, 2), (-1, 0, 3), (1, 1, 4)]);
        let req = required_peers(
            &TerrainSet::new(MatchMode::Sides),
            &terrain,
            CellCoord::new(0, 0),
        );
        assert_eq!(req[Peer::Top.index()], Some(2));
        assert_eq!(req[Peer::Left.index()], Some(3));
        assert_eq!(req[Peer::Right.index()], Some(0));
        assert_eq!(req[Peer::TopRight.index()], None);
    }

    #[test]
    fn corners_and_sides_wildcards_corner_unless_both_sides_match_centre() {
        let terrain = layer(&[
            (0, 0, 1),
            (0, 1, 1),
            (1, 0, 1),
            (1, 1, 2),
            (-1, 0, 2),
            (-1, 1, 1),
        ]);
        let req = required_peers(
            &TerrainSet::new(MatchMode::CornersAndSides),
            &terrain,
            CellCoord::new(0, 0),
        );
        assert_eq!(req[Peer::TopRight.index()], Some(2));
        assert_eq!(req[Peer::TopLeft.index()], None);
        assert_eq!(req[Peer::BottomRight.index()], None);
        assert_eq!(req[Peer::BottomLeft.index()], None);
        assert_eq!(req[Peer::Left.index()], Some(2));
    }

    #[test]
    fn corners_take_the_vertex_minimum() {
        let terrain = layer(&[
            (0, 0, 3),
            (0, 1, 3),
            (1, 1, 3),
            (1, 0, 3),
            (-1, 0, 4),
            (-1, 1, 5),
            (0, -1, 2),
            (-1, -1, 4),
        ]);
        let req = required_peers(
            &TerrainSet::new(MatchMode::Corners),
            &terrain,
            CellCoord::new(0, 0),
        );
        assert_eq!(req[Peer::TopRight.index()], Some(3));
        assert_eq!(req[Peer::TopLeft.index()], Some(3));
        assert_eq!(req[Peer::BottomLeft.index()], Some(2));
        assert_eq!(req[Peer::BottomRight.index()], Some(0));
        assert_eq!(req[Peer::Top.index()], None);
    }

    #[test]
    fn cells_sharing_a_vertex_agree_on_its_terrain() {
        let mut cells = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                cells.push((x, y, ((x * 7 + y * 13 + x * y) % 4) as TerrainId));
            }
        }
        let terrain = layer(&cells);
        let corner = |x: i32, y: i32, peer: Peer| {
            required_peers(
                &TerrainSet::new(MatchMode::Corners),
                &terrain,
                CellCoord::new(x, y),
            )[peer.index()]
        };
        for y in -1..8 {
            for x in -1..8 {
                let v = corner(x, y, Peer::TopRight);
                assert_eq!(corner(x + 1, y, Peer::TopLeft), v);
                assert_eq!(corner(x, y + 1, Peer::BottomRight), v);
                assert_eq!(corner(x + 1, y + 1, Peer::BottomLeft), v);
            }
        }
    }

    #[test]
    fn corner_enclosed_needs_both_sides_equal_to_centre() {
        let t = TileTerrains::filled(1).with(Peer::Top, 2);
        assert!(!t.corner_enclosed(Peer::TopLeft));
        assert!(!t.corner_enclosed(Peer::TopRight));
        assert!(t.corner_enclosed(Peer::BottomLeft));
        assert!(!t.corner_enclosed(Peer::Left));
    }

    fn picks_over_blocks(set: &TerrainSet) -> BTreeSet<u16> {
        let mut terrain = ChunkedLayer::new();
        for i in 0..200 {
            for peer in Peer::ALL {
                terrain.set(CellCoord(IVec2::new(i * 4, 0) + peer.offset()), 1);
            }
        }
        (0..200)
            .map(|i| {
                resolve_cell(set, &terrain, CellCoord::new(i * 4, 0))
                    .tile
                    .index()
            })
            .collect()
    }

    fn shape_set(shapes: &[(TileTerrains, u32)]) -> TerrainSet {
        let mut set = TerrainSet::new(MatchMode::CornersAndSides);
        for (i, &(t, w)) in shapes.iter().enumerate() {
            set = set.with_tile(TerrainTile::new(TileRef::new(i as u16), t, w));
        }
        set
    }

    #[test]
    fn shape_ties_prefer_side_matches_then_set_order() {
        let side_off = TileTerrains::filled(1).with(Peer::Top, 2);
        let tl_off = TileTerrains::filled(1).with(Peer::TopLeft, 2);
        let tr_off = TileTerrains::filled(1).with(Peer::TopRight, 2);
        let set = shape_set(&[(side_off, 1), (tl_off, 1)]);
        assert_eq!(picks_over_blocks(&set), BTreeSet::from([1]));
        let set = shape_set(&[(tl_off, 1), (tr_off, 1)]);
        assert_eq!(picks_over_blocks(&set), BTreeSet::from([0]));
        let set = shape_set(&[(tl_off, 0), (tr_off, 1)]);
        assert_eq!(picks_over_blocks(&set), BTreeSet::from([1]));
        let set = shape_set(&[(tl_off, 1), (tr_off, 1), (tl_off, 1)]);
        assert_eq!(picks_over_blocks(&set), BTreeSet::from([0, 2]));
    }

    #[test]
    fn overflowing_neighbours_read_empty_and_are_skipped() {
        let edge = CellCoord::new(i32::MAX, 0);
        let terrain = layer(&[(i32::MAX, 0, 1), (i32::MIN, 0, 1)]);
        let req = required_peers(&TerrainSet::new(MatchMode::Sides), &terrain, edge);
        assert_eq!(req[Peer::Right.index()], Some(0));
        let out = resolve_region(&variant_set(&[1]), &terrain, &BTreeMap::new(), [edge]);
        assert_eq!(out.len(), 6);
        assert!(out.iter().all(|(c, _)| c.0.x >= i32::MAX - 1));
    }

    #[test]
    fn empty_centre_and_unknown_terrain_resolve_empty() {
        let set = variant_set(&[1]);
        let terrain = layer(&[(0, 0, 7)]);
        let empty = resolve_cell(&set, &terrain, CellCoord::new(5, 5));
        assert_eq!(empty.tile, TileRef::EMPTY);
        assert!(empty.is_exact());
        let unknown = resolve_cell(&set, &terrain, CellCoord::new(0, 0));
        assert_eq!(unknown.tile, TileRef::EMPTY);
        assert_eq!(unknown.score, 0);
        assert!(!unknown.is_exact());
    }

    #[test]
    fn best_score_wins_over_weight() {
        let set = TerrainSet::new(MatchMode::Sides)
            .with_tile(TerrainTile::new(
                TileRef::new(0),
                TileTerrains::filled(1),
                100,
            ))
            .with_tile(TerrainTile::new(
                TileRef::new(1),
                TileTerrains::filled(1).with(Peer::Top, 0),
                1,
            ));
        let terrain = layer(&[(0, 0, 1), (-1, 0, 1), (1, 0, 1), (0, -1, 1)]);
        let r = resolve_cell(&set, &terrain, CellCoord::new(0, 0));
        assert_eq!(r.tile, TileRef::new(1));
        assert_eq!((r.score, r.max_score), (5, 5));
    }

    #[test]
    fn variants_spread_evenly_with_equal_weights() {
        let counts = pick_counts(&variant_set(&[1, 1]), 1000);
        assert!(
            counts.iter().all(|&c| (400..=600).contains(&c)),
            "{counts:?}"
        );
    }

    #[test]
    fn variants_follow_weights() {
        let counts = pick_counts(&variant_set(&[3, 1]), 1000);
        assert!((680..=820).contains(&counts[0]), "{counts:?}");
    }

    #[test]
    fn zero_weight_never_picked_while_positive_ties() {
        let counts = pick_counts(&variant_set(&[0, 1, 0]), 500);
        assert_eq!(counts, vec![0, 500, 0]);
        let counts = pick_counts(&variant_set(&[0, 0]), 50);
        assert_eq!(counts, vec![50, 0]);
    }

    #[test]
    fn same_input_gives_same_output() {
        let set = variant_set(&[1, 2, 3, 4]);
        let cells: Vec<_> = (0..200).map(|i| (i % 20, i / 20, 1)).collect();
        let terrain = layer(&cells);
        let dirty: Vec<_> = cells
            .iter()
            .map(|&(x, y, _)| CellCoord::new(x, y))
            .collect();
        let a = resolve_region(&set, &terrain, &BTreeMap::new(), dirty.clone());
        let b = resolve_region(&set, &terrain, &BTreeMap::new(), dirty);
        assert_eq!(a, b);
        assert!(!a.is_empty());
    }

    #[test]
    fn region_covers_neighbours_deduplicated_and_sorted() {
        let set = variant_set(&[1]);
        let terrain = layer(&[(0, 0, 1), (1, 0, 1)]);
        let dirty = [
            CellCoord::new(0, 0),
            CellCoord::new(1, 0),
            CellCoord::new(0, 0),
        ];
        let out = resolve_region(&set, &terrain, &BTreeMap::new(), dirty);
        let cells: Vec<_> = out.iter().map(|(c, _)| *c).collect();
        let mut expected = Vec::new();
        for y in -1..=1 {
            for x in -1..=2 {
                expected.push(CellCoord::new(x, y));
            }
        }
        assert_eq!(cells, expected);
        for (c, t) in &out {
            assert_eq!(*t, resolve_cell(&set, &terrain, *c).tile);
        }
        assert!(
            !out.iter()
                .find(|(c, _)| *c == CellCoord::new(0, 0))
                .unwrap()
                .1
                .is_empty()
        );
    }

    #[test]
    fn overrides_are_returned_unchanged() {
        let set = variant_set(&[1]);
        let terrain = layer(&[(0, 0, 1), (1, 0, 1)]);
        let hand = TileRef::new(42).with_flip_x(true);
        let overrides = BTreeMap::from([
            (CellCoord::new(0, 0), hand),
            (CellCoord::new(1, 1), TileRef::EMPTY),
        ]);
        let out = resolve_region(&set, &terrain, &overrides, [CellCoord::new(0, 0)]);
        let get = |c: CellCoord| out.iter().find(|(o, _)| *o == c).map(|(_, t)| *t);
        assert_eq!(get(CellCoord::new(0, 0)), Some(hand));
        assert_eq!(get(CellCoord::new(1, 1)), Some(TileRef::EMPTY));
        assert_eq!(get(CellCoord::new(1, 0)), Some(TileRef::new(0)));
    }

    const DIRT: TerrainId = 1;
    const SAND: TerrainId = 2;
    const WATER: TerrainId = 3;

    /// Template sets for the given (inner, outer) pairs in one set, tiles renumbered.
    fn combined(template: LayoutTemplate, pairs: &[(TerrainId, TerrainId)]) -> TerrainSet {
        let mut set = TerrainSet::new(template.mode());
        for &(inner, outer) in pairs {
            let offset = set.tiles.len() as u16;
            for t in template.terrain_set(inner, outer).tiles {
                let tile = TileRef::new(offset + t.tile.index());
                set = set.with_tile(TerrainTile::new(tile, t.terrains, t.weight));
            }
        }
        set
    }

    /// Dirt-sand and sand-water transitions both ways, no dirt-water.
    fn banded(template: LayoutTemplate) -> TerrainSet {
        combined(
            template,
            &[(DIRT, SAND), (SAND, DIRT), (SAND, WATER), (WATER, SAND)],
        )
        .with_order([DIRT, SAND, WATER])
    }

    fn tile_of(set: &TerrainSet, shape: TileTerrains) -> TileRef {
        set.tiles
            .iter()
            .find(|t| t.terrains == shape)
            .map(|t| t.tile)
            .unwrap()
    }

    /// Dirt for x <= 0, water for x >= 1, over a 4x3 block.
    fn seam() -> ChunkedLayer<TerrainId> {
        let mut cells = Vec::new();
        for y in -1..=1 {
            for x in -1..=2 {
                cells.push((x, y, if x <= 0 { DIRT } else { WATER }));
            }
        }
        layer(&cells)
    }

    fn assert_band(set: &TerrainSet, dirt: TileTerrains, water: TileTerrains) {
        let terrain = seam();
        let d = resolve_cell(set, &terrain, CellCoord::new(0, 0));
        assert!(d.is_exact(), "{d:?}");
        assert_eq!(d.tile, tile_of(set, dirt));
        let w = resolve_cell(set, &terrain, CellCoord::new(1, 0));
        assert!(w.is_exact(), "{w:?}");
        assert_eq!(w.tile, tile_of(set, water));
    }

    #[test]
    fn sides_seam_draws_sand_on_both_sides() {
        let set = banded(LayoutTemplate::Side16);
        let dirt = LayoutTemplate::Side16.tiles(DIRT, SAND)[4];
        let water = LayoutTemplate::Side16.tiles(WATER, SAND)[2];
        assert_band(&set, dirt, water);
    }

    #[test]
    fn corners_and_sides_seam_draws_sand_on_both_sides() {
        let set = banded(LayoutTemplate::Blob47);
        let dirt = TileTerrains::filled(DIRT)
            .with(Peer::Right, SAND)
            .with(Peer::TopRight, SAND)
            .with(Peer::BottomRight, SAND);
        let water = TileTerrains::filled(WATER)
            .with(Peer::Left, SAND)
            .with(Peer::TopLeft, SAND)
            .with(Peer::BottomLeft, SAND);
        assert_band(&set, dirt, water);
    }

    #[test]
    fn corners_seam_draws_sand_in_the_upper_cell() {
        let set = banded(LayoutTemplate::Corner16);
        let dirt = LayoutTemplate::Corner16.tiles(DIRT, SAND)[0];
        let water = LayoutTemplate::Corner16.tiles(WATER, SAND)[0b0101];
        assert_band(&set, dirt, water);
        let req = required_peers(&set, &seam(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::TopRight.index()], Some(DIRT));
        let req = required_peers(&set, &seam(), CellCoord::new(1, 0));
        assert_eq!(req[Peer::TopLeft.index()], Some(SAND));
        assert_eq!(req[Peer::TopRight.index()], Some(WATER));
    }

    #[test]
    fn resolve_leaves_terrain_unchanged() {
        let set = banded(LayoutTemplate::Blob47);
        let terrain = seam();
        let before: Vec<_> = terrain.cells().collect();
        let dirty: Vec<_> = before.iter().map(|(c, _)| *c).collect();
        let out = resolve_region(&set, &terrain, &BTreeMap::new(), dirty);
        assert!(!out.is_empty());
        assert_eq!(terrain.cells().collect::<Vec<_>>(), before);
        assert!(terrain.cells().all(|(_, t)| t == DIRT || t == WATER));
    }

    #[test]
    fn direct_transitions_are_not_substituted() {
        let set = combined(
            LayoutTemplate::Side16,
            &[(DIRT, SAND), (DIRT, WATER), (WATER, SAND), (WATER, DIRT)],
        )
        .with_order([DIRT, SAND, WATER]);
        let dirt = LayoutTemplate::Side16.tiles(DIRT, WATER)[4];
        let water = LayoutTemplate::Side16.tiles(WATER, DIRT)[2];
        assert_band(&set, dirt, water);
    }

    #[test]
    fn substitution_steps_once_to_a_usable_terrain() {
        let set = banded(LayoutTemplate::Side16).with_order([DIRT, SAND, 4, WATER]);
        let req = required_peers(&set, &seam(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::Right.index()], Some(SAND));
        let req = required_peers(&set, &seam(), CellCoord::new(1, 0));
        assert_eq!(req[Peer::Left.index()], Some(DIRT));
        let set = banded(LayoutTemplate::Side16).with_order([DIRT, 4, WATER]);
        let req = required_peers(&set, &seam(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::Right.index()], Some(WATER));
    }

    #[test]
    fn one_way_transitions_block_substitution_on_both_sides() {
        let set = combined(
            LayoutTemplate::Side16,
            &[(DIRT, SAND), (DIRT, WATER), (WATER, SAND)],
        )
        .with_order([DIRT, SAND, WATER]);
        let d = resolve_cell(&set, &seam(), CellCoord::new(0, 0));
        assert!(d.is_exact());
        assert_eq!(
            d.tile,
            tile_of(&set, LayoutTemplate::Side16.tiles(DIRT, WATER)[4])
        );
        let req = required_peers(&set, &seam(), CellCoord::new(1, 0));
        assert_eq!(req[Peer::Left.index()], Some(DIRT));
    }

    #[test]
    fn corner_only_transitions_do_not_count_for_sides() {
        let mut set = banded(LayoutTemplate::Blob47);
        for pair in [(DIRT, WATER), (WATER, DIRT)] {
            let offset = set.tiles.len() as u16;
            for t in LayoutTemplate::Corner2x2.terrain_set(pair.0, pair.1).tiles {
                let tile = TileRef::new(offset + t.tile.index());
                set = set.with_tile(TerrainTile::new(tile, t.terrains, t.weight));
            }
        }
        let dirt = TileTerrains::filled(DIRT)
            .with(Peer::Right, SAND)
            .with(Peer::TopRight, SAND)
            .with(Peer::BottomRight, SAND);
        let water = TileTerrains::filled(WATER)
            .with(Peer::Left, SAND)
            .with(Peer::TopLeft, SAND)
            .with(Peer::BottomLeft, SAND);
        assert_band(&set, dirt, water);
        let mut diagonal = layer(&[(0, 0, DIRT), (0, 1, DIRT), (1, 0, DIRT), (1, 1, WATER)]);
        diagonal.set(CellCoord::new(-1, 0), DIRT);
        let req = required_peers(&set, &diagonal, CellCoord::new(0, 0));
        assert_eq!(req[Peer::TopRight.index()], Some(WATER));
    }

    #[test]
    fn with_order_drops_empty_and_repeats() {
        let set = TerrainSet::new(MatchMode::Sides).with_order([0, 2, 1, 2, 0, 3, 1]);
        assert_eq!(set.order, vec![2, 1, 3]);
    }

    #[test]
    fn no_order_matches_unsubstituted_requirements() {
        let mut set = banded(LayoutTemplate::Side16);
        set.order.clear();
        let req = required_peers(&set, &seam(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::Right.index()], Some(WATER));
        assert!(!resolve_cell(&set, &seam(), CellCoord::new(0, 0)).is_exact());
    }

    #[test]
    fn unordered_and_empty_neighbours_are_left_as_is() {
        let set = banded(LayoutTemplate::Side16).with_order([DIRT, SAND]);
        let req = required_peers(&set, &seam(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::Right.index()], Some(WATER));
        let terrain = layer(&[(0, 0, DIRT)]);
        let set = banded(LayoutTemplate::Side16);
        let req = required_peers(&set, &terrain, CellCoord::new(0, 0));
        assert_eq!(req[Peer::Right.index()], Some(0));
    }

    #[test]
    fn terrain_rank_follows_order_then_id() {
        let set = TerrainSet::new(MatchMode::Corners).with_order([3, 1]);
        let ranks: Vec<_> = [0, 3, 1, 2, 4].iter().map(|t| set.rank(*t)).collect();
        assert!(ranks.windows(2).all(|w| w[0] < w[1]), "{ranks:?}");
        let plain = TerrainSet::new(MatchMode::Corners);
        assert!(plain.rank(0) < plain.rank(1) && plain.rank(1) < plain.rank(2));
    }

    #[test]
    fn cells_sharing_a_vertex_agree_under_a_custom_order() {
        let mut set = TerrainSet::new(MatchMode::Corners).with_order([3, 1, 2]);
        for a in 1..=3 {
            for b in 0..=3 {
                let shape = TileTerrains::filled(b).with(Peer::Centre, a);
                set = set.with_tile(TerrainTile::new(TileRef::new(0), shape, 1));
            }
        }
        let mut cells = Vec::new();
        for y in 0..8 {
            for x in 0..8 {
                cells.push((x, y, ((x * 7 + y * 13 + x * y) % 4) as TerrainId));
            }
        }
        let terrain = layer(&cells);
        let corner = |x: i32, y: i32, peer: Peer| {
            required_peers(&set, &terrain, CellCoord::new(x, y))[peer.index()]
        };
        for y in -1..8 {
            for x in -1..8 {
                let v = corner(x, y, Peer::TopRight);
                assert_eq!(corner(x + 1, y, Peer::TopLeft), v);
                assert_eq!(corner(x, y + 1, Peer::BottomRight), v);
                assert_eq!(corner(x + 1, y + 1, Peer::BottomLeft), v);
            }
        }
        let terrain = layer(&[(0, 0, 1), (1, 0, 3), (0, 1, 1), (1, 1, 1)]);
        let req = required_peers(&set, &terrain, CellCoord::new(0, 0));
        assert_eq!(req[Peer::TopRight.index()], Some(3));
    }

    #[test]
    fn banded_resolve_is_deterministic() {
        let set = banded(LayoutTemplate::Blob47);
        let cells: Vec<_> = (0..400)
            .map(|i: i32| (i % 20, i / 20, [DIRT, SAND, WATER][(i * 7 % 3) as usize]))
            .collect();
        let terrain = layer(&cells);
        let dirty: Vec<_> = cells
            .iter()
            .map(|&(x, y, _)| CellCoord::new(x, y))
            .collect();
        let a = resolve_region(&set, &terrain, &BTreeMap::new(), dirty.clone());
        let b = resolve_region(&set, &terrain, &BTreeMap::new(), dirty);
        assert_eq!(a, b);
        assert!(a.iter().any(|(_, t)| !t.is_empty()));
    }
}
