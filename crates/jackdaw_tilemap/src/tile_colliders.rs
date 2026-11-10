//! Collision for one chunk of cells, built from the shapes of the tiles they
//! hold. Each tile's shapes are turned by its cell's orientation; whole-tile
//! cells merge into boxes, and every other shape splits into convex polygons.

use std::collections::{BTreeMap, HashMap};

use glam::Vec2;

use crate::{CHUNK_AREA, CHUNK_SIZE, CellRect, ChunkCoord, Placement, TileCollision, TileRef};

/// A tile's collision on one physics layer, as [`ChunkCollisionBuilder`]
/// reads it.
pub trait CollisionShape {
    fn layer(&self) -> u32;
    /// Overlaps without blocking.
    fn trigger(&self) -> bool;
    /// Blocks only what comes down onto it from above.
    fn one_way(&self) -> bool;
    /// Counter-clockwise outer rings in tile fractions, y up, origin at the
    /// tile's bottom-left.
    fn polygons(&self) -> &[Vec<Vec2>];
}

impl CollisionShape for TileCollision {
    fn layer(&self) -> u32 {
        self.layer
    }

    fn trigger(&self) -> bool {
        self.trigger
    }

    fn one_way(&self) -> bool {
        self.one_way
    }

    fn polygons(&self) -> &[Vec<Vec2>] {
        &self.polygons
    }
}

/// A chunk's collision on one physics layer, solid or trigger, in tiles
/// from the chunk's bottom-left corner.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CollisionGroup {
    pub layer: u32,
    pub trigger: bool,
    /// Never set on a trigger.
    pub one_way: bool,
    /// Boxes as `[min, max]`: whole tiles merged along rows, and rows of the
    /// same span merged upward.
    pub boxes: Vec<[Vec2; 2]>,
    /// Convex counter-clockwise polygons.
    pub polygons: Vec<Vec<Vec2>>,
}

/// A tile's shape on one layer, turned by one orientation.
#[derive(Clone, Debug)]
struct TurnedShape {
    layer: u32,
    trigger: bool,
    one_way: bool,
    /// The shape covers the whole tile.
    whole: bool,
    /// Convex counter-clockwise parts in tile fractions.
    parts: Vec<Vec<Vec2>>,
}

const ORIENTATION: u8 = TileRef::FLIP_X | TileRef::FLIP_Y | TileRef::TRANSPOSE;
const EPSILON: f32 = 1e-5;

/// Builds the collision of chunks, keeping the convex parts of each tile in
/// each orientation for the next cell that holds it. A builder serves one
/// set of tile shapes; shapes that change need a new builder.
#[derive(Clone, Debug, Default)]
pub struct ChunkCollisionBuilder {
    tiles: HashMap<(u16, u8), Vec<TurnedShape>>,
}

impl ChunkCollisionBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// The collision of the chunk at `coord` holding `cells`, by physics
    /// layer with solid, then one-way, then trigger. `shapes` gives the
    /// shapes of a tile by its index. Tiles placed between cells are cut to
    /// `bounds`, as they are drawn.
    pub fn build<'a, S: CollisionShape + 'a>(
        &mut self,
        coord: ChunkCoord,
        cells: &[TileRef; CHUNK_AREA],
        placement: Placement,
        bounds: Option<CellRect>,
        shapes: impl Fn(u16) -> &'a [S],
    ) -> Vec<CollisionGroup> {
        let mut groups: BTreeMap<(u32, bool, bool), (Vec<bool>, CollisionGroup)> = BTreeMap::new();
        for (local, &tile) in cells.iter().enumerate() {
            if tile.is_empty() {
                continue;
            }
            let Some((lo, hi)) = placement.visible_part(coord.cell(local), bounds) else {
                continue;
            };
            let uncut = lo == Vec2::ZERO && hi == Vec2::ONE;
            let turned = self
                .tiles
                .entry((tile.index(), tile.orientation() & ORIENTATION))
                .or_insert_with(|| turn_shapes(shapes(tile.index()), tile));
            let at = Vec2::new(
                (local as i32 % CHUNK_SIZE) as f32,
                (local as i32 / CHUNK_SIZE) as f32,
            );
            for shape in turned.iter() {
                let (whole, group) = groups
                    .entry((shape.layer, shape.trigger, shape.one_way))
                    .or_insert_with(|| {
                        let group = CollisionGroup {
                            layer: shape.layer,
                            trigger: shape.trigger,
                            one_way: shape.one_way,
                            ..Default::default()
                        };
                        (vec![false; CHUNK_AREA], group)
                    });
                if shape.whole && uncut {
                    whole[local] = true;
                    continue;
                }
                for part in &shape.parts {
                    let part = if uncut {
                        part.clone()
                    } else {
                        clip_to_rect(part, lo, hi)
                    };
                    if part.len() >= 3 && area(&part) > EPSILON {
                        group
                            .polygons
                            .push(part.into_iter().map(|p| p + at).collect());
                    }
                }
            }
        }
        (groups.into_values())
            .map(|(whole, mut group)| {
                group.boxes = merge_boxes(&whole);
                group
            })
            .filter(|group| !group.boxes.is_empty() || !group.polygons.is_empty())
            .collect()
    }
}

/// Where point `p` of a tile's shape lands when the tile is drawn under
/// `tile`'s orientation: transposed across the image's top-left diagonal,
/// then mirrored.
fn turn(p: Vec2, tile: TileRef) -> Vec2 {
    let mut p = p;
    if tile.transpose() {
        p = Vec2::new(1.0 - p.y, 1.0 - p.x);
    }
    if tile.flip_x() {
        p.x = 1.0 - p.x;
    }
    if tile.flip_y() {
        p.y = 1.0 - p.y;
    }
    p
}

fn turn_shapes<S: CollisionShape>(shapes: &[S], tile: TileRef) -> Vec<TurnedShape> {
    let mirrored = [tile.transpose(), tile.flip_x(), tile.flip_y()]
        .into_iter()
        .filter(|&bit| bit)
        .count()
        % 2
        == 1;
    (shapes.iter())
        .map(|shape| {
            let rings: Vec<Vec<Vec2>> = (shape.polygons().iter())
                .map(|ring| {
                    let mut ring: Vec<Vec2> = ring.iter().map(|&p| turn(p, tile)).collect();
                    if mirrored {
                        ring.reverse();
                    }
                    ring
                })
                .collect();
            let whole = matches!(rings.as_slice(), [ring] if covers_tile(ring));
            let parts = if whole {
                vec![vec![Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y]]
            } else {
                rings.iter().flat_map(|ring| convex_parts(ring)).collect()
            };
            TurnedShape {
                layer: shape.layer(),
                trigger: shape.trigger(),
                one_way: shape.one_way() && !shape.trigger(),
                whole,
                parts,
            }
        })
        .collect()
}

/// A ring inside the tile with the tile's area covers all of it.
fn covers_tile(ring: &[Vec2]) -> bool {
    let inside = |p: &Vec2| {
        p.cmpge(Vec2::splat(-EPSILON)).all() && p.cmple(Vec2::splat(1.0 + EPSILON)).all()
    };
    ring.iter().all(inside) && (area(ring) - 1.0).abs() < 1e-4
}

/// Signed area, positive for a counter-clockwise ring.
fn area(ring: &[Vec2]) -> f32 {
    let next = ring.iter().cycle().skip(1);
    ring.iter()
        .zip(next)
        .map(|(a, b)| a.perp_dot(*b))
        .sum::<f32>()
        / 2.0
}

fn cross(a: Vec2, b: Vec2, c: Vec2) -> f32 {
    (b - a).perp_dot(c - b)
}

/// `ring` without repeated or collinear points, counter-clockwise; empty
/// when nothing with area is left.
fn cleaned(ring: &[Vec2]) -> Vec<Vec2> {
    let mut points: Vec<Vec2> = Vec::with_capacity(ring.len());
    for &p in ring {
        if points
            .last()
            .is_none_or(|q| q.distance_squared(p) > EPSILON * EPSILON)
        {
            points.push(p);
        }
    }
    while points.len() > 1
        && points[0].distance_squared(points[points.len() - 1]) <= EPSILON * EPSILON
    {
        points.pop();
    }
    let mut changed = true;
    while changed && points.len() >= 3 {
        changed = false;
        let n = points.len();
        for i in 0..n {
            let (a, b, c) = (points[(i + n - 1) % n], points[i], points[(i + 1) % n]);
            if cross(a, b, c).abs() <= EPSILON * (b - a).length() * (c - b).length() {
                points.remove(i);
                changed = true;
                break;
            }
        }
    }
    let signed = area(&points);
    if points.len() < 3 || signed.abs() <= EPSILON {
        return Vec::new();
    }
    if signed < 0.0 {
        points.reverse();
    }
    points
}

fn is_convex(points: &[Vec2]) -> bool {
    let n = points.len();
    (0..n).all(|i| cross(points[i], points[(i + 1) % n], points[(i + 2) % n]) >= -EPSILON)
}

/// Splits a simple ring into convex counter-clockwise polygons: ear-clipped
/// into triangles, which then join across shared edges for as long as the
/// joined polygon stays convex.
fn convex_parts(ring: &[Vec2]) -> Vec<Vec<Vec2>> {
    let ring = cleaned(ring);
    if ring.is_empty() {
        return Vec::new();
    }
    if is_convex(&ring) {
        return vec![ring];
    }
    let mut parts: Vec<Vec<usize>> = triangulate(&ring).into_iter().map(Vec::from).collect();
    'joining: loop {
        for i in 0..parts.len() {
            for j in i + 1..parts.len() {
                let Some(joined) = join(&parts[i], &parts[j]) else {
                    continue;
                };
                let points: Vec<Vec2> = joined.iter().map(|&k| ring[k]).collect();
                if is_convex(&points) {
                    parts[i] = joined;
                    parts.swap_remove(j);
                    continue 'joining;
                }
            }
        }
        break;
    }
    (parts.into_iter())
        .map(|part| part.into_iter().map(|k| ring[k]).collect())
        .collect()
}

/// Ear-clips a counter-clockwise simple ring into triangles of its indices.
/// A ring that stops yielding ears, by crossing itself, keeps the triangles
/// found so far.
fn triangulate(ring: &[Vec2]) -> Vec<[usize; 3]> {
    let mut open: Vec<usize> = (0..ring.len()).collect();
    let mut triangles = Vec::with_capacity(ring.len().saturating_sub(2));
    while open.len() > 3 {
        let n = open.len();
        let ear = (0..n).find(|&i| {
            let (a, b, c) = (open[(i + n - 1) % n], open[i], open[(i + 1) % n]);
            let (pa, pb, pc) = (ring[a], ring[b], ring[c]);
            cross(pa, pb, pc) > EPSILON
                && !(open.iter())
                    .filter(|&&k| k != a && k != b && k != c)
                    .any(|&k| in_triangle(ring[k], pa, pb, pc))
        });
        let Some(i) = ear else {
            return triangles;
        };
        triangles.push([open[(i + n - 1) % n], open[i], open[(i + 1) % n]]);
        open.remove(i);
    }
    if let [a, b, c] = open[..] {
        triangles.push([a, b, c]);
    }
    triangles
}

/// Whether `p` lies in the counter-clockwise triangle `a b c` or on its
/// edges, other than at a corner.
fn in_triangle(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let at_corner = [a, b, c]
        .iter()
        .any(|q| q.distance_squared(p) <= EPSILON * EPSILON);
    !at_corner
        && (b - a).perp_dot(p - a) >= -EPSILON
        && (c - b).perp_dot(p - b) >= -EPSILON
        && (a - c).perp_dot(p - c) >= -EPSILON
}

/// Two counter-clockwise polygons of ring indices joined across the edge
/// they share, or `None` if they share none.
fn join(p: &[usize], q: &[usize]) -> Option<Vec<usize>> {
    let (n, m) = (p.len(), q.len());
    for k in 0..n {
        let (a, b) = (p[k], p[(k + 1) % n]);
        let Some(l) = (0..m).find(|&l| q[l] == b && q[(l + 1) % m] == a) else {
            continue;
        };
        let mut joined: Vec<usize> = (0..n).map(|i| p[(k + 1 + i) % n]).collect();
        joined.extend((2..m).map(|i| q[(l + i) % m]));
        return Some(joined);
    }
    None
}

/// The part of convex polygon `points` inside the box `lo` to `hi`, without
/// repeated points; empty when nothing with area is left.
fn clip_to_rect(points: &[Vec2], lo: Vec2, hi: Vec2) -> Vec<Vec2> {
    let planes: [(Vec2, f32); 4] = [
        (Vec2::X, lo.x),
        (Vec2::NEG_X, -hi.x),
        (Vec2::Y, lo.y),
        (Vec2::NEG_Y, -hi.y),
    ];
    let mut out = points.to_vec();
    for (normal, offset) in planes {
        let input = std::mem::take(&mut out);
        let n = input.len();
        for i in 0..n {
            let (a, b) = (input[i], input[(i + 1) % n]);
            let (da, db) = (normal.dot(a) - offset, normal.dot(b) - offset);
            if da >= 0.0 {
                out.push(a);
            }
            if (da >= 0.0) != (db >= 0.0) {
                out.push(a + (b - a) * (da / (da - db)));
            }
        }
    }
    cleaned(&out)
}

/// Boxes covering the set cells of a chunk mask: runs along each row, each
/// run grown upward through rows with a run of the same span.
fn merge_boxes(whole: &[bool]) -> Vec<[Vec2; 2]> {
    let mut boxes: Vec<[Vec2; 2]> = Vec::new();
    let mut below: Vec<(i32, i32, usize)> = Vec::new();
    for y in 0..CHUNK_SIZE {
        let mut row = Vec::new();
        let mut x = 0;
        while x < CHUNK_SIZE {
            if !whole[(y * CHUNK_SIZE + x) as usize] {
                x += 1;
                continue;
            }
            let start = x;
            while x < CHUNK_SIZE && whole[(y * CHUNK_SIZE + x) as usize] {
                x += 1;
            }
            let grown = (below.iter()).find(|&&(from, to, _)| from == start && to == x);
            let index = match grown {
                Some(&(_, _, index)) => {
                    boxes[index][1].y = (y + 1) as f32;
                    index
                }
                None => {
                    boxes.push([
                        Vec2::new(start as f32, y as f32),
                        Vec2::new(x as f32, (y + 1) as f32),
                    ]);
                    boxes.len() - 1
                }
            };
            row.push((start, x, index));
        }
        below = row;
    }
    boxes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CellCoord, ShapePreset};

    const SOLID: u16 = 0;
    const TOP_HALF: u16 = 1;
    const SLOPE_UP: u16 = 2;
    const TRIGGER: u16 = 3;
    const BARE: u16 = 4;
    const ELL: u16 = 5;

    fn table() -> Vec<Vec<TileCollision>> {
        let preset = |preset, trigger| {
            vec![TileCollision::preset(preset, 0, trigger).expect("a preset shape")]
        };
        let ell = [
            (0.0, 0.0),
            (1.0, 0.0),
            (1.0, 0.5),
            (0.5, 0.5),
            (0.5, 1.0),
            (0.0, 1.0),
        ];
        vec![
            preset(ShapePreset::Solid, false),
            preset(ShapePreset::TopHalf, false),
            preset(ShapePreset::SlopeUp, false),
            preset(ShapePreset::Solid, true),
            Vec::new(),
            vec![TileCollision {
                layer: 1,
                trigger: false,
                one_way: false,
                preset: ShapePreset::Traced,
                polygons: vec![ell.iter().map(|&(x, y)| Vec2::new(x, y)).collect()],
            }],
        ]
    }

    fn chunk(cells: &[(i32, i32, TileRef)]) -> [TileRef; CHUNK_AREA] {
        let mut out = [TileRef::EMPTY; CHUNK_AREA];
        for &(x, y, tile) in cells {
            out[CellCoord::new(x, y).local_index()] = tile;
        }
        out
    }

    fn build(cells: &[(i32, i32, TileRef)]) -> Vec<CollisionGroup> {
        build_placed(cells, Placement::OnCells, None)
    }

    fn build_placed(
        cells: &[(i32, i32, TileRef)],
        placement: Placement,
        bounds: Option<CellRect>,
    ) -> Vec<CollisionGroup> {
        let table = table();
        ChunkCollisionBuilder::new().build(
            ChunkCoord::new(0, 0),
            &chunk(cells),
            placement,
            bounds,
            |index| table[index as usize].as_slice(),
        )
    }

    fn total_area(polygons: &[Vec<Vec2>]) -> f32 {
        polygons.iter().map(|p| area(p)).sum()
    }

    /// The same points, whichever one the ring starts at.
    fn same_ring(a: &[Vec2], b: &[Vec2]) -> bool {
        a.len() == b.len()
            && (0..a.len())
                .any(|shift| (0..a.len()).all(|i| a[(i + shift) % a.len()].distance(b[i]) < 1e-5))
    }

    #[test]
    fn a_row_of_solid_tiles_merges_into_one_box() {
        let row: Vec<_> = (2..7).map(|x| (x, 3, TileRef::new(SOLID))).collect();
        let groups = build(&row);
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0].boxes,
            vec![[Vec2::new(2.0, 3.0), Vec2::new(7.0, 4.0)]]
        );
        assert!(groups[0].polygons.is_empty());
    }

    #[test]
    fn rows_of_the_same_span_merge_upward_and_others_stay_apart() {
        let mut cells = Vec::new();
        for y in 0..3 {
            cells.extend((0..4).map(|x| (x, y, TileRef::new(SOLID))));
        }
        cells.extend((0..2).map(|x| (x, 3, TileRef::new(SOLID))));
        cells.push((10, 0, TileRef::new(SOLID)));
        let groups = build(&cells);
        assert_eq!(
            groups[0].boxes,
            vec![
                [Vec2::new(0.0, 0.0), Vec2::new(4.0, 3.0)],
                [Vec2::new(10.0, 0.0), Vec2::new(11.0, 1.0)],
                [Vec2::new(0.0, 3.0), Vec2::new(2.0, 4.0)],
            ]
        );
    }

    #[test]
    fn half_and_slope_tiles_become_polygons_at_their_cells() {
        let groups = build(&[
            (1, 0, TileRef::new(TOP_HALF)),
            (2, 0, TileRef::new(SLOPE_UP)),
        ]);
        assert_eq!(groups.len(), 1);
        let group = &groups[0];
        assert!(group.boxes.is_empty());
        let half = [(1.0, 0.5), (2.0, 0.5), (2.0, 1.0), (1.0, 1.0)].map(Vec2::from);
        let slope = [(2.0, 0.0), (3.0, 0.0), (3.0, 1.0)].map(Vec2::from);
        assert!(same_ring(&group.polygons[0], &half), "{:?}", group.polygons);
        assert!(
            same_ring(&group.polygons[1], &slope),
            "{:?}",
            group.polygons
        );
    }

    #[test]
    fn triggers_form_their_own_group_after_the_solid_one() {
        let groups = build(&[
            (0, 0, TileRef::new(TRIGGER)),
            (1, 0, TileRef::new(TRIGGER)),
            (0, 1, TileRef::new(SOLID)),
        ]);
        let kinds: Vec<_> = groups.iter().map(|g| (g.layer, g.trigger)).collect();
        assert_eq!(kinds, vec![(0, false), (0, true)]);
        assert_eq!(groups[1].boxes, vec![[Vec2::ZERO, Vec2::new(2.0, 1.0)]]);
    }

    #[test]
    fn a_mirrored_slope_falls_the_other_way_and_stays_counter_clockwise() {
        let groups = build(&[(0, 0, TileRef::new(SLOPE_UP).with_flip_x(true))]);
        let ring = &groups[0].polygons[0];
        let falling = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0)].map(Vec2::from);
        assert!(same_ring(ring, &falling), "{ring:?}");
        assert!(area(ring) > 0.0);
    }

    #[test]
    fn a_transposed_top_half_covers_the_left_half() {
        let groups = build(&[(0, 0, TileRef::new(TOP_HALF).with_transpose(true))]);
        let ring = &groups[0].polygons[0];
        let left = [(0.0, 0.0), (0.5, 0.0), (0.5, 1.0), (0.0, 1.0)].map(Vec2::from);
        assert!(same_ring(ring, &left), "{ring:?}");
    }

    #[test]
    fn a_turned_solid_tile_still_merges() {
        let groups = build(&[
            (0, 0, TileRef::new(SOLID)),
            (
                1,
                0,
                TileRef::new(SOLID).with_transpose(true).with_flip_y(true),
            ),
        ]);
        assert_eq!(groups[0].boxes, vec![[Vec2::ZERO, Vec2::new(2.0, 1.0)]]);
    }

    #[test]
    fn a_concave_shape_splits_into_convex_parts_of_the_same_area() {
        let groups = build(&[(4, 4, TileRef::new(ELL))]);
        assert_eq!(groups[0].layer, 1);
        let parts = &groups[0].polygons;
        assert!(parts.len() >= 2, "{parts:?}");
        assert!(parts.iter().all(|p| is_convex(p) && area(p) > 0.0));
        assert!((total_area(parts) - 0.75).abs() < 1e-5);
        let inside = |p: &Vec2| p.cmpge(Vec2::splat(4.0)).all() && p.cmple(Vec2::splat(5.0)).all();
        assert!(parts.iter().flatten().all(inside));
    }

    #[test]
    fn a_tile_between_cells_is_cut_at_the_bounds() {
        let bounds = CellRect {
            min: glam::IVec2::new(2, 0),
            max: glam::IVec2::new(6, 4),
        };
        let groups = build_placed(
            &[(1, 0, TileRef::new(SOLID)), (3, 0, TileRef::new(SOLID))],
            Placement::BetweenCells,
            Some(bounds),
        );
        let group = &groups[0];
        assert_eq!(
            group.boxes,
            vec![[Vec2::new(3.0, 0.0), Vec2::new(4.0, 1.0)]]
        );
        assert_eq!(group.polygons.len(), 1, "{:?}", group.polygons);
        let cut = [(1.5, 0.0), (2.0, 0.0), (2.0, 1.0), (1.5, 1.0)].map(Vec2::from);
        assert!(same_ring(&group.polygons[0], &cut), "{:?}", group.polygons);
    }

    #[test]
    fn a_cut_through_a_corner_keeps_no_repeated_point() {
        let ring = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 0.5)].map(Vec2::from);
        let cut = clip_to_rect(&ring, Vec2::ZERO, Vec2::new(1.0, 0.5));
        let expected = [(0.0, 0.0), (1.0, 0.0), (1.0, 0.5), (0.0, 0.5)].map(Vec2::from);
        assert!(same_ring(&cut, &expected), "{cut:?}");
        assert!(is_convex(&cut) && area(&cut) > 0.0);
    }

    #[test]
    fn cells_without_shapes_build_nothing() {
        assert!(build(&[(0, 0, TileRef::new(BARE))]).is_empty());
        assert!(build(&[]).is_empty());
    }

    struct Flagged {
        trigger: bool,
        one_way: bool,
        polygons: Vec<Vec<Vec2>>,
    }

    impl CollisionShape for Flagged {
        fn layer(&self) -> u32 {
            0
        }

        fn trigger(&self) -> bool {
            self.trigger
        }

        fn one_way(&self) -> bool {
            self.one_way
        }

        fn polygons(&self) -> &[Vec<Vec2>] {
            &self.polygons
        }
    }

    #[test]
    fn one_way_tiles_form_their_own_group_and_triggers_are_never_one_way() {
        let square = vec![vec![Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y]];
        let table: Vec<Vec<Flagged>> = [(false, false), (false, true), (true, true)]
            .into_iter()
            .map(|(trigger, one_way)| {
                vec![Flagged {
                    trigger,
                    one_way,
                    polygons: square.clone(),
                }]
            })
            .collect();
        let cells = chunk(&[
            (0, 0, TileRef::new(0)),
            (1, 0, TileRef::new(1)),
            (2, 0, TileRef::new(1)),
            (3, 0, TileRef::new(2)),
        ]);
        let groups = ChunkCollisionBuilder::new().build(
            ChunkCoord::new(0, 0),
            &cells,
            Placement::OnCells,
            None,
            |index| table[index as usize].as_slice(),
        );
        let kinds: Vec<_> = groups.iter().map(|g| (g.trigger, g.one_way)).collect();
        assert_eq!(kinds, vec![(false, false), (false, true), (true, false)]);
        assert_eq!(
            groups[1].boxes,
            vec![[Vec2::new(1.0, 0.0), Vec2::new(3.0, 1.0)]]
        );
    }

    #[test]
    fn a_self_crossing_ring_keeps_what_it_can_and_never_hangs() {
        let bow = [(0.0, 0.0), (1.0, 1.0), (1.0, 0.0), (0.0, 1.0)].map(Vec2::from);
        let parts = convex_parts(&bow);
        assert!(parts.iter().all(|p| is_convex(p)));
    }
}
