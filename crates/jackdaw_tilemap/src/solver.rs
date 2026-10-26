//! Terrain solver: turns the terrain ids around a cell into a tile.
//!
//! Tiles carry nine peers, row-major with the top (+y) row first. Required
//! peers come from the cell's 3x3 block; cells off the map or unpainted read 0.
//! - Centre: the cell's terrain. Side: the neighbour across that side, or "not
//!   the centre" when the neighbour has the centre's terrain but the two cells
//!   are not joined by their [`Links`]: once either is a Path cell, both must
//!   mark the shared side.
//! - Corner, `CornersAndSides`: a wildcard unless both adjacent sides are
//!   joined. Then the centre when the diagonal neighbour joins both side
//!   neighbours, "not the centre" when it has the centre's terrain but does not,
//!   otherwise the diagonal neighbour.
//! - Corner, `Corners`: the vertex terrain, the lowest of the four cells
//!   sharing that vertex by [`TerrainSet::rank`]. Links do not apply in this mode.
//!
//! In-between terrains are visual only. When A and B have no transition tiles
//! either way on that peer class (side or corner), a cell of A matches peer B
//! as the terrain next to A in the order towards B, if A has tiles for it.
//! Cells sharing a vertex agree on its terrain before this per-cell step, so
//! in `Corners` mode only the higher cell draws the band.
//!
//! Candidates share the cell's centre and score one point per matched required
//! peer. Ties go to more matched sides, then a positive weight, then no joined
//! side (side equal to centre) the cell does not join, then a straight tile
//! along a joined run, then set order; tiles with that exact shape are then
//! picked by weight from a cell hash.
//! Hand overrides are returned unchanged by [`resolve_region`].

use std::collections::{BTreeMap, BTreeSet};

use glam::IVec2;

use crate::path::links_join;
use crate::{CellCoord, ChunkedLayer, Links, TerrainId, TileRef};

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

    /// The peer on the opposite side of the centre.
    pub const fn opposite(self) -> Peer {
        match self {
            Peer::TopLeft => Peer::BottomRight,
            Peer::Top => Peer::Bottom,
            Peer::TopRight => Peer::BottomLeft,
            Peer::Left => Peer::Right,
            Peer::Centre => Peer::Centre,
            Peer::Right => Peer::Left,
            Peer::BottomLeft => Peer::TopRight,
            Peer::Bottom => Peer::Top,
            Peer::BottomRight => Peer::TopLeft,
        }
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
    /// Per tile, indexed like `set.tiles`; all zero when sides are not matched.
    links: Vec<SideLinks>,
}

/// Side masks over [`Peer::SIDES`], one bit per side.
#[derive(Clone, Copy)]
struct SideLinks {
    /// Sides whose terrain equals the centre.
    joined: u8,
    /// `joined` when it is exactly Top and Bottom or exactly Left and Right, else 0.
    straight: u8,
}

const TOP_BOTTOM: u8 = Links::TOP | Links::BOTTOM;
const LEFT_RIGHT: u8 = Links::LEFT | Links::RIGHT;

/// Side mask in the [`Links`] side bit layout.
fn side_mask(joined: impl Fn(Peer) -> bool) -> u8 {
    let mut mask = 0;
    for side in Peer::SIDES {
        if joined(side) {
            mask |= Links::side_bit(side);
        }
    }
    mask
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
        let links = set
            .tiles
            .iter()
            .map(|tile| {
                let t = tile.terrains;
                let joined = if set.mode.uses_sides() {
                    side_mask(|side| t.get(side) == t.centre())
                } else {
                    0
                };
                let straight = if joined == TOP_BOTTOM || joined == LEFT_RIGHT {
                    joined
                } else {
                    0
                };
                SideLinks { joined, straight }
            })
            .collect();
        Self {
            set,
            positions,
            transitions,
            links,
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

/// What a tile peer must hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Requirement {
    /// A wildcard; not counted in the score.
    Any,
    Is(TerrainId),
    /// Any terrain but this one.
    Not(TerrainId),
}

impl Requirement {
    pub const fn is_any(self) -> bool {
        matches!(self, Requirement::Any)
    }

    /// True when `terrain` satisfies a non-wildcard requirement, so it scores a point.
    pub const fn scores(self, terrain: TerrainId) -> bool {
        match self {
            Requirement::Any => false,
            Requirement::Is(t) => t == terrain,
            Requirement::Not(t) => t != terrain,
        }
    }
}

/// Required terrain per peer, indexed by [`Peer::index`].
pub type RequiredPeers = [Requirement; 9];

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
    links: &ChunkedLayer<Links>,
    cell: CellCoord,
) -> RequiredPeers {
    let block = read_block(terrain, cell);
    required_from_block(&Lookup::new(set), &block, &Joins::read(&block, links, cell))
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

/// Which neighbours of a cell join it, by terrain and [`Links`].
struct Joins {
    /// Side mask of the sides the cell joins.
    sides: u8,
    /// Per [`Peer::CORNERS`]: the diagonal has the centre's terrain and joins
    /// both side neighbours next to it.
    diagonals: [bool; 4],
}

impl Joins {
    fn read(block: &TileTerrains, links: &ChunkedLayer<Links>, cell: CellCoord) -> Self {
        let at = |peer: Peer| neighbour(cell, peer.offset()).map_or(Links::EMPTY, |n| links.get(n));
        let centre = block.centre();
        let sides = side_mask(|side| {
            block.get(side) == centre && links_join(at(Peer::Centre), at(side), side)
        });
        let diagonals = Peer::CORNERS.map(|corner| {
            corner.corner_sides().is_some_and(|[a, b]| {
                block.get(corner) == centre
                    && links_join(at(a), at(corner), b)
                    && links_join(at(b), at(corner), a)
            })
        });
        Self { sides, diagonals }
    }
}

const fn side_joined(joined: u8, side: Peer) -> bool {
    joined & Links::side_bit(side) != 0
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

fn required_from_block(lookup: &Lookup, block: &TileTerrains, joins: &Joins) -> RequiredPeers {
    let mode = lookup.set.mode;
    let centre = block.centre();
    let mut required = [Requirement::Any; 9];
    required[Peer::Centre.index()] = Requirement::Is(centre);
    if mode.uses_sides() {
        for side in Peer::SIDES {
            let t = block.get(side);
            required[side.index()] = if t == centre && !side_joined(joins.sides, side) {
                Requirement::Not(centre)
            } else {
                Requirement::Is(lookup.substitute(centre, t, PeerClass::Side))
            };
        }
    }
    for (corner, diagonal_joined) in Peer::CORNERS.into_iter().zip(joins.diagonals) {
        let sides_joined = corner
            .corner_sides()
            .is_some_and(|[a, b]| side_joined(joins.sides, a) && side_joined(joins.sides, b));
        let diagonal = block.get(corner);
        let substituted = |t| Requirement::Is(lookup.substitute(centre, t, PeerClass::Corner));
        required[corner.index()] = match mode {
            MatchMode::Sides => Requirement::Any,
            MatchMode::Corners => substituted(vertex_terrain(lookup, block, corner)),
            MatchMode::CornersAndSides => {
                if !sides_joined {
                    Requirement::Any
                } else if diagonal_joined {
                    Requirement::Is(centre)
                } else if diagonal == centre {
                    Requirement::Not(centre)
                } else {
                    substituted(diagonal)
                }
            }
        };
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
        .filter(|p| required[p.index()].scores(tile.get(**p)))
        .count();
    hits as u8
}

/// Resolves one cell from its neighbourhood. Builds the set's lookups per
/// call; [`resolve_region`] builds them once.
pub fn resolve_cell(
    set: &TerrainSet,
    terrain: &ChunkedLayer<TerrainId>,
    links: &ChunkedLayer<Links>,
    cell: CellCoord,
) -> Resolved {
    resolve_with(&Lookup::new(set), terrain, links, cell)
}

fn resolve_with(
    lookup: &Lookup,
    terrain: &ChunkedLayer<TerrainId>,
    links: &ChunkedLayer<Links>,
    cell: CellCoord,
) -> Resolved {
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
    let required = required_from_block(lookup, &block, &Joins::read(&block, links, cell));
    let max_score = required.iter().filter(|r| !r.is_any()).count() as u8;
    let needed = side_mask(|side| required[side.index()] == Requirement::Is(centre));
    let mut best: Option<(&TerrainTile, (u8, u8, bool, bool, bool))> = None;
    for (tile, shape_links) in set.tiles.iter().zip(&lookup.links) {
        if tile.terrains.centre() != centre {
            continue;
        }
        let key = (
            match_score(&required, &tile.terrains),
            count_matches(&required, &tile.terrains, &Peer::SIDES),
            tile.weight > 0,
            shape_links.joined & !needed == 0,
            shape_links.straight != 0 && shape_links.straight & !needed == 0,
        );
        if best.is_none_or(|(_, k)| key > k) {
            best = Some((tile, key));
        }
    }
    let Some((shape, (score, ..))) = best else {
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
    links: &ChunkedLayer<Links>,
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
                .unwrap_or_else(|| resolve_with(&lookup, terrain, links, cell).tile);
            (cell, tile)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::Requirement::{Any, Is, Not};
    use super::*;
    use crate::{
        LayoutTemplate, Legend, Region, apply_connect_stroke, apply_path_stroke, parse_rows,
        write_region,
    };

    fn no_links() -> ChunkedLayer<Links> {
        ChunkedLayer::new()
    }

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
            let r = resolve_cell(set, &terrain, &no_links(), CellCoord::new(i as i32 * 2, 0));
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
            &no_links(),
            CellCoord::new(0, 0),
        );
        assert_eq!(req, [Any, Is(0), Any, Is(0), Is(3), Is(0), Any, Is(0), Any]);
        let far = CellCoord::new(i32::MAX, i32::MIN);
        let req = required_peers(
            &TerrainSet::new(MatchMode::CornersAndSides),
            &terrain,
            &no_links(),
            far,
        );
        assert_eq!(req, [Is(0); 9]);
    }

    #[test]
    fn sides_read_orthogonal_neighbours() {
        let terrain = layer(&[(0, 0, 1), (0, 1, 2), (-1, 0, 3), (1, 1, 4)]);
        let req = required_peers(
            &TerrainSet::new(MatchMode::Sides),
            &terrain,
            &no_links(),
            CellCoord::new(0, 0),
        );
        assert_eq!(req[Peer::Top.index()], Is(2));
        assert_eq!(req[Peer::Left.index()], Is(3));
        assert_eq!(req[Peer::Right.index()], Is(0));
        assert_eq!(req[Peer::TopRight.index()], Any);
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
            &no_links(),
            CellCoord::new(0, 0),
        );
        assert_eq!(req[Peer::TopRight.index()], Is(2));
        assert_eq!(req[Peer::TopLeft.index()], Any);
        assert_eq!(req[Peer::BottomRight.index()], Any);
        assert_eq!(req[Peer::BottomLeft.index()], Any);
        assert_eq!(req[Peer::Left.index()], Is(2));
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
            &no_links(),
            CellCoord::new(0, 0),
        );
        assert_eq!(req[Peer::TopRight.index()], Is(3));
        assert_eq!(req[Peer::TopLeft.index()], Is(3));
        assert_eq!(req[Peer::BottomLeft.index()], Is(2));
        assert_eq!(req[Peer::BottomRight.index()], Is(0));
        assert_eq!(req[Peer::Top.index()], Any);
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
                &no_links(),
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
                resolve_cell(set, &terrain, &no_links(), CellCoord::new(i * 4, 0))
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
        let req = required_peers(
            &TerrainSet::new(MatchMode::Sides),
            &terrain,
            &no_links(),
            edge,
        );
        assert_eq!(req[Peer::Right.index()], Is(0));
        let out = resolve_region(
            &variant_set(&[1]),
            &terrain,
            &no_links(),
            &BTreeMap::new(),
            [edge],
        );
        assert_eq!(out.len(), 6);
        assert!(out.iter().all(|(c, _)| c.0.x >= i32::MAX - 1));
    }

    #[test]
    fn empty_centre_and_unknown_terrain_resolve_empty() {
        let set = variant_set(&[1]);
        let terrain = layer(&[(0, 0, 7)]);
        let empty = resolve_cell(&set, &terrain, &no_links(), CellCoord::new(5, 5));
        assert_eq!(empty.tile, TileRef::EMPTY);
        assert!(empty.is_exact());
        let unknown = resolve_cell(&set, &terrain, &no_links(), CellCoord::new(0, 0));
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
        let r = resolve_cell(&set, &terrain, &no_links(), CellCoord::new(0, 0));
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
        let a = resolve_region(&set, &terrain, &no_links(), &BTreeMap::new(), dirty.clone());
        let b = resolve_region(&set, &terrain, &no_links(), &BTreeMap::new(), dirty);
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
        let out = resolve_region(&set, &terrain, &no_links(), &BTreeMap::new(), dirty);
        let cells: Vec<_> = out.iter().map(|(c, _)| *c).collect();
        let mut expected = Vec::new();
        for y in -1..=1 {
            for x in -1..=2 {
                expected.push(CellCoord::new(x, y));
            }
        }
        assert_eq!(cells, expected);
        for (c, t) in &out {
            assert_eq!(*t, resolve_cell(&set, &terrain, &no_links(), *c).tile);
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
        let out = resolve_region(
            &set,
            &terrain,
            &no_links(),
            &overrides,
            [CellCoord::new(0, 0)],
        );
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
        let d = resolve_cell(set, &terrain, &no_links(), CellCoord::new(0, 0));
        assert!(d.is_exact(), "{d:?}");
        assert_eq!(d.tile, tile_of(set, dirt));
        let w = resolve_cell(set, &terrain, &no_links(), CellCoord::new(1, 0));
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
        let req = required_peers(&set, &seam(), &no_links(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::TopRight.index()], Is(DIRT));
        let req = required_peers(&set, &seam(), &no_links(), CellCoord::new(1, 0));
        assert_eq!(req[Peer::TopLeft.index()], Is(SAND));
        assert_eq!(req[Peer::TopRight.index()], Is(WATER));
    }

    #[test]
    fn resolve_leaves_terrain_unchanged() {
        let set = banded(LayoutTemplate::Blob47);
        let terrain = seam();
        let before: Vec<_> = terrain.cells().collect();
        let dirty: Vec<_> = before.iter().map(|(c, _)| *c).collect();
        let out = resolve_region(&set, &terrain, &no_links(), &BTreeMap::new(), dirty);
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
        let req = required_peers(&set, &seam(), &no_links(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::Right.index()], Is(SAND));
        let req = required_peers(&set, &seam(), &no_links(), CellCoord::new(1, 0));
        assert_eq!(req[Peer::Left.index()], Is(DIRT));
        let set = banded(LayoutTemplate::Side16).with_order([DIRT, 4, WATER]);
        let req = required_peers(&set, &seam(), &no_links(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::Right.index()], Is(WATER));
    }

    #[test]
    fn one_way_transitions_block_substitution_on_both_sides() {
        let set = combined(
            LayoutTemplate::Side16,
            &[(DIRT, SAND), (DIRT, WATER), (WATER, SAND)],
        )
        .with_order([DIRT, SAND, WATER]);
        let d = resolve_cell(&set, &seam(), &no_links(), CellCoord::new(0, 0));
        assert!(d.is_exact());
        assert_eq!(
            d.tile,
            tile_of(&set, LayoutTemplate::Side16.tiles(DIRT, WATER)[4])
        );
        let req = required_peers(&set, &seam(), &no_links(), CellCoord::new(1, 0));
        assert_eq!(req[Peer::Left.index()], Is(DIRT));
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
        let req = required_peers(&set, &diagonal, &no_links(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::TopRight.index()], Is(WATER));
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
        let req = required_peers(&set, &seam(), &no_links(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::Right.index()], Is(WATER));
        assert!(!resolve_cell(&set, &seam(), &no_links(), CellCoord::new(0, 0)).is_exact());
    }

    #[test]
    fn unordered_and_empty_neighbours_are_left_as_is() {
        let set = banded(LayoutTemplate::Side16).with_order([DIRT, SAND]);
        let req = required_peers(&set, &seam(), &no_links(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::Right.index()], Is(WATER));
        let terrain = layer(&[(0, 0, DIRT)]);
        let set = banded(LayoutTemplate::Side16);
        let req = required_peers(&set, &terrain, &no_links(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::Right.index()], Is(0));
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
            required_peers(&set, &terrain, &no_links(), CellCoord::new(x, y))[peer.index()]
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
        let req = required_peers(&set, &terrain, &no_links(), CellCoord::new(0, 0));
        assert_eq!(req[Peer::TopRight.index()], Is(3));
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
        let a = resolve_region(&set, &terrain, &no_links(), &BTreeMap::new(), dirty.clone());
        let b = resolve_region(&set, &terrain, &no_links(), &BTreeMap::new(), dirty);
        assert_eq!(a, b);
        assert!(a.iter().any(|(_, t)| !t.is_empty()));
    }

    const GRASS: TerrainId = 1;
    const DRY: TerrainId = 2;
    const TILLED: TerrainId = 3;

    struct PathTiles {
        terrain: TerrainId,
        isolated: [u16; 2],
        vertical: [u16; 3],
        horizontal: [u16; 3],
    }

    // Tile indices are from a CC0 12-column farm tilesheet.
    const DRY_PATH: PathTiles = PathTiles {
        terrain: DRY,
        isolated: [0, 63],
        vertical: [12, 24, 36],
        horizontal: [60, 61, 62],
    };
    const TILLED_PATH: PathTiles = PathTiles {
        terrain: TILLED,
        isolated: [1, 51],
        vertical: [13, 25, 37],
        horizontal: [48, 49, 50],
    };

    fn path_shape(terrain: TerrainId, joins: &[Peer]) -> TileTerrains {
        let mut shape = TileTerrains::filled(GRASS).with(Peer::Centre, terrain);
        for side in joins {
            shape = shape.with(*side, terrain);
        }
        shape
    }

    fn path_tile(index: u16, terrain: TerrainId, joins: &[Peer], weight: u32) -> TerrainTile {
        TerrainTile::new(TileRef::new(index), path_shape(terrain, joins), weight)
    }

    fn add_path(mut set: TerrainSet, path: &PathTiles, horizontal_first: bool) -> TerrainSet {
        let t = path.terrain;
        let isolated = [
            path_tile(path.isolated[0], t, &[], 1),
            path_tile(path.isolated[1], t, &[], 1),
        ];
        let vertical = [
            path_tile(path.vertical[0], t, &[Peer::Bottom], 1),
            path_tile(path.vertical[1], t, &[Peer::Top, Peer::Bottom], 1),
            path_tile(path.vertical[2], t, &[Peer::Top], 1),
        ];
        let horizontal = [
            path_tile(path.horizontal[0], t, &[Peer::Right], 1),
            path_tile(path.horizontal[1], t, &[Peer::Left, Peer::Right], 1),
            path_tile(path.horizontal[2], t, &[Peer::Left], 1),
        ];
        let (first, second) = if horizontal_first {
            (horizontal, vertical)
        } else {
            (vertical, horizontal)
        };
        for tile in isolated.into_iter().chain(first).chain(second) {
            set = set.with_tile(tile);
        }
        set
    }

    fn farm_set() -> TerrainSet {
        let set = add_path(TerrainSet::new(MatchMode::Sides), &DRY_PATH, false);
        add_path(set, &TILLED_PATH, false)
    }

    fn farm_set_horizontal_first() -> TerrainSet {
        let set = add_path(TerrainSet::new(MatchMode::Sides), &DRY_PATH, true);
        add_path(set, &TILLED_PATH, true)
    }

    struct Case {
        set: TerrainSet,
        region: Region,
        terrain: ChunkedLayer<TerrainId>,
        links: ChunkedLayer<Links>,
        tiles: BTreeMap<CellCoord, TileRef>,
    }

    impl Case {
        fn new(set: TerrainSet, rows: &str) -> Self {
            let legend = Legend::parse_entries([(".", "grass"), ("=", "dry"), ("#", "tilled")])
                .expect("valid legend");
            let region = parse_rows(rows, CellCoord::new(0, 0), &legend).expect("valid rows");
            let mut terrain = ChunkedLayer::new();
            write_region(&mut terrain, &region, |symbol| match symbol {
                '=' => DRY,
                '#' => TILLED,
                _ => GRASS,
            });
            let dirty: Vec<_> = terrain.cells().map(|(c, _)| c).collect();
            let tiles = resolve_region(&set, &terrain, &no_links(), &BTreeMap::new(), dirty)
                .into_iter()
                .collect();
            Self {
                set,
                region,
                terrain,
                links: no_links(),
                tiles,
            }
        }

        fn cells(&self, cells: &[(u32, u32)]) -> Vec<CellCoord> {
            cells
                .iter()
                .map(|&(c, r)| self.region.world(c, r))
                .collect()
        }

        fn update(&mut self, dirty: Vec<CellCoord>) {
            let out = resolve_region(
                &self.set,
                &self.terrain,
                &self.links,
                &BTreeMap::new(),
                dirty,
            );
            self.tiles.extend(out);
        }

        fn path(&mut self, cells: &[(u32, u32)], id: TerrainId) {
            let cells = self.cells(cells);
            let dirty = apply_path_stroke(&mut self.terrain, &mut self.links, &cells, id);
            self.update(dirty.expect("contiguous stroke"));
        }

        fn connect(&mut self, cells: &[(u32, u32)], id: TerrainId) {
            let cells = self.cells(cells);
            let dirty = apply_connect_stroke(&mut self.terrain, &mut self.links, &cells, id);
            self.update(dirty);
        }

        /// The tiles a fresh resolve of every painted cell gives.
        fn resolve_all(&self) -> Vec<Vec<Option<u16>>> {
            let mut fresh = Case {
                set: self.set.clone(),
                region: self.region.clone(),
                terrain: self.terrain.clone(),
                links: self.links.clone(),
                tiles: BTreeMap::new(),
            };
            fresh.update(self.terrain.cells().map(|(c, _)| c).collect());
            fresh.grid()
        }

        fn required(&self, col: u32, row: u32) -> RequiredPeers {
            let cell = self.region.world(col, row);
            required_peers(&self.set, &self.terrain, &self.links, cell)
        }

        fn tile(&self, col: u32, row: u32) -> Option<u16> {
            let tile = self.tiles[&self.region.world(col, row)];
            (!tile.is_empty()).then_some(tile.index())
        }

        fn grid(&self) -> Vec<Vec<Option<u16>>> {
            (0..self.region.height)
                .map(|row| {
                    (0..self.region.width)
                        .map(|col| self.tile(col, row))
                        .collect()
                })
                .collect()
        }

        fn at(&self, col: u32, row: u32) -> Resolved {
            resolve_cell(
                &self.set,
                &self.terrain,
                &self.links,
                self.region.world(col, row),
            )
        }
    }

    fn some<const N: usize>(indices: [u16; N]) -> [Option<u16>; N] {
        indices.map(Some)
    }

    #[test]
    fn straight_runs_resolve_to_caps_and_middles() {
        for path in [DRY_PATH, TILLED_PATH] {
            let s = if path.terrain == DRY { '=' } else { '#' };
            let [top, mid, bottom] = some(path.vertical);
            let [left, centre, right] = some(path.horizontal);
            let one = Case::new(farm_set(), &format!("...\n.{s}.\n..."));
            assert!(some(path.isolated).contains(&one.tile(1, 1)));
            assert!(one.at(1, 1).is_exact());
            for (n, expected) in [
                (2, vec![top, bottom]),
                (5, vec![top, mid, mid, mid, bottom]),
            ] {
                let mut rows = vec!["...".to_string()];
                rows.extend((0..n).map(|_| format!(".{s}.")));
                rows.push("...".to_string());
                let case = Case::new(farm_set(), &rows.join("\n"));
                let column: Vec<_> = (1..=n).map(|row| case.tile(1, row)).collect();
                assert_eq!(column, expected);
                assert!((1..=n).all(|row| case.at(1, row).is_exact()));
            }
            for (n, expected) in [
                (2, vec![left, right]),
                (5, vec![left, centre, centre, centre, right]),
            ] {
                let dots = ".".repeat(n as usize + 2);
                let run: String = std::iter::repeat_n(s, n as usize).collect();
                let case = Case::new(farm_set(), &format!("{dots}\n.{run}.\n{dots}"));
                let line: Vec<_> = (1..=n).map(|col| case.tile(col, 1)).collect();
                assert_eq!(line, expected);
                assert!((1..=n).all(|col| case.at(col, 1).is_exact()));
            }
        }
    }

    #[test]
    fn grass_resolves_empty() {
        let out = Case::new(farm_set(), "....\n.==.\n....").grid();
        assert_eq!(out[0], vec![None; 4]);
        assert_eq!(out[1][0], None);
        assert_eq!(out[1][3], None);
        assert_eq!(out[1][1], Some(DRY_PATH.horizontal[0]));
    }

    #[test]
    fn l_turn_arms_are_exact_and_set_order_picks_the_corner_cap() {
        let [v_top, v_mid, v_bottom] = some(DRY_PATH.vertical);
        let [h_left, h_mid, h_right] = some(DRY_PATH.horizontal);
        let down_right = ".....\n.===.\n.=...\n.=...\n.....";
        let case = Case::new(farm_set(), down_right);
        assert_eq!([case.tile(2, 1), case.tile(3, 1)], [h_mid, h_right]);
        assert_eq!([case.tile(1, 2), case.tile(1, 3)], [v_mid, v_bottom]);
        for (col, row) in [(2, 1), (3, 1), (1, 2), (1, 3)] {
            assert!(case.at(col, row).is_exact());
        }
        let corner = case.at(1, 1);
        assert_eq!((corner.score, corner.max_score), (4, 5));
        let mirrors = [
            (down_right, (1, 1), [v_top, h_left]),
            (
                ".....\n.===.\n...=.\n...=.\n.....",
                (3, 1),
                [v_top, h_right],
            ),
            (
                ".....\n.=...\n.=...\n.===.\n.....",
                (1, 3),
                [v_bottom, h_left],
            ),
            (
                ".....\n...=.\n...=.\n.===.\n.....",
                (3, 3),
                [v_bottom, h_right],
            ),
        ];
        for (rows, (col, row), [vertical_cap, horizontal_cap]) in mirrors {
            let vertical_first = Case::new(farm_set(), rows).tile(col, row);
            let horizontal_first = Case::new(farm_set_horizontal_first(), rows).tile(col, row);
            assert_eq!(vertical_first, vertical_cap);
            assert_eq!(horizontal_first, horizontal_cap);
        }
    }

    #[test]
    fn t_junction_keeps_the_through_run() {
        let [_, v_mid, v_bottom] = some(DRY_PATH.vertical);
        let [h_left, h_mid, h_right] = some(DRY_PATH.horizontal);
        let case = Case::new(farm_set(), ".......\n.=====.\n...=...\n...=...\n.......");
        let top: Vec<_> = (1..6).map(|col| case.tile(col, 1)).collect();
        assert_eq!(top, [h_left, h_mid, h_mid, h_mid, h_right]);
        assert_eq!([case.tile(3, 2), case.tile(3, 3)], [v_mid, v_bottom]);
        let side = Case::new(farm_set(), ".....\n.=...\n.==..\n.=...\n.....");
        assert_eq!(side.tile(1, 2), v_mid);
        assert_eq!(side.tile(2, 2), h_right);
    }

    #[test]
    fn cross_centre_is_a_middle_chosen_by_set_order() {
        let [v_top, v_mid, v_bottom] = some(DRY_PATH.vertical);
        let [h_left, h_mid, h_right] = some(DRY_PATH.horizontal);
        let cross = ".......\n...=...\n...=...\n.=====.\n...=...\n...=...\n.......";
        let case = Case::new(farm_set(), cross);
        let row: Vec<_> = [1, 2, 4, 5].map(|col| case.tile(col, 3)).to_vec();
        assert_eq!(row, [h_left, h_mid, h_mid, h_right]);
        let column: Vec<_> = [1, 2, 4, 5].map(|r| case.tile(3, r)).to_vec();
        assert_eq!(column, [v_top, v_mid, v_mid, v_bottom]);
        assert_eq!(case.tile(3, 3), v_mid);
        let flipped = Case::new(farm_set_horizontal_first(), cross);
        assert_eq!(flipped.tile(3, 3), h_mid);
    }

    #[test]
    fn straight_run_beats_an_equal_scoring_turn() {
        let set = add_path(
            TerrainSet::new(MatchMode::Sides).with_tile(path_tile(
                99,
                DRY,
                &[Peer::Right, Peer::Bottom],
                1,
            )),
            &DRY_PATH,
            false,
        );
        let tee = Case::new(set.clone(), ".......\n.=====.\n...=...\n.......");
        assert_eq!(tee.tile(3, 1), Some(DRY_PATH.horizontal[1]));
        let turn = Case::new(set, "....\n.==.\n.=..\n....");
        assert_eq!(turn.tile(1, 1), Some(99));
        assert!(turn.at(1, 1).is_exact());
    }

    #[test]
    fn zero_weight_straight_loses_to_a_weighted_turn() {
        let set = TerrainSet::new(MatchMode::Sides)
            .with_tile(path_tile(
                DRY_PATH.horizontal[1],
                DRY,
                &[Peer::Left, Peer::Right],
                0,
            ))
            .with_tile(path_tile(99, DRY, &[Peer::Right, Peer::Bottom], 1));
        let tee = Case::new(set, ".......\n.=====.\n...=...\n.......");
        assert_eq!(tee.tile(3, 1), Some(99));
    }

    #[test]
    fn path_end_without_a_cap_shows_the_isolated_tile() {
        let [a, b] = DRY_PATH.isolated;
        let set = TerrainSet::new(MatchMode::Sides)
            .with_tile(path_tile(
                DRY_PATH.vertical[1],
                DRY,
                &[Peer::Top, Peer::Bottom],
                1,
            ))
            .with_tile(path_tile(a, DRY, &[], 1))
            .with_tile(path_tile(b, DRY, &[], 1));
        let case = Case::new(set, "...\n.=.\n.=.\n.=.\n...");
        assert!(some(DRY_PATH.isolated).contains(&case.tile(1, 1)));
        assert!(some(DRY_PATH.isolated).contains(&case.tile(1, 3)));
        assert_eq!(case.tile(1, 2), Some(DRY_PATH.vertical[1]));
    }

    #[test]
    fn adjacent_parallel_strips_connect() {
        let [v_top, v_mid, _] = some(DRY_PATH.vertical);
        let [h_left, _, h_right] = some(DRY_PATH.horizontal);
        let case = Case::new(farm_set(), "....\n.==.\n.==.\n.==.\n....");
        let req = required_peers(
            &case.set,
            &case.terrain,
            &no_links(),
            case.region.world(1, 2),
        );
        assert_eq!(req[Peer::Right.index()], Is(DRY));
        assert_eq!([case.tile(1, 2), case.tile(2, 2)], [v_mid, v_mid]);
        assert!(!case.at(1, 2).is_exact() && !case.at(2, 2).is_exact());
        assert!([v_top, h_left].contains(&case.tile(1, 1)));
        assert!([v_top, h_right].contains(&case.tile(2, 1)));
    }

    #[test]
    fn dry_beside_tilled_has_no_transition() {
        let pair = Case::new(farm_set(), "....\n.=#.\n....");
        assert!(some(DRY_PATH.isolated).contains(&pair.tile(1, 1)));
        assert!(some(TILLED_PATH.isolated).contains(&pair.tile(2, 1)));
        let rows = "......\n.==##.\n......";
        let runs = Case::new(farm_set(), rows);
        let line: Vec<_> = (1..5).map(|col| runs.tile(col, 1)).collect();
        let expected = [
            DRY_PATH.horizontal[0],
            DRY_PATH.horizontal[2],
            TILLED_PATH.horizontal[0],
            TILLED_PATH.horizontal[2],
        ];
        assert_eq!(line, some(expected));
        assert!(!runs.at(2, 1).is_exact() && !runs.at(3, 1).is_exact());
        assert_eq!(Case::new(farm_set(), rows).grid(), runs.grid());
    }

    #[test]
    fn isolated_cells_use_both_variants_stably() {
        let mut rows = Vec::new();
        for _ in 0..10 {
            rows.push(".".repeat(41));
            rows.push(format!(".{}", "=.".repeat(20)));
        }
        rows.push(".".repeat(41));
        let rows = rows.join("\n");
        let case = Case::new(farm_set(), &rows);
        let picks: Vec<_> = case.grid().into_iter().flatten().flatten().collect();
        assert_eq!(picks.len(), 200);
        assert!(picks.iter().all(|i| DRY_PATH.isolated.contains(i)));
        let first = picks.iter().filter(|i| **i == DRY_PATH.isolated[0]).count();
        assert!((60..=140).contains(&first), "{first}");
        assert_eq!(Case::new(farm_set(), &rows).grid(), case.grid());
    }

    fn column(col: u32, rows: std::ops::RangeInclusive<u32>) -> Vec<(u32, u32)> {
        rows.map(|row| (col, row)).collect()
    }

    fn row(row: u32, cols: std::ops::RangeInclusive<u32>) -> Vec<(u32, u32)> {
        cols.map(|col| (col, row)).collect()
    }

    const STRIPS: &str = "....\n....\n....\n....\n....";

    #[test]
    fn parallel_path_strips_stay_apart_after_a_resolve() {
        let vertical = some(DRY_PATH.vertical).to_vec();
        let mut case = Case::new(farm_set(), STRIPS);
        case.path(&column(1, 1..=3), DRY);
        case.path(&column(2, 1..=3), DRY);
        for col in [1, 2] {
            let tiles: Vec<_> = (1..=3).map(|row| case.tile(col, row)).collect();
            assert_eq!(tiles, vertical, "column {col}");
            assert!((1..=3).all(|row| case.at(col, row).is_exact()));
        }
        assert_eq!(case.required(1, 2)[Peer::Right.index()], Not(DRY));
        assert_eq!(case.resolve_all(), case.grid());
    }

    #[test]
    fn parallel_connect_strips_join() {
        let mut case = Case::new(farm_set(), STRIPS);
        case.connect(&column(1, 1..=3), DRY);
        case.connect(&column(2, 1..=3), DRY);
        let joined = Case::new(farm_set(), "....\n.==.\n.==.\n.==.\n....");
        assert_eq!(case.grid(), joined.grid());
        assert_eq!(case.required(1, 2)[Peer::Right.index()], Is(DRY));
    }

    #[test]
    fn crossing_path_keeps_the_first_links_and_joins_at_the_crossing() {
        let [v_top, v_mid, v_bottom] = some(DRY_PATH.vertical);
        let [h_left, h_mid, h_right] = some(DRY_PATH.horizontal);
        let grass = ".......\n".repeat(7);
        let mut case = Case::new(farm_set(), grass.trim_end());
        case.path(&row(3, 1..=5), DRY);
        case.path(&column(3, 1..=5), DRY);
        let all = [Peer::Top, Peer::Left, Peer::Right, Peer::Bottom];
        let centre = case.links.get(case.region.world(3, 3));
        assert!(centre.is_path() && all.iter().all(|side| centre.has(*side)));
        let across: Vec<_> = [1, 2, 4, 5].map(|col| case.tile(col, 3)).to_vec();
        assert_eq!(across, [h_left, h_mid, h_mid, h_right]);
        let down: Vec<_> = [1, 2, 4, 5].map(|r| case.tile(3, r)).to_vec();
        assert_eq!(down, [v_top, v_mid, v_mid, v_bottom]);
        assert_eq!(case.tile(3, 3), v_mid);
        assert_eq!(case.resolve_all(), case.grid());
    }

    #[test]
    fn connect_over_a_path_cell_rejoins_connect_neighbours() {
        let [v_top, ..] = some(DRY_PATH.vertical);
        let mut case = Case::new(farm_set(), STRIPS);
        case.path(&column(1, 1..=3), DRY);
        case.connect(&column(2, 1..=3), DRY);
        assert_eq!(case.tile(1, 2), Some(DRY_PATH.vertical[1]));
        case.connect(&[(1, 2)], DRY);
        assert_eq!(case.links.get(case.region.world(1, 2)), Links::EMPTY);
        assert_eq!(case.tile(1, 2), Some(DRY_PATH.horizontal[0]));
        assert_eq!(case.required(1, 2)[Peer::Top.index()], Not(DRY));
        assert!(some(DRY_PATH.isolated).contains(&case.tile(1, 1)));
        case.connect(&[(1, 1)], 0);
        assert_eq!(case.links.get(case.region.world(1, 1)), Links::EMPTY);
        assert_eq!(case.terrain.get(case.region.world(1, 1)), 0);
        assert_eq!(case.tile(1, 1), None);
        assert_eq!(case.tile(2, 1), v_top);
        assert_eq!(case.resolve_all(), case.grid());
    }

    #[test]
    fn path_beside_connect_joins_only_when_both_mark_the_side() {
        let mut case = Case::new(farm_set(), "....\n....\n....");
        case.path(&[(1, 1)], DRY);
        case.connect(&[(2, 1)], DRY);
        assert_eq!(case.required(1, 1)[Peer::Right.index()], Not(DRY));
        assert_eq!(case.required(2, 1)[Peer::Left.index()], Not(DRY));
        case.path(&[(2, 1)], DRY);
        assert_eq!(case.required(1, 1)[Peer::Right.index()], Not(DRY));
        case.path(&[(1, 1), (2, 1)], DRY);
        assert_eq!(case.required(1, 1)[Peer::Right.index()], Is(DRY));
        assert_eq!(case.required(2, 1)[Peer::Left.index()], Is(DRY));
    }

    #[test]
    fn corners_need_both_sides_joined_by_links() {
        let set = TerrainSet::new(MatchMode::CornersAndSides);
        let terrain = layer(&[(0, 0, 1), (0, 1, 1), (1, 0, 1), (1, 1, 2)]);
        let mut links = no_links();
        let corner = Peer::TopRight.index();
        let origin = CellCoord::new(0, 0);
        assert_eq!(
            required_peers(&set, &terrain, &links, origin)[corner],
            Is(2)
        );
        links.set(origin, Links::EMPTY.with_path().with(Peer::Top));
        links.set(
            CellCoord::new(0, 1),
            Links::EMPTY.with_path().with(Peer::Bottom),
        );
        assert_eq!(required_peers(&set, &terrain, &links, origin)[corner], Any);
    }

    #[test]
    fn non_contiguous_path_stroke_writes_nothing() {
        let mut case = Case::new(farm_set(), STRIPS);
        let (terrain, links) = (case.terrain.clone(), case.links.clone());
        let cells = case.cells(&[(1, 1), (1, 3)]);
        let err = apply_path_stroke(&mut case.terrain, &mut case.links, &cells, DRY);
        assert!(err.is_err());
        assert_eq!((case.terrain, case.links), (terrain, links));
    }

    /// A 5x5 block of terrain 1 around the origin with the strokes painted in 2.
    fn painted(
        paths: &[&[(i32, i32)]],
        connect: &[(i32, i32)],
    ) -> (ChunkedLayer<TerrainId>, ChunkedLayer<Links>) {
        let mut terrain = ChunkedLayer::new();
        let mut links = no_links();
        for x in -2..=2 {
            for y in -2..=2 {
                terrain.set(CellCoord::new(x, y), 1);
            }
        }
        let cells =
            |c: &[(i32, i32)]| -> Vec<_> { c.iter().map(|&(x, y)| CellCoord::new(x, y)).collect() };
        for path in paths {
            apply_path_stroke(&mut terrain, &mut links, &cells(path), 2).expect("contiguous");
        }
        apply_connect_stroke(&mut terrain, &mut links, &cells(connect), 2);
        (terrain, links)
    }

    fn corner_shape(
        terrain: &ChunkedLayer<TerrainId>,
        links: &ChunkedLayer<Links>,
    ) -> (TerrainId, bool) {
        let set = LayoutTemplate::Blob47.terrain_set(2, 1);
        let r = resolve_cell(&set, terrain, links, CellCoord::new(0, 0));
        let tile = set
            .tiles
            .iter()
            .find(|t| t.tile == r.tile)
            .expect("tile in set");
        (tile.terrains.get(Peer::TopRight), r.is_exact())
    }

    #[test]
    fn unlinked_diagonal_path_cell_leaves_the_inner_corner_notched() {
        let l_turn: &[(i32, i32)] = &[(0, 1), (0, 0), (1, 0)];
        let (terrain, links) = painted(&[l_turn, &[(1, 1)]], &[]);
        let set = LayoutTemplate::Blob47.terrain_set(2, 1);
        let req = required_peers(&set, &terrain, &links, CellCoord::new(0, 0));
        assert_eq!(req[Peer::TopRight.index()], Not(2));
        assert_eq!(corner_shape(&terrain, &links), (1, true));
        let ring: &[(i32, i32)] = &[(0, 1), (0, 0), (1, 0), (1, 1), (0, 1)];
        let (terrain, links) = painted(&[ring], &[]);
        assert_eq!(corner_shape(&terrain, &links), (2, true));
        let (terrain, links) = painted(&[], &[(0, 1), (0, 0), (1, 0), (1, 1)]);
        assert_eq!(corner_shape(&terrain, &links), (2, true));
    }

    #[test]
    fn corners_mode_ignores_links() {
        let set = LayoutTemplate::Corner16.terrain_set(2, 1);
        let ring: &[(i32, i32)] = &[(0, 1), (0, 0), (1, 0), (1, 1)];
        let strip: &[(i32, i32)] = &[(-2, -1), (-1, -1), (0, -1)];
        let (terrain, links) = painted(&[ring, strip, &[(-1, 1)]], &[]);
        assert!(!links.is_empty());
        let dirty: Vec<_> = terrain.cells().map(|(c, _)| c).collect();
        let with = resolve_region(&set, &terrain, &links, &BTreeMap::new(), dirty.clone());
        let without = resolve_region(&set, &terrain, &no_links(), &BTreeMap::new(), dirty);
        assert_eq!(with, without);
    }
}
