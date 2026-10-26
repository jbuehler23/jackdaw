//! Layout fitting: turns labelled tiles into proposed terrain sets.
//!
//! - Pair: a terrain tile's inner terrain is its centre and its outer the most
//!   common other border label; ties go to the label most common around that
//!   centre across the tileset, then the lower id.
//! - Seam: over a peer's span along an edge (a corner region's, or for a side
//!   the centre region's `n/4`, from `terrain_find`'s regions), the mean distance from each edge pixel to the
//!   closest pixel of the partner's opposite edge within [`SEAM_SHIFT`] along
//!   it, both ways. Distances are Oklab, 1 against a transparent pixel; pixels
//!   transparent in both edges are skipped, and nothing compared is no
//!   evidence. Partners are the pair's tiles and the inner terrain's pure
//!   tiles whose mirrored peer is labelled inner, deduplicated by edge.
//! - Correction: a border peer not labelled outer joins, becoming inner, when
//!   its best seam on an edge it touches is within [`SEAM_GOOD`], and becomes
//!   outer otherwise; an outer peer below [`LOW_CONFIDENCE`] is checked
//!   against half that and joins within it. The peer's confidence is its
//!   distance from the threshold used, `|seam - t| / t`, at most 1. Without
//!   evidence the label stays, at confidence 0.
//! - Mode: each mode is charged the confidence of the peers it cannot
//!   reproduce. All modes but `Corners` miss inner corners beside an outer
//!   side; `Sides` also misses enclosed corners that differ among tiles of the
//!   same sides (the lesser share per corner); `Corners` misses sides unlike
//!   both their corners when those agree (between mixed corners the border
//!   crosses the side, so its label is arbitrary). The lowest charge wins,
//!   ties in the order `Sides`, `Corners`, `CornersAndSides`. Charges are
//!   summed over both orders of a terrain pair; if `Corners` wins, both
//!   orders form one set, its inner terrain the one centring more tiles.
//!   Otherwise each order picks its own mode.
//! - Normalising: peers the mode does not read (sides in `Corners`, corners in
//!   `Sides`, corners beside an outer side in `CornersAndSides`) take the
//!   tile's other terrain. A tile whose corrections turn a read peer
//!   transparent or opaque leaves the set.
//! - Fit: a set needs at least [`MIN_SHAPES`] distinct transition shapes (the
//!   smallest template) whose read outer peers close all four sides or all
//!   four corners, counting only tiles whose centre and read peers reach
//!   [`LOW_CONFIDENCE`]; low-confidence tiles join a set but cannot found it.
//! - Coverage: the 16 side, 16 corner or 47 blob shapes of the mode some tile
//!   has, by corners alone in `Corners`; the largest template of the mode with
//!   [`TEMPLATE_SHARE`] of its shapes present is named.
//! - Terrains: background when outer of a set, inner of none and without a
//!   pure tile; dropped when in no set and without a pure tile. Terrain tiles
//!   in no set, other than kept terrains' pure tiles, are unassigned.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Range;

use glam::{IVec2, Vec3};

use crate::solver::{MatchMode, Peer, TerrainSet, TerrainTile, TileTerrains};
use crate::terrain_find::{MIN_ALPHA, region_rects, srgb_to_oklab};
use crate::{
    LayoutTemplate, SheetInput, TerrainAnalysis, TerrainId, TileAddress, TileKind, TileLabels,
    TileRef,
};

/// Largest mean Oklab distance of a seam that joins two tiles.
const SEAM_GOOD: f32 = 0.03;
/// Pixels a drawn edge may wander along a seam, for ragged hand-drawn borders.
const SEAM_SHIFT: usize = 1;
/// Below this, a peer or a tile is reported as low confidence.
const LOW_CONFIDENCE: f32 = 0.5;
/// Distinct transition shapes of the smallest template.
const MIN_SHAPES: usize = 4;
/// Share of a template's shapes present to name it.
const TEMPLATE_SHARE: f32 = 0.9;
const BORDER: [Peer; 8] = [
    Peer::TopLeft,
    Peer::Top,
    Peer::TopRight,
    Peer::Left,
    Peer::Right,
    Peer::BottomLeft,
    Peer::Bottom,
    Peer::BottomRight,
];
const MODES: [MatchMode; 3] = [
    MatchMode::Sides,
    MatchMode::Corners,
    MatchMode::CornersAndSides,
];

/// Edge pixels in Oklab, `None` when transparent, in [`Peer::SIDES`] order;
/// rows run left to right and columns top to bottom.
type Edges = [Vec<Option<Vec3>>; 4];
/// Distinct partner edges by the partner's inner peer and the side it lies on.
type Partners<'a> = HashMap<(Peer, usize), Vec<&'a [Option<Vec3>]>>;

/// A fitted tile: its peers after correction, with confidence per peer.
#[derive(Clone, Debug, PartialEq)]
pub struct FittedTile {
    pub address: TileAddress,
    pub terrains: TileTerrains,
    pub confidence: [f32; 9],
    /// Peers the set's mode reads whose terrain differs from the label.
    pub corrected: [bool; 9],
}

/// Tiles of `inner` over `outer`; in `Corners` also of `outer` over `inner`.
#[derive(Clone, Debug, PartialEq)]
pub struct ProposedSet {
    pub mode: MatchMode,
    pub inner: TerrainId,
    pub outer: TerrainId,
    /// Transition tiles and the pure tiles of its inner terrains, by address.
    pub tiles: Vec<FittedTile>,
    /// Shapes of the mode's full layout (16 side, 16 corner or 47 blob
    /// shapes) that some tile has.
    pub covered: usize,
    pub total: usize,
    pub missing: Vec<TileTerrains>,
    pub template: Option<LayoutTemplate>,
}

impl ProposedSet {
    /// A solver set of the tiles `tile_ref` maps, weight 1, outer below inner.
    pub fn terrain_set(&self, tile_ref: impl Fn(TileAddress) -> Option<TileRef>) -> TerrainSet {
        let mut set = TerrainSet::new(self.mode).with_order([self.outer, self.inner]);
        for tile in &self.tiles {
            if let Some(r) = tile_ref(tile.address) {
                set = set.with_tile(TerrainTile::new(r, tile.terrains, 1));
            }
        }
        set
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProposedTerrain {
    pub id: TerrainId,
    pub background: bool,
    pub pure_tiles: Vec<TileAddress>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TilesetProposal {
    pub sets: Vec<ProposedSet>,
    /// Kept terrains by id; dropped terrains are absent.
    pub terrains: Vec<ProposedTerrain>,
    /// Terrain tiles in no set, other than kept terrains' pure tiles.
    pub unassigned: Vec<TileAddress>,
    pub low_confidence: Vec<TileAddress>,
}

/// Sheets given to [`fit_layouts`] that do not match the analysis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitError {
    SheetCount {
        inputs: usize,
        analysed: usize,
    },
    TileCount {
        sheet: usize,
        inputs: usize,
        analysed: usize,
    },
}

impl std::fmt::Display for FitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FitError::SheetCount { inputs, analysed } => {
                write!(f, "{inputs} sheets given for {analysed} analysed")
            }
            FitError::TileCount {
                sheet,
                inputs,
                analysed,
            } => write!(
                f,
                "sheet {sheet}: {inputs} tiles given for {analysed} analysed"
            ),
        }
    }
}

impl std::error::Error for FitError {}

/// One order of a terrain pair after correction, with its mode charges.
struct Half {
    inner: TerrainId,
    outer: TerrainId,
    /// Corrected tiles with their labels.
    tiles: Vec<(FittedTile, TileTerrains)>,
    charges: [f32; 3],
}

/// Fits layouts to the terrain tiles of `analysis`, whose sheets are `inputs`.
pub fn fit_layouts(
    analysis: &TerrainAnalysis,
    inputs: &[SheetInput],
) -> Result<TilesetProposal, FitError> {
    check_inputs(analysis, inputs)?;
    let terrain_tiles: Vec<(TileAddress, &TileLabels)> = (analysis.tiles.iter().enumerate())
        .flat_map(|(sheet, tiles)| {
            let tiles = tiles.iter().enumerate();
            tiles.filter_map(move |(tile, l)| {
                let address = TileAddress {
                    sheet,
                    tile: tile as u32,
                };
                l.as_ref().map(|l| (address, l))
            })
        })
        .filter(|(_, l)| l.kind == TileKind::Terrain)
        .collect();
    let mut around: HashMap<(TerrainId, TerrainId), usize> = HashMap::new();
    for (_, l) in &terrain_tiles {
        let centre = l.terrains.centre();
        for t in BORDER.map(|p| l.terrains.get(p)) {
            if t != centre {
                *around.entry((centre, t)).or_default() += 1;
            }
        }
    }
    let mut pairs: BTreeMap<(TerrainId, TerrainId), Vec<TileAddress>> = BTreeMap::new();
    for (address, l) in &terrain_tiles {
        let centre = l.terrains.centre();
        if let Some(outer) = outer_of(&l.terrains, &around).filter(|_| centre != 0) {
            pairs.entry((centre, outer)).or_default().push(*address);
        }
    }
    let edges: HashMap<TileAddress, Edges> = (terrain_tiles.iter())
        .filter_map(|(a, _)| Some((*a, tile_edges(inputs.get(a.sheet)?, a.tile)?)))
        .collect();
    let mut groups: BTreeMap<(TerrainId, TerrainId), Vec<Half>> = BTreeMap::new();
    for ((inner, outer), tiles) in pairs {
        let half = correct_pair(analysis, &edges, inner, outer, &tiles);
        groups
            .entry((inner.min(outer), inner.max(outer)))
            .or_default()
            .push(half);
    }
    let mut sets = Vec::new();
    for halves in groups.into_values() {
        let summed = halves.iter().fold([0.0; 3], |sum, h| {
            std::array::from_fn(|i| sum[i] + h.charges[i])
        });
        if pick(summed) == MatchMode::Corners {
            sets.extend(finish(MatchMode::Corners, halves));
        } else {
            for half in halves {
                sets.extend(finish(pick(half.charges), vec![half]));
            }
        }
    }

    let inners: BTreeSet<TerrainId> = sets.iter().map(|s| s.inner).collect();
    let outers: BTreeSet<TerrainId> = sets.iter().map(|s| s.outer).collect();
    let kept: Vec<_> = (analysis.terrains.iter())
        .filter(|t| inners.contains(&t.id) || outers.contains(&t.id) || !t.pure_tiles.is_empty())
        .collect();
    let terrains = (kept.iter())
        .map(|t| ProposedTerrain {
            id: t.id,
            background: outers.contains(&t.id)
                && !inners.contains(&t.id)
                && t.pure_tiles.is_empty(),
            pure_tiles: t.pure_tiles.clone(),
        })
        .collect();
    let mut placed: BTreeSet<TileAddress> = (kept.iter())
        .flat_map(|t| t.pure_tiles.iter().copied())
        .collect();
    let mut doubtful = BTreeSet::new();
    for set in &sets {
        for tile in &set.tiles {
            placed.insert(tile.address);
            if low_confidence(set.mode, tile) {
                doubtful.insert(tile.address);
            }
        }
    }
    let unassigned = (terrain_tiles.iter())
        .map(|(a, _)| *a)
        .filter(|a| !placed.contains(a))
        .collect();
    Ok(TilesetProposal {
        sets,
        terrains,
        unassigned,
        low_confidence: doubtful.into_iter().collect(),
    })
}

pub(crate) fn check_inputs(
    analysis: &TerrainAnalysis,
    inputs: &[SheetInput],
) -> Result<(), FitError> {
    if inputs.len() != analysis.tiles.len() {
        return Err(FitError::SheetCount {
            inputs: inputs.len(),
            analysed: analysis.tiles.len(),
        });
    }
    for (sheet, (input, tiles)) in inputs.iter().zip(&analysis.tiles).enumerate() {
        let size = glam::UVec2::new(input.image.width, input.image.height);
        let count = input.layout.tile_count() as usize;
        let unusable = tiles.is_empty() && !input.layout.fits(size);
        if tiles.len() != count && !unusable {
            return Err(FitError::TileCount {
                sheet,
                inputs: count,
                analysed: tiles.len(),
            });
        }
    }
    Ok(())
}

/// The most common border label other than the centre, `None` for a pure
/// tile.
fn outer_of(
    terrains: &TileTerrains,
    around: &HashMap<(TerrainId, TerrainId), usize>,
) -> Option<TerrainId> {
    let centre = terrains.centre();
    let mut counts: BTreeMap<TerrainId, usize> = BTreeMap::new();
    for t in BORDER.map(|p| terrains.get(p)) {
        if t != centre {
            *counts.entry(t).or_default() += 1;
        }
    }
    let rank = |t: TerrainId, n: usize| {
        let seen = around.get(&(centre, t)).copied().unwrap_or(0);
        (n, seen, std::cmp::Reverse(t))
    };
    let best = counts.into_iter().max_by_key(|&(t, n)| rank(t, n));
    best.map(|(t, _)| t)
}

fn tile_edges(input: &SheetInput, tile: u32) -> Option<Edges> {
    let side = |slot: usize| side_pixels(input, tile, slot);
    Some([side(0)?, side(1)?, side(2)?, side(3)?])
}

/// The pixels along side `slot` ([`Peer::SIDES`] order) of `tile`; `None`
/// when the tile is outside the layout.
pub(crate) fn side_pixels(input: &SheetInput, tile: u32, slot: usize) -> Option<Vec<Option<Vec3>>> {
    if tile >= input.layout.tile_count() {
        return None;
    }
    let size = input.layout.tile;
    let along = if slot == 1 || slot == 2 {
        size.y
    } else {
        size.x
    };
    Some(
        (0..along)
            .map(|i| side_pixel(input, tile, slot, 0, i))
            .collect(),
    )
}

/// Pixel `i` along side `slot` of `tile`, `depth` pixels in, in Oklab;
/// `None` when transparent or outside the tile.
pub(crate) fn side_pixel(
    input: &SheetInput,
    tile: u32,
    slot: usize,
    depth: u32,
    i: u32,
) -> Option<Vec3> {
    let (origin, size) = input.layout.tile_rect(tile);
    let vertical = slot == 1 || slot == 2;
    let (along, across) = if vertical {
        (size.y, size.x)
    } else {
        (size.x, size.y)
    };
    if tile >= input.layout.tile_count() || i >= along || depth >= across {
        return None;
    }
    let inward = if slot >= 2 { across - 1 - depth } else { depth };
    let (x, y) = if vertical { (inward, i) } else { (i, inward) };
    let p = input.image.pixel(origin.x + x, origin.y + y)?;
    (f32::from(p[3]) / 255.0 >= MIN_ALPHA).then(|| srgb_to_oklab(p))
}

/// Corrects the tiles of `inner` over `outer` and charges each mode.
fn correct_pair(
    analysis: &TerrainAnalysis,
    edges: &HashMap<TileAddress, Edges>,
    inner: TerrainId,
    outer: TerrainId,
    tiles: &[TileAddress],
) -> Half {
    let pure = analysis.terrain(inner).map_or(&[][..], |t| &t.pure_tiles);
    let members: Vec<(TileAddress, &TileLabels)> = (tiles.iter().chain(pure))
        .filter_map(|a| Some((*a, analysis.tile(*a)?)))
        .collect();
    let partners = partners(&members, edges, inner);
    let fitted: Vec<(FittedTile, TileTerrains)> = (members.iter())
        .map(|&(address, labels)| {
            let mut tile = FittedTile {
                address,
                terrains: labels.terrains,
                confidence: labels.confidence,
                corrected: [false; 9],
            };
            if !pure.contains(&address) {
                let own = edges.get(&address);
                correct(&mut tile, own, &partners, inner, outer);
            }
            (tile, labels.terrains)
        })
        .collect();
    let charges = charges(fitted.iter().map(|(t, _)| t), inner);
    Half {
        inner,
        outer,
        tiles: fitted,
        charges,
    }
}

fn partners<'a>(
    members: &[(TileAddress, &TileLabels)],
    edges: &'a HashMap<TileAddress, Edges>,
    inner: TerrainId,
) -> Partners<'a> {
    let mut partners: Partners = HashMap::new();
    for (address, labels) in members {
        let Some(own) = edges.get(address) else {
            continue;
        };
        let size = (own[0].len() as u32, own[1].len() as u32);
        for (slot, side) in Peer::SIDES.into_iter().enumerate() {
            let edge = &own[slot];
            let on_side = BORDER.into_iter().filter(|p| touches(*p, side));
            for peer in on_side.filter(|p| labels.terrains.get(*p) == inner) {
                let window = widen(segment(peer, side, size.0, size.1), edge.len());
                let list = partners.entry((peer, slot)).or_default();
                let seen = |other: &&[Option<Vec3>]| {
                    other.len() == edge.len()
                        && other.get(window.clone()) == edge.get(window.clone())
                };
                if !list.iter().any(seen) {
                    list.push(edge);
                }
            }
        }
    }
    partners
}

/// Corrects the border peers of `tile` by seam evidence; see the module docs.
fn correct(
    tile: &mut FittedTile,
    own: Option<&Edges>,
    partners: &Partners,
    inner: TerrainId,
    outer: TerrainId,
) {
    for peer in BORDER {
        let label = tile.terrains.get(peer);
        if label == outer && tile.confidence[peer.index()] >= LOW_CONFIDENCE {
            continue;
        }
        let Some(seam) = own.and_then(|own| best_seam(own, partners, peer)) else {
            tile.confidence[peer.index()] = 0.0;
            continue;
        };
        let reach = if label == outer {
            SEAM_GOOD / 2.0
        } else {
            SEAM_GOOD
        };
        let joins = seam <= reach;
        tile.terrains = tile.terrains.with(peer, if joins { inner } else { outer });
        tile.confidence[peer.index()] = ((seam - reach).abs() / reach).min(1.0);
    }
}

/// Lowest seam between `peer` of a tile with edges `own` and a partner
/// continuing it, over the edges the peer touches; `None` without evidence.
fn best_seam(own: &Edges, partners: &Partners, peer: Peer) -> Option<f32> {
    let size = (own[0].len() as u32, own[1].len() as u32);
    let mut best: Option<f32> = None;
    for (slot, side) in Peer::SIDES.into_iter().enumerate() {
        let Some(mirrored) = mirror(peer, side).filter(|_| touches(peer, side)) else {
            continue;
        };
        let mine = &own[slot];
        let range = segment(peer, side, size.0, size.1);
        let opposite = Peer::SIDES.len() - 1 - slot;
        let theirs = partners.get(&(mirrored, opposite)).into_iter().flatten();
        for theirs in theirs.filter(|t| t.len() == mine.len()) {
            if let Some(s) = seam(mine, theirs, range.clone()) {
                best = Some(best.map_or(s, |b| b.min(s)));
            }
        }
    }
    best
}

/// Whether `peer` lies on `side` of the tile.
fn touches(peer: Peer, side: Peer) -> bool {
    let (at, across) = (peer.offset(), side.offset());
    if across.y != 0 {
        at.y == across.y
    } else {
        at.x == across.x
    }
}

/// The peer of the neighbour across `side` that meets `peer`.
fn mirror(peer: Peer, side: Peer) -> Option<Peer> {
    let (at, across) = (peer.offset(), side.offset());
    if across.y != 0 {
        Peer::from_offset(IVec2::new(at.x, -at.y))
    } else {
        Peer::from_offset(IVec2::new(-at.x, at.y))
    }
}

/// The pixels along `side` of a `w x h` tile that decide `peer`: a corner
/// region's span, or for a side the centre region's, where a joined centre
/// must cross the edge.
fn segment(peer: Peer, side: Peer, w: u32, h: u32) -> Range<usize> {
    let region = if peer.corner_sides().is_some() {
        peer
    } else {
        Peer::Centre
    };
    let [x, y, rw, rh] = region_rects(w, h)[region.index()];
    let (start, len) = if side.offset().y != 0 {
        (x, rw)
    } else {
        (y, rh)
    };
    start as usize..(start + len) as usize
}

pub(crate) fn widen(range: Range<usize>, len: usize) -> Range<usize> {
    range.start.saturating_sub(SEAM_SHIFT)..(range.end + SEAM_SHIFT).min(len)
}

/// Mean over `range` of each pixel's distance to the nearest pixel of the
/// other edge within [`SEAM_SHIFT`], both ways; `None` when nothing compares.
pub(crate) fn seam(a: &[Option<Vec3>], b: &[Option<Vec3>], range: Range<usize>) -> Option<f32> {
    let (mut sum, mut count) = (0.0, 0usize);
    for (x, y) in [(a, b), (b, a)] {
        for i in range.clone() {
            let (Some(p), Some(q)) = (x.get(i), y.get(i)) else {
                continue;
            };
            if p.is_none() && q.is_none() {
                continue;
            }
            let near = y.get(widen(i..i + 1, y.len())).unwrap_or_default();
            sum += (near.iter())
                .map(|q| distance(*p, *q))
                .fold(f32::INFINITY, f32::min);
            count += 1;
        }
    }
    (count > 0).then(|| sum / count as f32)
}

/// Oklab distance; 0 between transparent pixels and 1 against one.
fn distance(p: Option<Vec3>, q: Option<Vec3>) -> f32 {
    match p.zip(q) {
        Some((p, q)) => p.distance(q),
        None if p.is_some() || q.is_some() => 1.0,
        None => 0.0,
    }
}

/// Charges for [`MODES`]; see the module docs.
fn charges<'a>(tiles: impl Iterator<Item = &'a FittedTile>, inner: TerrainId) -> [f32; 3] {
    let is_inner = |t: &FittedTile, p: Peer| t.terrains.get(p) == inner;
    let (mut loose, mut by_corners) = (0.0, 0.0);
    let mut groups: BTreeMap<[bool; 4], [[f32; 2]; 4]> = BTreeMap::new();
    for t in tiles {
        let sides = Peer::SIDES.map(|s| is_inner(t, s));
        let group = groups.entry(sides).or_default();
        for (k, c) in Peer::CORNERS.into_iter().enumerate() {
            let conf = t.confidence[c.index()];
            if !t.terrains.corner_enclosed(c) {
                loose += if is_inner(t, c) { conf } else { 0.0 };
            } else {
                group[k][usize::from(is_inner(t, c))] += conf;
            }
        }
        for s in Peer::SIDES {
            let ends = Peer::CORNERS.into_iter().filter(|c| touches(*c, s));
            let inner_ends = ends.filter(|c| is_inner(t, *c)).count();
            if (inner_ends == 2 && !is_inner(t, s)) || (inner_ends == 0 && is_inner(t, s)) {
                by_corners += t.confidence[s.index()];
            }
        }
    }
    let variants: f32 = groups.values().flatten().map(|[a, b]| a.min(*b)).sum();
    [loose + variants, by_corners, loose]
}

/// The mode of the lowest charge, ties in [`MODES`] order.
fn pick(charges: [f32; 3]) -> MatchMode {
    let mut best = 0;
    for (i, c) in charges.iter().enumerate() {
        if *c < charges[best] {
            best = i;
        }
    }
    MODES[best]
}

/// Whether the centre or a peer `mode` reads is below [`LOW_CONFIDENCE`].
fn low_confidence(mode: MatchMode, tile: &FittedTile) -> bool {
    let read = BORDER
        .into_iter()
        .filter(|p| reads(mode, &tile.terrains, *p));
    let mut peers = read.chain([Peer::Centre]);
    peers.any(|p| tile.confidence[p.index()] < LOW_CONFIDENCE)
}

/// Whether `mode` reads peer `p` of a normalised tile: sides when it uses
/// them, corners in `Corners` and enclosed corners in `CornersAndSides`.
fn reads(mode: MatchMode, t: &TileTerrains, p: Peer) -> bool {
    if p.corner_sides().is_none() {
        return p != Peer::Centre && mode.uses_sides();
    }
    match mode {
        MatchMode::Corners => true,
        MatchMode::CornersAndSides => t.corner_enclosed(p),
        MatchMode::Sides => false,
    }
}

/// Normalises the halves' tiles to `mode`, checks the fit and reports
/// coverage; see the module docs.
fn finish(mode: MatchMode, halves: Vec<Half>) -> Option<ProposedSet> {
    let main = (halves.iter()).fold(None::<&Half>, |best, h| match best {
        Some(b) if b.tiles.len() >= h.tiles.len() => Some(b),
        _ => Some(h),
    })?;
    let (inner, outer) = (main.inner, main.outer);
    let other = |t: &TileTerrains| if t.centre() == inner { outer } else { inner };
    let mut tiles = Vec::new();
    for (mut tile, raw) in halves.into_iter().flat_map(|h| h.tiles) {
        let fill = other(&tile.terrains);
        for peer in BORDER {
            let loose = mode == MatchMode::CornersAndSides && !tile.terrains.corner_enclosed(peer);
            if !mode.uses(peer) || (loose && peer.corner_sides().is_some()) {
                tile.terrains = tile.terrains.with(peer, fill);
            }
        }
        let now = tile.terrains;
        let changed: Vec<Peer> = (BORDER.into_iter())
            .filter(|p| reads(mode, &now, *p) && now.get(*p) != raw.get(*p))
            .collect();
        if changed
            .iter()
            .any(|p| (now.get(*p) == 0) != (raw.get(*p) == 0))
        {
            continue;
        }
        for p in changed {
            tile.corrected[p.index()] = true;
        }
        tiles.push(tile);
    }
    tiles.sort_by_key(|t| t.address);

    let confident: Vec<&TileTerrains> = (tiles.iter())
        .filter(|t| !low_confidence(mode, t))
        .map(|t| &t.terrains)
        .collect();
    let read = |t: &TileTerrains, p: Peer| t.get(p) == other(t) && reads(mode, t, p);
    let shapes: BTreeSet<[TerrainId; 9]> = (confident.iter())
        .filter(|t| BORDER.iter().any(|p| read(t, *p)))
        .map(|t| t.peers)
        .collect();
    let closes = |peers: [Peer; 4]| (peers.iter()).all(|p| confident.iter().any(|t| read(t, *p)));
    if shapes.len() < MIN_SHAPES || !(closes(Peer::SIDES) || closes(Peer::CORNERS)) {
        return None;
    }

    let key = |t: &TileTerrains| match mode {
        MatchMode::Corners => Peer::CORNERS.map(|c| t.get(c)).to_vec(),
        MatchMode::Sides | MatchMode::CornersAndSides => t.peers.to_vec(),
    };
    let present: BTreeSet<Vec<TerrainId>> = tiles.iter().map(|t| key(&t.terrains)).collect();
    let full = match mode {
        MatchMode::Sides => LayoutTemplate::Side16,
        MatchMode::Corners => LayoutTemplate::Corner16,
        MatchMode::CornersAndSides => LayoutTemplate::Blob47,
    };
    let universe = full.tiles(inner, outer);
    let missing: Vec<TileTerrains> = (universe.iter())
        .filter(|t| !present.contains(&key(t)))
        .copied()
        .collect();
    let share = |template: LayoutTemplate| {
        let shapes = template.tiles(inner, outer);
        let hits = shapes.iter().filter(|t| present.contains(&key(t))).count();
        hits as f32 / shapes.len().max(1) as f32
    };
    let template = (LayoutTemplate::ALL.into_iter())
        .filter(|t| t.mode() == mode && share(*t) >= TEMPLATE_SHARE)
        .max_by_key(|t| t.tile_count());
    Some(ProposedSet {
        mode,
        inner,
        outer,
        tiles,
        covered: universe.len() - missing.len(),
        total: universe.len(),
        missing,
        template,
    })
}

#[cfg(test)]
mod tests {
    use glam::UVec2;

    use super::*;
    use crate::{FoundTerrain, GridLayout, RgbaImage};

    const GRASS: TerrainId = 1;
    const PATH: TerrainId = 2;
    const WOOD: TerrainId = 3;
    const ROCK: TerrainId = 4;
    /// Art only: a darker path lip.
    const LIP: u8 = 9;
    /// Art only: a path colour 0.02 from `PATH` in Oklab.
    const NEAR_PATH: u8 = 10;
    /// Art only: a path colour 0.012 from `PATH` in Oklab.
    const NEARER_PATH: u8 = 11;
    const N: u32 = 16;
    const NOISE: [[u8; 4]; 4] = [
        [80, 160, 60, 255],
        [40, 100, 40, 255],
        [150, 210, 110, 255],
        [230, 220, 90, 255],
    ];

    fn colour(art: u8) -> [u8; 4] {
        match art {
            0 => [0; 4],
            GRASS => [80, 160, 60, 255],
            PATH => [220, 170, 110, 255],
            WOOD => [150, 90, 40, 255],
            ROCK => [120, 120, 130, 255],
            LIP => [170, 100, 60, 255],
            NEAR_PATH => [232, 166, 106, 255],
            NEARER_PATH => [227, 167, 107, 255],
            _ => [255, 0, 255, 255],
        }
    }

    #[derive(Clone, Copy, Default, PartialEq)]
    enum Style {
        /// 3x3 blocks split at 2 and 14 pixels, so every peer region and
        /// edge segment shows one colour.
        #[default]
        Blocks,
        /// Blocks with a transparent outermost pixel ring.
        Inset,
        /// A 6 pixel wide path (columns or rows 5 to 10) on four-tone grass.
        Narrow,
    }

    /// One row of synthetic tiles.
    #[derive(Default)]
    struct Synth {
        art: Vec<[u8; 9]>,
        labels: Vec<TileLabels>,
        style: Style,
    }

    impl Synth {
        fn tile(mut self, art: [u8; 9], terrains: TileTerrains) -> Self {
            self.art.push(art);
            self.labels.push(TileLabels {
                kind: TileKind::Terrain,
                terrains,
                confidence: [1.0; 9],
            });
            self
        }

        fn shape(self, terrains: TileTerrains) -> Self {
            self.tile(terrains.peers, terrains)
        }

        fn shapes(self, all: impl IntoIterator<Item = TileTerrains>) -> Self {
            all.into_iter().fold(self, Synth::shape)
        }

        fn confidence(mut self, peer: Peer, value: f32) -> Self {
            if let Some(last) = self.labels.last_mut() {
                last.confidence[peer.index()] = value;
            }
            self
        }

        fn style(mut self, style: Style) -> Self {
            self.style = style;
            self
        }

        fn pixel(&self, tile: u32, x: u32, y: u32) -> [u8; 4] {
            let art = self.art[tile as usize];
            let block = |c: u32| usize::from(c >= 2) + usize::from(c >= 14);
            let band = |c: u32| usize::from(c >= 5) + usize::from(c >= 11);
            let edge = x == 0 || y == 0 || x == N - 1 || y == N - 1;
            match self.style {
                Style::Inset if edge => [0; 4],
                Style::Blocks | Style::Inset => colour(art[block(y) * 3 + block(x)]),
                Style::Narrow => {
                    let (col, row) = (band(x), band(y));
                    let arm = (col == 1 || row == 1) && art[row * 3 + col] == PATH;
                    if arm && art[Peer::Centre.index()] == PATH {
                        colour(PATH)
                    } else {
                        NOISE[((x * 73 + y * 151 + tile * 197) ^ (x * y)) as usize % 4]
                    }
                }
            }
        }

        fn pixels(&self) -> Vec<u8> {
            let mut pixels = Vec::new();
            for y in 0..N {
                for tile in 0..self.art.len() as u32 {
                    for x in 0..N {
                        pixels.extend(self.pixel(tile, x, y));
                    }
                }
            }
            pixels
        }

        fn analysis(&self) -> TerrainAnalysis {
            let max = self.labels.iter().flat_map(|l| l.terrains.peers).max();
            let terrains = (1..=max.unwrap_or(0))
                .map(|id| FoundTerrain {
                    id,
                    oklab: Vec3::ZERO,
                    swatch: [0; 3],
                    variance: 0.0,
                    pure_tiles: (0..self.labels.len() as u32)
                        .filter(|&t| self.labels[t as usize].terrains.peers == [id; 9])
                        .map(|tile| TileAddress { sheet: 0, tile })
                        .collect(),
                    name: format!("t{id}"),
                })
                .collect();
            TerrainAnalysis {
                terrains,
                tiles: vec![self.labels.iter().copied().map(Some).collect()],
            }
        }

        fn input<'a>(&self, pixels: &'a [u8]) -> SheetInput<'a> {
            let width = N * self.art.len() as u32;
            SheetInput {
                name: "synthetic",
                image: RgbaImage::new(width, N, pixels).expect("sized"),
                layout: GridLayout {
                    tile: UVec2::splat(N),
                    margin: UVec2::ZERO,
                    spacing: UVec2::ZERO,
                    columns: self.art.len() as u32,
                    rows: 1,
                },
            }
        }

        fn fit(&self) -> TilesetProposal {
            let pixels = self.pixels();
            fit_layouts(&self.analysis(), &[self.input(&pixels)]).expect("matching inputs")
        }
    }

    fn path(joins: &[Peer]) -> TileTerrains {
        joins.iter().fold(
            TileTerrains::filled(GRASS).with(Peer::Centre, PATH),
            |t, side| t.with(*side, PATH),
        )
    }

    /// Isolated, vertical top, middle, bottom, horizontal left, middle, right.
    fn strips() -> Vec<TileTerrains> {
        use Peer::{Bottom, Left, Right, Top};
        let joins: [&[Peer]; 7] = [
            &[],
            &[Bottom],
            &[Top, Bottom],
            &[Top],
            &[Right],
            &[Left, Right],
            &[Left],
        ];
        joins.into_iter().map(path).collect()
    }

    /// Corner16 art. A side between an outer and an inner corner is drawn
    /// as `mixed`.
    fn corner_art(mixed: TerrainId) -> Vec<TileTerrains> {
        let shape = move |mask: usize| {
            let outer = |c: usize| mask & (1 << c) != 0;
            let mut t = TileTerrains::filled(PATH);
            for (bit, corner) in Peer::CORNERS.into_iter().enumerate() {
                if outer(bit) {
                    t = t.with(corner, GRASS);
                }
            }
            for side in Peer::SIDES {
                let ends = Peer::CORNERS
                    .into_iter()
                    .enumerate()
                    .filter(|(_, c)| c.corner_sides().is_some_and(|s| s.contains(&side)));
                let outer_ends = ends.filter(|(bit, _)| outer(*bit)).count();
                t = t.with(side, [PATH, mixed, GRASS][outer_ends]);
            }
            t
        };
        (0..16).map(shape).collect()
    }

    fn only_set(proposal: &TilesetProposal) -> &ProposedSet {
        assert_eq!(proposal.sets.len(), 1, "{proposal:#?}");
        &proposal.sets[0]
    }

    fn fitted(set: &ProposedSet, tile: u32) -> &FittedTile {
        set.tiles
            .iter()
            .find(|t| t.address.tile == tile)
            .unwrap_or_else(|| panic!("tile {tile} fitted"))
    }

    #[test]
    fn a_lip_drawn_on_an_unjoined_side_is_corrected_to_outer() {
        let middle = path(&[Peer::Left, Peer::Right]);
        let mut art = middle.peers;
        art[Peer::Bottom.index()] = LIP;
        for label in [PATH, WOOD] {
            let synth = Synth::default()
                .shapes(strips())
                .tile(art, middle.with(Peer::Bottom, label));
            let proposal = synth.fit();
            let set = only_set(&proposal);
            let lip = fitted(set, 7);
            assert_eq!(lip.terrains, middle, "label {label}");
            assert!(lip.corrected[Peer::Bottom.index()]);
            assert!(!lip.corrected[Peer::Left.index()]);
            assert_eq!(lip.confidence[Peer::Bottom.index()], 1.0);
        }
    }

    #[test]
    fn a_low_confidence_outer_side_with_an_exact_seam_joins() {
        let middle = path(&[Peer::Left, Peer::Right]);
        let synth = Synth::default()
            .shapes(strips())
            .tile(middle.peers, middle.with(Peer::Right, GRASS))
            .confidence(Peer::Right, 0.3);
        let set = only_set(&synth.fit()).clone();
        let tile = fitted(&set, 7);
        assert_eq!(tile.terrains, middle);
        assert!(tile.corrected[Peer::Right.index()]);
    }

    #[test]
    fn a_seam_mismatch_lowers_confidence() {
        let middle = path(&[Peer::Left, Peer::Right]);
        let mut art = middle.peers;
        art[Peer::Right.index()] = NEAR_PATH;
        let synth = Synth::default().shapes(strips()).tile(art, middle);
        let proposal = synth.fit();
        let set = only_set(&proposal);
        let tile = fitted(set, 7);
        assert_eq!(tile.terrains, middle);
        assert!(tile.confidence[Peer::Right.index()] < 0.5);
        assert_eq!(tile.confidence[Peer::Left.index()], 1.0);
        assert!(proposal.low_confidence.contains(&tile.address));
        assert!(!proposal.low_confidence.contains(&fitted(set, 5).address));
    }

    #[test]
    fn one_wide_strips_fit_sides() {
        let proposal = Synth::default().shapes(strips()).fit();
        let set = only_set(&proposal);
        assert_eq!(
            (set.mode, set.inner, set.outer),
            (MatchMode::Sides, PATH, GRASS)
        );
        assert_eq!((set.covered, set.total, set.missing.len()), (7, 16, 9));
        assert_eq!(set.template, None);
        for (tile, shape) in set.tiles.iter().zip(strips()) {
            assert_eq!(tile.terrains, shape);
        }
    }

    #[test]
    fn full_layouts_fit_their_mode() {
        let cases = [
            (
                corner_art(GRASS),
                MatchMode::Corners,
                LayoutTemplate::Corner16,
            ),
            (
                corner_art(PATH),
                MatchMode::Corners,
                LayoutTemplate::Corner16,
            ),
            (
                LayoutTemplate::Side16.tiles(PATH, GRASS),
                MatchMode::Sides,
                LayoutTemplate::Side16,
            ),
            (
                LayoutTemplate::Blob47.tiles(PATH, GRASS),
                MatchMode::CornersAndSides,
                LayoutTemplate::Blob47,
            ),
        ];
        for (shapes, mode, template) in cases {
            let proposal = Synth::default().shapes(shapes).fit();
            let set = only_set(&proposal);
            let count = template.tile_count();
            assert_eq!(set.mode, mode, "{template:?}");
            assert_eq!((set.covered, set.total), (count, count), "{template:?}");
            assert_eq!(set.template, Some(template));
            assert!(set.missing.is_empty());
            assert!(set.tiles.iter().all(|t| !t.corrected.contains(&true)));
        }
    }

    #[test]
    fn near_complete_coverage_names_the_template_and_the_gap() {
        let mut shapes = LayoutTemplate::Side16.tiles(PATH, GRASS);
        let gone = shapes.remove(6);
        let proposal = Synth::default().shapes(shapes).fit();
        let set = only_set(&proposal);
        assert_eq!((set.covered, set.total), (15, 16));
        assert_eq!(set.missing, vec![gone]);
        assert_eq!(set.template, Some(LayoutTemplate::Side16));
    }

    #[test]
    fn background_stray_and_unassigned_terrains() {
        let wood = TileTerrains::filled(ROCK).with(Peer::Centre, WOOD);
        let mut torn = path(&[Peer::Right]);
        torn.peers[Peer::TopLeft.index()] = 0;
        let mut holed = path(&[Peer::Left, Peer::Right]);
        holed.peers[Peer::Bottom.index()] = 0;
        let synth = Synth::default()
            .shapes(strips())
            .shape(TileTerrains::filled(ROCK))
            .shape(wood)
            .shape(wood.with(Peer::Right, WOOD))
            .shape(torn)
            .shape(holed);
        let proposal = synth.fit();
        let set = only_set(&proposal);
        assert_eq!((set.inner, set.outer), (PATH, GRASS));
        let terrains = |proposal: &TilesetProposal, background: bool| {
            proposal
                .terrains
                .iter()
                .filter(|t| t.background == background)
                .map(|t| t.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(terrains(&proposal, true), [GRASS]);
        assert_eq!(terrains(&proposal, false), [PATH, ROCK]);
        // The torn tile's transparent corner is not read in `Sides`; the
        // holed tile's bottom has a partner whose seam turns it opaque.
        assert_eq!(fitted(set, 10).terrains, path(&[Peer::Right]));
        let unassigned: Vec<u32> = proposal.unassigned.iter().map(|a| a.tile).collect();
        assert_eq!(unassigned, [8, 9, 11]);

        let with_grass = synth.shape(TileTerrains::filled(GRASS)).fit();
        assert_eq!(terrains(&with_grass, true), []);
        assert!(with_grass.terrains.iter().any(|t| t.id == GRASS));
    }

    #[test]
    fn a_set_converts_to_a_solver_set() {
        let proposal = Synth::default().shapes(strips()).fit();
        let set = only_set(&proposal).terrain_set(|a| u16::try_from(a.tile).ok().map(TileRef::new));
        assert_eq!(set.mode, MatchMode::Sides);
        assert_eq!(set.order, [GRASS, PATH]);
        let shapes: Vec<TileTerrains> = set.tiles.iter().map(|t| t.terrains).collect();
        assert_eq!(shapes, strips());
        assert_eq!(set.tiles[3].tile, TileRef::new(3));
    }

    #[test]
    fn a_side_without_a_partner_keeps_its_label_at_no_confidence() {
        use Peer::{Left, Right, Top};
        let shapes = [
            path(&[]),
            path(&[Right]),
            path(&[Left, Right]),
            path(&[Left]),
        ];
        let proposal = Synth::default().shapes(shapes).shape(path(&[Top])).fit();
        let set = only_set(&proposal);
        let tile = fitted(set, 4);
        assert_eq!(tile.terrains, path(&[Top]));
        assert_eq!(tile.confidence[Top.index()], 0.0);
        assert!(!tile.corrected[Top.index()]);
        assert!(proposal.low_confidence.contains(&tile.address));
    }

    #[test]
    fn a_near_threshold_join_of_an_outer_label_is_low_confidence() {
        let middle = path(&[Peer::Left, Peer::Right]);
        let mut art = middle.peers;
        art[Peer::Right.index()] = NEARER_PATH;
        let synth = Synth::default()
            .shapes(strips())
            .tile(art, middle.with(Peer::Right, GRASS))
            .confidence(Peer::Right, 0.3);
        let proposal = synth.fit();
        let tile = fitted(only_set(&proposal), 7);
        assert_eq!(tile.terrains, middle);
        assert!(tile.corrected[Peer::Right.index()]);
        assert!(tile.confidence[Peer::Right.index()] < 0.5);
    }

    #[test]
    fn a_narrow_path_joins_across_noisy_grass() {
        let synth = Synth::default().shapes(strips()).style(Style::Narrow);
        let proposal = synth.fit();
        let set = only_set(&proposal);
        for (tile, shape) in set.tiles.iter().zip(strips()) {
            assert_eq!(tile.terrains, shape, "tile {}", tile.address.tile);
            assert!(!tile.corrected.contains(&true));
        }
    }

    #[test]
    fn an_inset_overlay_gives_no_seam_evidence_and_founds_no_set() {
        let proposal = Synth::default().shapes(strips()).style(Style::Inset).fit();
        assert!(proposal.sets.is_empty());
        assert_eq!(proposal.unassigned.len(), 7);
    }

    #[test]
    fn tiny_tiles_have_segments_inside_their_edges() {
        for n in 1..=4u32 {
            for peer in BORDER {
                for side in Peer::SIDES.into_iter().filter(|s| touches(peer, *s)) {
                    let range = segment(peer, side, n, n);
                    assert!(range.end <= n as usize, "{n} {peer:?} {side:?}");
                }
            }
        }
        assert_eq!(seam(&[], &[], 0..0), None);
        let one = [Some(Vec3::ONE)];
        assert_eq!(seam(&one, &one, 0..1), Some(0.0));
        assert_eq!(seam(&[None], &[None], 0..1), None);
    }

    #[test]
    fn mismatched_inputs_are_an_error() {
        let synth = Synth::default().shapes(strips());
        let pixels = synth.pixels();
        let input = synth.input(&pixels);
        let analysis = synth.analysis();
        assert_eq!(
            fit_layouts(&analysis, &[input, input]),
            Err(FitError::SheetCount {
                inputs: 2,
                analysed: 1
            })
        );
        let mut short = input;
        short.layout.columns -= 1;
        assert_eq!(
            fit_layouts(&analysis, &[short]),
            Err(FitError::TileCount {
                sheet: 0,
                inputs: 6,
                analysed: 7
            })
        );
    }

    #[test]
    fn a_corner_set_split_by_centre_is_one_set() {
        let synth = Synth::default().shapes(corner_art(GRASS).into_iter().map(|t| {
            let grass = Peer::CORNERS.iter().filter(|c| t.get(**c) == GRASS).count();
            if grass >= 3 {
                t.with(Peer::Centre, GRASS)
            } else {
                t
            }
        }));
        let proposal = synth.fit();
        let set = only_set(&proposal);
        assert_eq!(set.mode, MatchMode::Corners);
        assert_eq!((set.inner, set.outer), (PATH, GRASS));
        assert_eq!(set.tiles.len(), 16);
        assert_eq!((set.covered, set.total), (16, 16));
        assert_eq!(set.template, Some(LayoutTemplate::Corner16));
        let centres = set
            .tiles
            .iter()
            .filter(|t| t.terrains.centre() == GRASS)
            .count();
        assert_eq!(centres, 5);
    }
}
