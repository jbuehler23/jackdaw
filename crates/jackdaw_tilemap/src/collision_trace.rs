//! Collision tracing: polygons from the pixels of Solid terrains.
//!
//! - Mask: a pixel at least [`MIN_ALPHA`] opaque is solid when its nearest
//!   terrain colour (Oklab) among the tile's own peer terrains is Solid; in a
//!   tile marked solid decor, every such pixel is.
//! - Trace: marching squares on the pixel-corner lattice, following pixel
//!   edges with the solid side on the left and turning left at a saddle, so
//!   pixels touching only at a corner trace apart. Holes are filled: only
//!   outer boundaries are kept, less those inside another one.
//! - Simplify: Ramer-Douglas-Peucker within [`SIMPLIFY_STEPS`] snap steps
//!   between the vertices on the ring's bounding box, keeping every vertex
//!   tied for farthest, so mirrored rings simplify alike.
//! - Snap: to an eighth of the tile on an axis whose size is a multiple of
//!   [`SNAP_DIVISIONS`], else to a whole pixel, halves to even; repeated and
//!   collinear vertices are removed. A ring left without area or crossing
//!   itself falls back to its snapped unsimplified ring, then to its pixels
//!   traced at snap resolution (a cell solid when any of its pixels is), then
//!   to its bounding box grown outward to whole snap steps, so no solid pixel
//!   loses its collider.
//! - Output: counter-clockwise polygons, possibly concave, in tile fractions,
//!   y up, origin at the tile's bottom-left. A fully solid tile is the unit
//!   square.
//! - Half tile (a heuristic of our own: a wall-to-floor tile is half solid
//!   whatever the depth its face is drawn at): in a terrain tile whose read
//!   peers (fitted ones in its set's mode, else all its labels) are all
//!   drawn, a quadrant is Solid when its corner peer is, or in a sides set
//!   when the centre and both sides next to it are. With some but not all
//!   quadrants Solid, the solid pixels are compared with bands of one depth
//!   along the sides whose two quadrants are Solid plus squares of that
//!   depth in the other Solid quadrants. If the best depth differs in at
//!   most [`STRIP_MISMATCH`] of the pixels (a blob merely touching a side
//!   does not), the tile becomes exactly its Solid quadrants, so a wall's
//!   straight and corner tiles meet at half depth.

use std::collections::{BTreeMap, BTreeSet};

use glam::{IVec2, UVec2, Vec2, Vec3};

use crate::layout_fit::check_inputs;
use crate::terrain_find::{MIN_ALPHA, srgb_to_oklab};
use crate::{
    FitError, MatchMode, Peer, SheetInput, TerrainAnalysis, TerrainId, TileAddress, TileKind,
    TileLabels, TileTerrains, TilesetProposal,
};

/// Snap grid divisions per tile side.
const SNAP_DIVISIONS: u32 = 8;
/// Simplification tolerance in snap steps.
const SIMPLIFY_STEPS: f32 = 0.75;
/// Share of the tile's pixels by which the solid pixels may differ from the
/// bands and squares.
const STRIP_MISMATCH: f32 = 0.3;
/// Distance within which simplification treats vertices as tied.
const TIE: f32 = 1e-4;

/// Per-pixel solidity of one tile, rows bottom to top.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SolidMask {
    size: UVec2,
    solid: Vec<bool>,
}

impl SolidMask {
    /// `None` unless `solid` holds `size.x * size.y` pixels, rows bottom to top.
    pub fn new(size: UVec2, solid: Vec<bool>) -> Option<Self> {
        let len = (size.x as usize).checked_mul(size.y as usize)?;
        (solid.len() == len).then_some(Self { size, solid })
    }

    pub fn size(&self) -> UVec2 {
        self.size
    }

    /// Whether pixel `(x, y)`, y up, is solid; false outside the tile.
    pub fn get(&self, x: i32, y: i32) -> bool {
        let inside = x >= 0 && y >= 0 && (x as u32) < self.size.x && (y as u32) < self.size.y;
        inside
            && (self.solid)
                .get(y as usize * self.size.x as usize + x as usize)
                .copied()
                .unwrap_or(false)
    }

    pub fn count(&self) -> usize {
        self.solid.iter().filter(|s| **s).count()
    }
}

/// Collision polygons of one tile.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileShape {
    /// Counter-clockwise, non-overlapping outer rings in tile fractions, y
    /// up, origin at the tile's bottom-left.
    pub polygons: Vec<Vec<Vec2>>,
    /// The half-tile rule made the shape rather than the trace.
    pub half_tile: bool,
}

impl TileShape {
    /// Area as a fraction of the tile.
    pub fn area(&self) -> f32 {
        self.polygons.iter().map(|p| signed_area(p)).sum()
    }

    pub fn vertex_count(&self) -> usize {
        self.polygons.iter().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.polygons.is_empty()
    }
}

/// The solid pixels of the tile at `address`, classified among `peers`
/// (a fitted tile's terrains) or, when `None`, its labelled peers. With
/// `decor_solid` every opaque pixel is solid, whatever the tile's kind; a
/// decor tile, or a terrain tile with no Solid peer, is otherwise all clear,
/// returned without reading its pixels. `Ok(None)` for an empty or unknown
/// tile; an error when `inputs` do not match `analysis`.
pub fn solid_mask(
    analysis: &TerrainAnalysis,
    inputs: &[SheetInput],
    address: TileAddress,
    peers: Option<&TileTerrains>,
    solid: impl Fn(TerrainId) -> bool,
    decor_solid: bool,
) -> Result<Option<SolidMask>, FitError> {
    check_inputs(analysis, inputs)?;
    let Some(labels) = analysis.tile(address) else {
        return Ok(None);
    };
    let peers = peers.unwrap_or(&labels.terrains);
    Ok(mask_of(
        analysis,
        inputs,
        address,
        labels,
        peers,
        &solid,
        decor_solid,
    ))
}

/// Traces, simplifies and snaps the outer boundaries of `mask`.
pub fn trace_mask(mask: &SolidMask) -> TileShape {
    let step = snap_step(mask.size);
    let size = mask.size.as_vec2().max(Vec2::ONE);
    let polygons = (outlines(mask).into_iter())
        .map(|ring| {
            let simplified = simplify(&ring, SIMPLIFY_STEPS * step.min_element());
            (snap(&simplified, step).or_else(|| snap(&ring, step)))
                .or_else(|| coarse(&ring, mask, step))
                .unwrap_or_else(|| cover(&ring, step))
        })
        .map(|ring| ring.into_iter().map(|v| v / size).collect())
        .collect();
    TileShape {
        polygons,
        half_tile: false,
    }
}

/// Traces every analysed tile of `inputs`: terrain tiles through their
/// fitted peers and their set's mode where a set holds them, else through
/// their labels with every peer read; decor tiles only when listed in
/// `solid_decor`. Tiles without a shape are left out.
pub fn trace_proposal(
    analysis: &TerrainAnalysis,
    inputs: &[SheetInput],
    proposal: &TilesetProposal,
    solid: impl Fn(TerrainId) -> bool,
    solid_decor: &[TileAddress],
) -> Result<BTreeMap<TileAddress, TileShape>, FitError> {
    check_inputs(analysis, inputs)?;
    let solid_decor: BTreeSet<&TileAddress> = solid_decor.iter().collect();
    let mut fitted = BTreeMap::new();
    for set in &proposal.sets {
        for tile in &set.tiles {
            let read = (&tile.terrains, set.mode);
            fitted.entry(tile.address).or_insert(read);
        }
    }
    let mut shapes = BTreeMap::new();
    for (sheet, tiles) in analysis.tiles.iter().enumerate() {
        for (index, labels) in tiles.iter().enumerate() {
            let Some(labels) = labels else {
                continue;
            };
            let address = TileAddress {
                sheet,
                tile: index as u32,
            };
            let raw = (&labels.terrains, MatchMode::CornersAndSides);
            let (peers, mode) = fitted.get(&address).copied().unwrap_or(raw);
            let decor = solid_decor.contains(&address);
            let terrain = labels.kind == TileKind::Terrain && !decor;
            if !decor && !(terrain && peers.peers.iter().any(|id| *id != 0 && solid(*id))) {
                continue;
            }
            let Some(mask) = mask_of(analysis, inputs, address, labels, peers, &solid, decor)
            else {
                continue;
            };
            let quadrants = terrain
                .then(|| half_quadrants(&mask, peers, mode, &solid))
                .flatten();
            let shape = quadrants.map_or_else(|| trace_mask(&mask), quadrant_shape);
            if !shape.is_empty() {
                shapes.insert(address, shape);
            }
        }
    }
    Ok(shapes)
}

fn mask_of(
    analysis: &TerrainAnalysis,
    inputs: &[SheetInput],
    address: TileAddress,
    labels: &TileLabels,
    peers: &TileTerrains,
    solid: &dyn Fn(TerrainId) -> bool,
    decor_solid: bool,
) -> Option<SolidMask> {
    let input = inputs.get(address.sheet)?;
    if address.tile >= input.layout.tile_count() {
        return None;
    }
    let (origin, size) = input.layout.tile_rect(address.tile);
    let mut ids: Vec<TerrainId> = peers.peers.iter().copied().filter(|id| *id != 0).collect();
    ids.sort_unstable();
    ids.dedup();
    let colours: Vec<(Vec3, bool)> = (ids.iter())
        .filter_map(|id| Some((analysis.terrain(*id)?.oklab, solid(*id))))
        .collect();
    let terrain = labels.kind == TileKind::Terrain;
    if !decor_solid && !(terrain && colours.iter().any(|(_, s)| *s)) {
        return SolidMask::new(size, vec![false; size.x as usize * size.y as usize]);
    }
    let mut mask = Vec::with_capacity(size.x as usize * size.y as usize);
    for y in (0..size.y).rev() {
        for x in 0..size.x {
            let p = input.image.pixel(origin.x + x, origin.y + y)?;
            let opaque = f32::from(p[3]) / 255.0 >= MIN_ALPHA;
            mask.push(opaque && (decor_solid || nearest_is_solid(&colours, p)));
        }
    }
    SolidMask::new(size, mask)
}

fn nearest_is_solid(colours: &[(Vec3, bool)], pixel: [u8; 4]) -> bool {
    let oklab = srgb_to_oklab(pixel);
    let mut best = (f32::INFINITY, false);
    for (colour, solid) in colours {
        let d = colour.distance_squared(oklab);
        if d < best.0 {
            best = (d, *solid);
        }
    }
    best.1
}

fn snap_step(size: UVec2) -> Vec2 {
    let axis = |n: u32| {
        if n > 0 && n.is_multiple_of(SNAP_DIVISIONS) {
            (n / SNAP_DIVISIONS) as f32
        } else {
            1.0
        }
    };
    Vec2::new(axis(size.x), axis(size.y))
}

/// Unit steps east, north, west and south; a left turn is the next index.
const DIRECTIONS: [IVec2; 4] = [IVec2::X, IVec2::Y, IVec2::NEG_X, IVec2::NEG_Y];

/// Outer pixel-edge boundaries with the solid side on the left, as corner
/// vertices in pixels. Clockwise rings (holes) and rings inside another
/// outer ring (islands in a filled hole) are dropped.
fn outlines(mask: &SolidMask) -> Vec<Vec<Vec2>> {
    let (w, h) = (mask.size.x as i32, mask.size.y as i32);
    let stride = w + 1;
    let at = |v: IVec2| (v.y * stride + v.x) as usize;
    let mut edges = vec![0u8; at(IVec2::new(w, h)) + 1];
    let mut add = |v: IVec2, d: usize| {
        if let Some(bits) = edges.get_mut(at(v)) {
            *bits |= 1 << d;
        }
    };
    for y in 0..h {
        for x in 0..w {
            if !mask.get(x, y) {
                continue;
            }
            for (d, corner) in [(0, 0), (1, 0), (1, 1), (0, 1)].into_iter().enumerate() {
                let open = IVec2::new(x, y) + DIRECTIONS[(d + 3) % 4];
                if !mask.get(open.x, open.y) {
                    add(IVec2::new(x + corner.0, y + corner.1), d);
                }
            }
        }
    }
    let mut unused = edges.clone();
    let limit = 4 * edges.len();
    let mut rings = Vec::new();
    for start in 0..unused.len() {
        for first in 0..4 {
            if unused[start] & (1 << first) == 0 {
                continue;
            }
            let origin = IVec2::new(start as i32 % stride, start as i32 / stride);
            let (mut v, mut d) = (origin, first);
            let mut steps = Vec::new();
            loop {
                if let Some(bits) = unused.get_mut(at(v)) {
                    *bits &= !(1 << d);
                }
                steps.push((v, d));
                v += DIRECTIONS[d];
                let bits = edges.get(at(v)).copied().unwrap_or(0);
                let next = [(d + 1) % 4, d, (d + 3) % 4];
                match next.into_iter().find(|n| bits & (1 << n) != 0) {
                    Some(n) if (v, n) != (origin, first) && steps.len() < limit => d = n,
                    _ => break,
                }
            }
            let n = steps.len();
            let ring: Vec<Vec2> = (0..n)
                .filter(|i| steps[*i].1 != steps[(i + n - 1) % n].1)
                .map(|i| steps[i].0.as_vec2())
                .collect();
            let inward = (DIRECTIONS[first] + DIRECTIONS[(first + 1) % 4]).as_vec2();
            let pixel = origin.as_vec2() + 0.5 * inward;
            if signed_area(&ring) > 0.0 {
                rings.push((ring, pixel));
            }
        }
    }
    let island = |i: usize, pixel: Vec2| {
        (rings.iter().enumerate()).any(|(j, (ring, _))| j != i && contains(ring, pixel))
    };
    (rings.iter().enumerate())
        .filter(|(i, (_, pixel))| !island(*i, *pixel))
        .map(|(_, (ring, _))| ring.clone())
        .collect()
}

/// Even-odd test for a point off every edge.
fn contains(ring: &[Vec2], p: Vec2) -> bool {
    let n = ring.len();
    let mut inside = false;
    for i in 0..n {
        let (a, b) = (ring[i], ring[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            inside = !inside;
        }
    }
    inside
}

/// Ramer-Douglas-Peucker over a closed ring between its vertices on its
/// bounding box, keeping every vertex tied for the largest distance.
fn simplify(ring: &[Vec2], epsilon: f32) -> Vec<Vec2> {
    let n = ring.len();
    let (min, max) = bounds(ring);
    let on_box = |v: Vec2| v.x == min.x || v.y == min.y || v.x == max.x || v.y == max.y;
    let mut keep: Vec<bool> = ring.iter().map(|v| on_box(*v)).collect();
    let anchors: Vec<usize> = (0..n).filter(|i| keep[*i]).collect();
    let mut spans: Vec<(usize, usize)> = (0..anchors.len())
        .map(|k| {
            (
                anchors[k],
                anchors.get(k + 1).copied().unwrap_or(anchors[0] + n),
            )
        })
        .collect();
    while let Some((a, b)) = spans.pop() {
        let (pa, pb) = (ring[a % n], ring[b % n]);
        let distance = |i: usize| segment_distance(ring[i % n], pa, pb);
        let worst = (a + 1..b).map(distance).fold(0.0, f32::max);
        if worst <= epsilon {
            continue;
        }
        let mut from = a;
        for i in (a + 1..b).filter(|i| distance(*i) >= worst - TIE) {
            keep[i % n] = true;
            spans.push((from, i));
            from = i;
        }
        spans.push((from, b));
    }
    (0..n).filter(|i| keep[*i]).map(|i| ring[i]).collect()
}

fn bounds(ring: &[Vec2]) -> (Vec2, Vec2) {
    let min = ring.iter().fold(Vec2::INFINITY, |m, v| m.min(*v));
    let max = ring.iter().fold(Vec2::NEG_INFINITY, |m, v| m.max(*v));
    (min, max)
}

fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 {
        ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + t * ab)
}

/// Snaps to `step`, halves to even, and removes repeated and collinear
/// vertices; `None` for a ring left without area or crossing itself.
fn snap(ring: &[Vec2], step: Vec2) -> Option<Vec<Vec2>> {
    let round = |v: Vec2| Vec2::new(v.x.round_ties_even(), v.y.round_ties_even());
    let mut ring: Vec<Vec2> = ring.iter().map(|v| round(*v / step) * step).collect();
    loop {
        let n = ring.len();
        if n < 3 {
            return None;
        }
        let straight = |i: usize| {
            let (a, b, c) = (ring[(i + n - 1) % n], ring[i], ring[(i + 1) % n]);
            (b - a).perp_dot(c - b) == 0.0
        };
        let Some(i) = (0..n).find(|i| straight(*i)) else {
            break;
        };
        ring.remove(i);
    }
    (signed_area(&ring) > 0.0 && simple(&ring)).then_some(ring)
}

/// The ring traced at snap resolution, a cell solid when a solid pixel
/// inside the ring falls in it; `None` unless that is one simple ring.
fn coarse(ring: &[Vec2], mask: &SolidMask, step: Vec2) -> Option<Vec<Vec2>> {
    let cell = step.as_uvec2().max(UVec2::ONE);
    let size = (mask.size + cell - UVec2::ONE) / cell;
    let cells = (0..size.y).flat_map(|cy| (0..size.x).map(move |cx| UVec2::new(cx, cy)));
    let solid = cells.map(|c| {
        let pixels =
            (0..cell.y).flat_map(|y| (0..cell.x).map(move |x| c * cell + UVec2::new(x, y)));
        pixels
            .map(UVec2::as_ivec2)
            .any(|p| mask.get(p.x, p.y) && contains(ring, p.as_vec2() + 0.5))
    });
    let rings = outlines(&SolidMask::new(size, solid.collect())?);
    let [ring] = rings.as_slice() else {
        return None;
    };
    simple(ring).then(|| ring.iter().map(|v| *v * step).collect())
}

/// The ring's bounding box grown outward to whole snap steps.
fn cover(ring: &[Vec2], step: Vec2) -> Vec<Vec2> {
    let (min, max) = bounds(ring);
    let (min, max) = ((min / step).floor() * step, (max / step).ceil() * step);
    vec![min, Vec2::new(max.x, min.y), max, Vec2::new(min.x, max.y)]
}

/// No two edges meet other than neighbours at their shared vertex.
fn simple(ring: &[Vec2]) -> bool {
    let n = ring.len();
    let edge = |i: usize| (ring[i], ring[(i + 1) % n]);
    (0..n).all(|i| {
        (i + 2..n)
            .filter(|j| !(i == 0 && *j == n - 1))
            .all(|j| !segments_meet(edge(i), edge(j)))
    })
}

fn segments_meet((p1, p2): (Vec2, Vec2), (q1, q2): (Vec2, Vec2)) -> bool {
    let side = |a: Vec2, b: Vec2, c: Vec2| (b - a).perp_dot(c - a);
    let within = |a: Vec2, b: Vec2, c: Vec2| c.cmpge(a.min(b)).all() && c.cmple(a.max(b)).all();
    let (d1, d2) = (side(q1, q2, p1), side(q1, q2, p2));
    let (d3, d4) = (side(p1, p2, q1), side(p1, p2, q2));
    let crosses = d1 * d2 < 0.0 && d3 * d4 < 0.0;
    crosses
        || (d1 == 0.0 && within(q1, q2, p1))
        || (d2 == 0.0 && within(q1, q2, p2))
        || (d3 == 0.0 && within(p1, p2, q1))
        || (d4 == 0.0 && within(p1, p2, q2))
}

/// Quadrants in solid-mask order: bottom row first.
const QUADRANTS: [Peer; 4] = [
    Peer::BottomLeft,
    Peer::BottomRight,
    Peer::TopLeft,
    Peer::TopRight,
];

/// Solid quadrants, in [`QUADRANTS`] order, of a wall tile under the
/// half-tile rule, or `None` when it does not apply.
fn half_quadrants(
    mask: &SolidMask,
    peers: &TileTerrains,
    mode: MatchMode,
    solid: &dyn Fn(TerrainId) -> bool,
) -> Option<[bool; 4]> {
    let drawn = Peer::ALL.into_iter().filter(|p| mode.uses(*p));
    if drawn.clone().any(|p| peers.get(p) == 0) {
        return None;
    }
    let is_solid = |p: Peer| solid(peers.get(p));
    let quadrant = |c: Peer| match mode {
        MatchMode::Sides => {
            let sides = c.corner_sides().map(|s| s.into_iter().all(is_solid));
            is_solid(Peer::Centre) && sides.unwrap_or(false)
        }
        MatchMode::Corners | MatchMode::CornersAndSides => is_solid(c),
    };
    let quadrants = QUADRANTS.map(quadrant);
    if quadrants.iter().all(|q| *q) || !quadrants.iter().any(|q| *q) {
        return None;
    }
    let [bl, br, tl, tr] = quadrants;
    let (bottom, top, left, right) = (bl && br, tl && tr, bl && tl, br && tr);
    let corners = [
        bl && !bottom && !left,
        br && !bottom && !right,
        tl && !top && !left,
        tr && !top && !right,
    ];
    let (w, h) = (mask.size.x as i32, mask.size.y as i32);
    let n = w.max(h).max(1);
    let cells = (w * h).max(1) as f32;
    let fit = |k: i32| {
        let depth = k as f32 / n as f32;
        let (dx, dy) = (
            (depth * w as f32).round() as i32,
            (depth * h as f32).round() as i32,
        );
        let model = |x: i32, y: i32| {
            let (l, r, b, t) = (x < dx, x >= w - dx, y < dy, y >= h - dy);
            (bottom && b)
                || (top && t)
                || (left && l)
                || (right && r)
                || (corners[0] && b && l)
                || (corners[1] && b && r)
                || (corners[2] && t && l)
                || (corners[3] && t && r)
        };
        let points = (0..h).flat_map(|y| (0..w).map(move |x| (x, y)));
        points
            .filter(|(x, y)| model(*x, *y) != mask.get(*x, *y))
            .count()
    };
    let best = (1..n).map(fit).min()?;
    (best as f32 / cells <= STRIP_MISMATCH).then_some(quadrants)
}

fn quadrant_shape(quadrants: [bool; 4]) -> TileShape {
    let polygons = SolidMask::new(UVec2::splat(2), quadrants.to_vec())
        .map(|mask| outlines(&mask))
        .unwrap_or_default();
    TileShape {
        polygons: (polygons.into_iter())
            .map(|ring| ring.into_iter().map(|v| v / 2.0).collect())
            .collect(),
        half_tile: true,
    }
}

fn signed_area(ring: &[Vec2]) -> f32 {
    let n = ring.len();
    let twice: f32 = (0..n).map(|i| ring[i].perp_dot(ring[(i + 1) % n])).sum();
    twice / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FittedTile, GridLayout, ProposedSet, RgbaImage};

    /// A mask from rows top to bottom, `#` solid.
    fn mask(rows: &[&str]) -> SolidMask {
        let size = UVec2::new(rows[0].len() as u32, rows.len() as u32);
        let solid = rows.iter().rev().flat_map(|r| r.chars().map(|c| c == '#'));
        SolidMask::new(size, solid.collect()).expect("rectangular")
    }

    fn mask_fn(size: u32, solid: impl Fn(u32, u32) -> bool) -> SolidMask {
        let cells = (0..size).flat_map(|y| (0..size).map(move |x| (x, y)));
        let solid = cells.map(|(x, y)| solid(x, y)).collect();
        SolidMask::new(UVec2::splat(size), solid).expect("square")
    }

    fn ring(points: &[(f32, f32)]) -> Vec<Vec2> {
        points.iter().map(|&(x, y)| Vec2::new(x, y)).collect()
    }

    /// The ring rotated to start at its lowest-then-leftmost vertex.
    fn canonical(ring: &[Vec2]) -> Vec<Vec2> {
        let start = (0..ring.len())
            .min_by(|&a, &b| {
                let (a, b) = (ring[a], ring[b]);
                a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x))
            })
            .unwrap_or(0);
        ring[start..]
            .iter()
            .chain(&ring[..start])
            .copied()
            .collect()
    }

    fn snapped(shape: &TileShape, step: f32) -> bool {
        let on = |v: f32| ((v / step).round() * step - v).abs() < 1e-5;
        shape.polygons.iter().flatten().all(|v| on(v.x) && on(v.y))
    }

    #[test]
    fn full_tile_is_the_unit_square() {
        let shape = trace_mask(&mask_fn(16, |_, _| true));
        assert_eq!(shape.polygons.len(), 1);
        let square = ring(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)]);
        assert_eq!(canonical(&shape.polygons[0]), square);
        assert_eq!(shape.area(), 1.0);
    }

    #[test]
    fn empty_tile_has_no_shape() {
        assert!(trace_mask(&mask_fn(16, |_, _| false)).is_empty());
    }

    #[test]
    fn bottom_half_is_one_rectangle() {
        let shape = trace_mask(&mask_fn(16, |_, y| y < 8));
        assert_eq!(shape.polygons.len(), 1);
        let half = ring(&[(0.0, 0.0), (1.0, 0.0), (1.0, 0.5), (0.0, 0.5)]);
        assert_eq!(canonical(&shape.polygons[0]), half);
    }

    #[test]
    fn pixel_slope_becomes_a_snapped_triangle() {
        let shape = trace_mask(&mask_fn(16, |x, y| x >= y));
        assert_eq!(shape.polygons.len(), 1);
        let triangle = ring(&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0)]);
        assert_eq!(canonical(&shape.polygons[0]), triangle);
        assert!(snapped(&shape, 0.125));
    }

    #[test]
    fn l_shape_is_one_concave_hexagon() {
        let shape = trace_mask(&mask_fn(16, |x, y| x < 8 || y < 8));
        assert_eq!(shape.polygons.len(), 1);
        assert_eq!(shape.vertex_count(), 6);
        assert_eq!(shape.area(), 0.75);
    }

    #[test]
    fn separated_blobs_are_two_polygons() {
        let shape = trace_mask(&mask_fn(16, |x, y| {
            (4..12).contains(&y) && !(4..10).contains(&x)
        }));
        assert_eq!(shape.polygons.len(), 2);
        assert_eq!(shape.vertex_count(), 8);
        assert_eq!(shape.area(), (4.0 * 8.0 + 6.0 * 8.0) / 256.0);
    }

    #[test]
    fn corner_touching_pixels_trace_apart() {
        let shape = trace_mask(&mask_fn(16, |x, y| (x < 8) == (y < 8)));
        assert_eq!(shape.polygons.len(), 2);
        assert_eq!(shape.area(), 0.5);
    }

    #[test]
    fn a_ring_traces_its_outer_boundary_with_the_hole_filled() {
        let inside = |v: u32| (2..14).contains(&v);
        let hole = |v: u32| (6..10).contains(&v);
        let shape = trace_mask(&mask_fn(16, |x, y| {
            inside(x) && inside(y) && !(hole(x) && hole(y))
        }));
        assert_eq!(shape.polygons.len(), 1);
        assert_eq!(shape.vertex_count(), 4);
        assert_eq!(shape.area(), 0.75 * 0.75);
    }

    #[test]
    fn non_multiple_of_eight_snaps_to_whole_pixels() {
        let shape = trace_mask(&mask_fn(12, |x, y| x >= 1 && y < 5));
        assert_eq!(shape.polygons.len(), 1);
        let rect = ring(&[(1.0, 0.0), (12.0, 0.0), (12.0, 5.0), (1.0, 5.0)]);
        let rect: Vec<Vec2> = rect.into_iter().map(|v| v / 12.0).collect();
        assert_eq!(canonical(&shape.polygons[0]), rect);
    }

    #[test]
    fn eighths_drop_single_pixel_bumps() {
        let shape = trace_mask(&mask_fn(16, |x, y| y < 8 || (x == 5 && y == 8)));
        assert_eq!(shape.vertex_count(), 4);
    }

    fn rect(min: (f32, f32), max: (f32, f32)) -> Vec<Vec2> {
        ring(&[min, (max.0, min.1), max, (min.0, max.1)])
    }

    fn eighths(ring: Vec<Vec2>) -> Vec<Vec2> {
        ring.into_iter().map(|v| v / 8.0).collect()
    }

    #[test]
    fn small_rings_keep_a_snapped_collider() {
        let on_grid = trace_mask(&mask_fn(16, |x, y| {
            (4..6).contains(&x) && (4..6).contains(&y)
        }));
        assert_eq!(on_grid.polygons, [eighths(rect((2.0, 2.0), (3.0, 3.0)))]);
        // Off the grid, a block grows outward to whole snap cells.
        let off_grid = |x: u32, y: u32| (3..5).contains(&x) && (3..5).contains(&y);
        let block = trace_mask(&mask_fn(16, off_grid));
        assert_eq!(block.polygons, [eighths(rect((1.0, 1.0), (3.0, 3.0)))]);
        let speck = trace_mask(&mask_fn(16, |x, y| x == 3 && y == 3));
        assert_eq!(speck.polygons, [eighths(rect((1.0, 1.0), (2.0, 2.0)))]);
        let post = trace_mask(&mask_fn(16, |x, y| x == 7 && (3..13).contains(&y)));
        assert_eq!(post.polygons, [eighths(rect((3.0, 1.0), (4.0, 7.0)))]);
    }

    /// `shape` mirrored left to right, each ring counter-clockwise again.
    fn mirrored(shape: &TileShape) -> Vec<Vec<Vec2>> {
        let mut rings: Vec<Vec<Vec2>> = (shape.polygons.iter())
            .map(|r| {
                canonical(
                    &r.iter()
                        .rev()
                        .map(|v| Vec2::new(1.0 - v.x, v.y))
                        .collect::<Vec<_>>(),
                )
            })
            .collect();
        rings.sort_by(|a, b| a[0].x.total_cmp(&b[0].x).then(a[0].y.total_cmp(&b[0].y)));
        rings
    }

    fn sorted(shape: &TileShape) -> Vec<Vec<Vec2>> {
        let mut rings: Vec<Vec<Vec2>> = shape.polygons.iter().map(|r| canonical(r)).collect();
        rings.sort_by(|a, b| a[0].x.total_cmp(&b[0].x).then(a[0].y.total_cmp(&b[0].y)));
        rings
    }

    fn flip(mask: &SolidMask) -> SolidMask {
        let size = mask.size();
        let cells = (0..size.y as i32).flat_map(|y| (0..size.x as i32).map(move |x| (x, y)));
        let solid = cells
            .map(|(x, y)| mask.get(size.x as i32 - 1 - x, y))
            .collect();
        SolidMask::new(size, solid).expect("same size")
    }

    #[test]
    fn one_pixel_walls_either_side_of_a_grid_line_mirror() {
        let left = trace_mask(&mask_fn(16, |x, _| x == 7));
        let right = trace_mask(&mask_fn(16, |x, _| x == 8));
        assert_eq!(left.polygons, [eighths(rect((3.0, 0.0), (4.0, 8.0)))]);
        assert_eq!(sorted(&right), mirrored(&left));
    }

    #[test]
    fn mirrored_tiles_give_mirrored_polygons() {
        for blob in [
            blob(),
            mask_fn(16, |x, y| x * 3 + y * 2 < 29 && y + 1 > x / 3),
        ] {
            let shape = trace_mask(&blob);
            assert!(!shape.is_empty());
            assert_eq!(sorted(&trace_mask(&flip(&blob))), mirrored(&shape));
        }
    }

    #[test]
    fn a_necked_blob_stays_a_simple_ring() {
        let necked = |x: u32, y: u32| {
            let lobe = (4..12).contains(&y) && !(6..10).contains(&x);
            lobe || (y == 7 && (6..10).contains(&x))
        };
        let shape = trace_mask(&mask_fn(16, necked));
        assert!(!shape.is_empty());
        for polygon in &shape.polygons {
            assert!(simple(polygon), "{polygon:?}");
        }
        assert!(shape.area() >= (2.0 * 6.0 * 8.0 + 4.0) / 256.0);
    }

    #[test]
    fn islands_in_a_filled_hole_are_dropped() {
        let inside = |v: u32| (2..14).contains(&v);
        let hole = |v: u32| (5..11).contains(&v);
        let post = |v: u32| (7..9).contains(&v);
        let well = trace_mask(&mask_fn(16, |x, y| {
            (inside(x) && inside(y) && !(hole(x) && hole(y))) || (post(x) && post(y))
        }));
        assert_eq!(well.polygons, [eighths(rect((1.0, 1.0), (7.0, 7.0)))]);
        assert_eq!(well.area(), 0.75 * 0.75);
    }

    fn blob() -> SolidMask {
        mask(&[
            "................",
            "....######......",
            "...#########....",
            "..###########...",
            "..############..",
            ".#############..",
            ".##########.....",
            ".#########......",
            "..########...##.",
            "..#########.###.",
            "...##########...",
            "....#########...",
            ".....#######....",
            "......####......",
            "................",
            "................",
        ])
    }

    #[test]
    fn tracing_is_deterministic() {
        let a = trace_mask(&blob());
        assert!(!a.is_empty());
        assert_eq!(a, trace_mask(&blob()));
        for polygon in &a.polygons {
            assert!(signed_area(polygon) > 0.0 && simple(polygon));
        }
    }

    const GRASS: [u8; 4] = [90, 170, 70, 255];
    const ROCK: [u8; 4] = [120, 120, 130, 255];
    const T: u32 = 16;

    /// One `T`-pixel tile drawn by `paint(x, y_down)`, labelled `terrains`.
    struct OneTile {
        pixels: Vec<u8>,
        analysis: TerrainAnalysis,
    }

    impl OneTile {
        fn new(
            kind: TileKind,
            terrains: TileTerrains,
            paint: impl Fn(u32, u32) -> [u8; 4],
        ) -> Self {
            let cells = (0..T).flat_map(|y| (0..T).map(move |x| (x, y)));
            let pixels = cells.flat_map(|(x, y)| paint(x, y)).collect();
            let terrain = |id, colour: [u8; 4]| crate::FoundTerrain {
                id,
                oklab: srgb_to_oklab(colour),
                swatch: [colour[0], colour[1], colour[2]],
                variance: 0.0,
                pure_tiles: Vec::new(),
                name: String::new(),
            };
            let labels = TileLabels {
                kind,
                terrains,
                confidence: [1.0; 9],
            };
            let analysis = TerrainAnalysis {
                terrains: vec![terrain(1, GRASS), terrain(2, ROCK)],
                tiles: vec![vec![Some(labels)]],
            };
            Self { pixels, analysis }
        }

        fn input(&self) -> SheetInput<'_> {
            SheetInput {
                name: "one",
                image: RgbaImage::new(T, T, &self.pixels).expect("sized"),
                layout: GridLayout {
                    tile: UVec2::splat(T),
                    margin: UVec2::ZERO,
                    spacing: UVec2::ZERO,
                    columns: 1,
                    rows: 1,
                },
            }
        }

        fn proposal(&self, mode: MatchMode, terrains: TileTerrains) -> TilesetProposal {
            let tile = FittedTile {
                address: ADDRESS,
                terrains,
                confidence: [1.0; 9],
                corrected: [false; 9],
            };
            TilesetProposal {
                sets: vec![ProposedSet {
                    mode,
                    inner: 2,
                    outer: 1,
                    tiles: vec![tile],
                    covered: 1,
                    total: 16,
                    missing: Vec::new(),
                    template: None,
                }],
                ..TilesetProposal::default()
            }
        }
    }

    const ADDRESS: TileAddress = TileAddress { sheet: 0, tile: 0 };

    fn rock_top(depth: u32) -> OneTile {
        let terrains = TileTerrains::filled(1)
            .with(Peer::TopLeft, 2)
            .with(Peer::Top, 2)
            .with(Peer::TopRight, 2);
        OneTile::new(TileKind::Terrain, terrains, move |_, y| {
            if y < depth { ROCK } else { GRASS }
        })
    }

    #[test]
    fn mask_marks_pixels_nearest_a_solid_peer() {
        let tile = rock_top(5);
        let rock = |id| id == 2;
        let mask = solid_mask(&tile.analysis, &[tile.input()], ADDRESS, None, rock, false);
        let mask = mask.expect("matching inputs").expect("analysed tile");
        assert_eq!(mask.count(), 5 * 16);
        assert!(mask.get(0, 15) && !mask.get(0, 10));
        let none = solid_mask(
            &tile.analysis,
            &[tile.input()],
            ADDRESS,
            None,
            |_| false,
            false,
        );
        assert_eq!(none.map(|m| m.map(|m| m.count())), Ok(Some(0)));
        let marked = solid_mask(
            &tile.analysis,
            &[tile.input()],
            ADDRESS,
            None,
            |_| false,
            true,
        );
        assert_eq!(marked.map(|m| m.map(|m| m.count())), Ok(Some(256)));
        let all_rock = TileTerrains::filled(2);
        let fitted = solid_mask(
            &tile.analysis,
            &[tile.input()],
            ADDRESS,
            Some(&all_rock),
            rock,
            false,
        );
        assert_eq!(fitted.map(|m| m.map(|m| m.count())), Ok(Some(256)));
    }

    #[test]
    fn solid_decor_marks_every_opaque_pixel() {
        let tile = OneTile::new(TileKind::Decor, TileTerrains::default(), |x, y| {
            if (4..12).contains(&x) && y >= 6 {
                [200, 40, 40, 255]
            } else if x == 0 {
                [200, 40, 40, 40]
            } else {
                [0; 4]
            }
        });
        let input = [tile.input()];
        let solid = solid_mask(&tile.analysis, &input, ADDRESS, None, |_| false, true);
        assert_eq!(solid.map(|m| m.map(|m| m.count())), Ok(Some(8 * 10)));
        let passable = solid_mask(&tile.analysis, &input, ADDRESS, None, |_| true, false);
        assert_eq!(passable.map(|m| m.map(|m| m.count())), Ok(Some(0)));
        let missing = TileAddress { sheet: 0, tile: 1 };
        let missing = solid_mask(&tile.analysis, &input, missing, None, |_| true, true);
        assert_eq!(missing, Ok(None));
        let mismatched = solid_mask(&tile.analysis, &[], ADDRESS, None, |_| true, true);
        assert!(matches!(mismatched, Err(FitError::SheetCount { .. })));
    }

    /// Traces `tile` alone with rock Solid, fitted in `mode` when given.
    fn traced(tile: &OneTile, mode: Option<MatchMode>) -> TileShape {
        let terrains = tile.analysis.tiles[0][0].expect("labelled").terrains;
        let proposal = mode.map_or_else(TilesetProposal::default, |m| tile.proposal(m, terrains));
        let shapes = trace_proposal(&tile.analysis, &[tile.input()], &proposal, rock, &[]);
        shapes
            .expect("matching inputs")
            .remove(&ADDRESS)
            .unwrap_or_default()
    }

    #[test]
    fn wall_faces_of_any_depth_become_a_half_tile() {
        let half = ring(&[(0.0, 0.5), (1.0, 0.5), (1.0, 1.0), (0.0, 1.0)]);
        for depth in [2, 3, 8, 12, 14] {
            // A tile in no set reads every labelled peer.
            for mode in [Some(MatchMode::CornersAndSides), None] {
                let shape = traced(&rock_top(depth), mode);
                assert!(shape.half_tile, "depth {depth}");
                assert_eq!(sorted(&shape), std::slice::from_ref(&half), "depth {depth}");
            }
        }
        // Without wall-over-floor peers a deep solid area keeps its trace.
        let terrains = TileTerrains::filled(1)
            .with(Peer::Centre, 2)
            .with(Peer::Top, 2);
        let deep = OneTile::new(TileKind::Terrain, terrains, |_, y| {
            if y < 14 { ROCK } else { GRASS }
        });
        let shape = traced(&deep, Some(MatchMode::Sides));
        assert!(!shape.half_tile);
        assert_eq!(shape.area(), 14.0 / 16.0);
    }

    /// Rock painted by `paint(x, y_down)` at `depth` pixels with rock `peers`.
    fn wall(peers: &[Peer], depth: u32, paint: fn(u32, u32, u32) -> bool) -> OneTile {
        let terrains = (peers.iter()).fold(TileTerrains::filled(1), |t, p| t.with(*p, 2));
        OneTile::new(TileKind::Terrain, terrains, move |x, y| {
            if paint(x, y, depth) { ROCK } else { GRASS }
        })
    }

    #[test]
    fn wall_corners_line_up_at_half_depth() {
        use Peer::{BottomLeft, BottomRight, Centre, Left, Right, Top, TopLeft, TopRight};
        let straight: fn(u32, u32, u32) -> bool = |_, y, d| y < d;
        let two_sides: fn(u32, u32, u32) -> bool = |x, y, d| y < d || x < d;
        let corner: fn(u32, u32, u32) -> bool = |x, y, d| y < d && x < d;
        let side_and_corner: fn(u32, u32, u32) -> bool =
            |x, y, d| y < d || (x >= 16 - d && y >= 16 - d);
        let cases = [
            (vec![TopLeft, Top, TopRight], straight, 2),
            (
                vec![TopLeft, Top, TopRight, Left, BottomLeft, Centre],
                two_sides,
                3,
            ),
            (vec![TopLeft], corner, 1),
            (
                vec![TopLeft, Top, TopRight, BottomRight],
                side_and_corner,
                3,
            ),
        ];
        let half = |v: f32| v == 0.0 || v == 0.5 || v == 1.0;
        // In a sides set a quadrant needs the centre and both sides beside it.
        let sides_wall = wall(&[Centre, Top, TopLeft, TopRight], 12, straight);
        let shape = traced(&sides_wall, Some(MatchMode::Sides));
        assert!(!shape.half_tile);
        let sides_wall = wall(&[Centre, Top, Left, Right], 12, straight);
        let shape = traced(&sides_wall, Some(MatchMode::Sides));
        assert!(shape.half_tile);
        assert_eq!(
            sorted(&shape),
            [ring(&[(0.0, 0.5), (1.0, 0.5), (1.0, 1.0), (0.0, 1.0)])]
        );
        for (peers, paint, quadrants) in cases {
            for depth in [5, 8, 12] {
                if depth != 8 && peers.contains(&BottomRight) {
                    continue;
                }
                for mode in [Some(MatchMode::CornersAndSides), None] {
                    let tile = wall(&peers, depth, paint);
                    let shape = traced(&tile, mode);
                    let case = format!("{peers:?} depth {depth} {mode:?}");
                    assert!(shape.half_tile, "{case}");
                    assert_eq!(shape.area(), quadrants as f32 / 4.0, "{case}");
                    assert!(
                        shape
                            .polygons
                            .iter()
                            .flatten()
                            .all(|v| half(v.x) && half(v.y)),
                        "{case}"
                    );
                }
            }
        }
    }

    fn rock(id: TerrainId) -> bool {
        id == 2
    }

    #[test]
    fn half_tile_needs_a_strip_and_a_solid_side() {
        // A footprint reaching into the far half is traced, not replaced.
        let terrains = TileTerrains::filled(1)
            .with(Peer::Centre, 2)
            .with(Peer::Top, 2);
        let cap = OneTile::new(TileKind::Terrain, terrains, |x, y| {
            if (4..12).contains(&x) && y < 12 {
                ROCK
            } else {
                GRASS
            }
        });
        let proposal = cap.proposal(MatchMode::Sides, terrains);
        let shapes = trace_proposal(&cap.analysis, &[cap.input()], &proposal, |id| id == 2, &[]);
        let shape = &shapes.expect("matching inputs")[&ADDRESS];
        assert!(!shape.half_tile);
        assert_eq!(shape.area(), 0.5 * 0.75);
        // A wall edge beside nothing drawn is traced as drawn.
        let terrains = TileTerrains::filled(0)
            .with(Peer::TopRight, 2)
            .with(Peer::Right, 2)
            .with(Peer::BottomRight, 2)
            .with(Peer::Centre, 2);
        let edge = OneTile::new(TileKind::Terrain, terrains, |x, _| {
            if x >= 6 { ROCK } else { [0; 4] }
        });
        let shapes = trace_proposal(
            &edge.analysis,
            &[edge.input()],
            &TilesetProposal::default(),
            rock,
            &[],
        );
        let shape = &shapes.expect("matching inputs")[&ADDRESS];
        assert!(!shape.half_tile);
        assert_eq!(shape.area(), 10.0 / 16.0);
        // Both sides passable: no half tile, nothing traced.
        let tile = rock_top(3);
        let proposal = tile.proposal(MatchMode::CornersAndSides, terrains);
        let shapes = trace_proposal(&tile.analysis, &[tile.input()], &proposal, |_| false, &[]);
        assert!(shapes.expect("matching inputs").is_empty());
    }

    #[test]
    fn trace_proposal_checks_its_inputs() {
        let tile = rock_top(3);
        let proposal = TilesetProposal::default();
        let result = trace_proposal(&tile.analysis, &[], &proposal, |_| true, &[]);
        assert!(matches!(result, Err(FitError::SheetCount { .. })));
    }
}
