//! Concentric LOD rings around a viewer.
//!
//! A terrain is drawn as a few square levels centred on the camera. Level
//! 0 samples every grid point; each level out doubles the step and covers
//! four times the ground for the same vertex count, and skips the square
//! the finer level already draws. The outermost level covers whatever is
//! left of the terrain, so nothing is ever missing however far the camera
//! moves.
//!
//! Two rules keep the joins closed:
//!
//! - Every level's origin is snapped to twice its own step, which is the
//!   next level's step. A level's boundary therefore falls on grid lines
//!   the coarser level also has vertices on.
//! - A level's outer ring of vertices is snapped to the coarser grid:
//!   the vertices that sit between two coarse ones take their average, so
//!   the fine edge is exactly the coarse edge and no crack opens between
//!   them.
//!
//! Nothing here knows about cameras, entities or materials: it takes a
//! viewer position in the terrain's own grid coordinates and a borrowed
//! heightmap, and returns mesh data.

use bevy_math::{IVec2, Vec2};

use crate::heightmap::Heightmap;

/// Raw mesh data for one piece of terrain surface, ready to be turned
/// into a Bevy `Mesh`.
#[derive(Clone, Debug)]
pub struct SurfaceMeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    /// Grid coordinate each vertex was sampled from, parallel to
    /// `positions`.
    ///
    /// The smooth and flat surfaces emit vertices in different orders and
    /// different counts, so a caller building a per-vertex attribute of
    /// its own reads this rather than walking the grid.
    pub grid: Vec<[u32; 2]>,
}

/// Cells per side of every level's square, at that level's own step.
///
/// Even, so a level's boundary vertices land on the coarser grid. The
/// vertex budget per level is `(RING_CELLS + 1)^2`.
pub const RING_CELLS: u32 = 64;

/// Levels a terrain is drawn with at most, however large it is. Past this
/// the outermost level covers more ground per quad.
pub const MAX_LEVELS: u32 = 8;

/// How coarse the outermost level may get before another level is worth
/// having: quads per side of the whole terrain.
const COARSE_BUDGET: u32 = RING_CELLS * 2;

/// One LOD level: a square of quads at one step, minus the square the
/// finer level covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClipmapLevel {
    /// 0 is the finest.
    pub level: u32,
    /// Grid points between neighbouring vertices of this level.
    pub step: u32,
    /// Grid coordinate of the level's minimum corner. Negative when the
    /// camera sits near or past the terrain's edge; the quads outside the
    /// terrain are dropped, not clamped.
    pub origin: IVec2,
    /// Quads per side.
    pub cells: u32,
    /// The finer level's square, in grid coordinates, which this level
    /// leaves to it.
    pub hole: Option<GridSquare>,
    /// Whether this level's outer ring snaps to the coarser grid. False
    /// only on the outermost level, which has no coarser neighbour.
    pub snap_outer: bool,
}

impl ClipmapLevel {
    /// The square this level covers, in grid coordinates.
    pub fn square(&self) -> GridSquare {
        GridSquare {
            min: self.origin,
            max: self.origin + IVec2::splat((self.cells * self.step) as i32),
        }
    }

    /// Vertices per side.
    pub fn vertices(&self) -> u32 {
        self.cells + 1
    }

    /// Whether `other` samples the same vertices as this level: same
    /// grid, same extent, same edge treatment, differing at most in the
    /// square it leaves to the finer level.
    ///
    /// Two levels that agree here have identical positions, normals, UVs
    /// and grid coordinates, so moving from one to the other is an index
    /// rewrite.
    ///
    /// The terrain's own resolution is not compared, because a level
    /// carries no record of it. A resolution change reaches callers as a
    /// whole-terrain rebuild, which remeshes rather than asking this.
    pub fn same_lattice(&self, other: &Self) -> bool {
        self.level == other.level
            && self.step == other.step
            && self.origin == other.origin
            && self.cells == other.cells
            && self.snap_outer == other.snap_outer
    }
}

/// A block of grid coordinates. Both `min` and `max` are grid points a
/// level has vertices on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSquare {
    pub min: IVec2,
    pub max: IVec2,
}

impl GridSquare {
    /// Whether the quad spanning `[min, max]` lies wholly inside.
    fn contains_quad(&self, min: IVec2, max: IVec2) -> bool {
        min.x >= self.min.x && min.y >= self.min.y && max.x <= self.max.x && max.y <= self.max.y
    }

    /// Whether `other` lies wholly inside this square.
    pub fn contains_square(&self, other: &GridSquare) -> bool {
        self.contains_quad(other.min, other.max)
    }

    /// Whether this square holds `other` with `margin` grid points of
    /// clearance on every side of it that has terrain beyond, `bound`
    /// being the terrain's own extent.
    ///
    /// The clearance keeps the seam closed. A finer square lying flush
    /// against this one leaves no quads of this level between the two:
    /// every quad along that side is inside the hole and dropped, so the
    /// finer level's outer ring, snapped for this level's step, abuts
    /// what the next level out draws at twice that step, and the two
    /// part. One of this level's own steps of clearance puts a column of
    /// its quads there, so the vertices the finer ring snapped against
    /// are this level's plain samples rather than its outer ring.
    ///
    /// Two exemptions, both where nothing is drawn on the other side: a
    /// side running along the terrain's edge, and a finer square with no
    /// terrain under it at all.
    pub fn holds_clear_of(&self, other: &GridSquare, bound: &GridSquare, margin: i32) -> bool {
        let inner_min = other.min.max(bound.min);
        let inner_max = other.max.min(bound.max);
        if inner_min.x >= inner_max.x || inner_min.y >= inner_max.y {
            return true;
        }
        let outer_min = self.min.max(bound.min);
        let outer_max = self.max.min(bound.max);
        // Every side measured outward, so the two low sides read the same
        // way as the two high ones.
        let clear = |inner: i32, outer: i32, edge: i32| {
            inner <= outer && (inner == edge || inner + margin <= outer)
        };
        clear(inner_max.x, outer_max.x, bound.max.x)
            && clear(inner_max.y, outer_max.y, bound.max.y)
            && clear(-inner_min.x, -outer_min.x, -bound.min.x)
            && clear(-inner_min.y, -outer_min.y, -bound.min.y)
    }
}

/// The levels a terrain of `resolution` grid points per edge is drawn
/// with for a viewer at `viewer`, in fractional grid coordinates.
///
/// Fewer levels for a small terrain: a level is only worth adding while
/// the outermost one would otherwise carry more than the coarse budget of
/// quads per side.
pub fn clipmap_levels(resolution: u32, viewer: Vec2) -> Vec<ClipmapLevel> {
    let cells = resolution.saturating_sub(1);
    if cells == 0 {
        return Vec::new();
    }
    let count = level_count(cells);

    let mut levels: Vec<ClipmapLevel> = Vec::with_capacity(count as usize);
    for level in 0..count {
        let step = 1u32 << level;
        let hole = levels.last().map(ClipmapLevel::square);
        let outermost = level + 1 == count;
        let (origin, level_cells) = if outermost {
            // Whatever is left of the terrain, so no camera position can
            // leave ground undrawn.
            (IVec2::ZERO, cells.div_ceil(step))
        } else {
            let half = (RING_CELLS / 2 * step) as f32;
            let snap = (2 * step) as i32;
            let origin = IVec2::new(
                snap_floor((viewer.x - half).floor() as i32, snap),
                snap_floor((viewer.y - half).floor() as i32, snap),
            );
            (origin, RING_CELLS)
        };
        levels.push(ClipmapLevel {
            level,
            step,
            origin,
            cells: level_cells,
            hole,
            snap_outer: !outermost,
        });
    }
    levels
}

/// Levels worth drawing a terrain of `cells` quads per edge with.
fn level_count(cells: u32) -> u32 {
    for count in 1..MAX_LEVELS {
        if cells.div_ceil(1 << (count - 1)) <= COARSE_BUDGET {
            return count;
        }
    }
    MAX_LEVELS
}

/// Round down to a multiple of `step`, for negative coordinates too.
fn snap_floor(value: i32, step: i32) -> i32 {
    value.div_euclid(step) * step
}

/// Mesh data for one level, sampled from a borrowed heightmap.
///
/// `present` is asked for each quad whether the ground under it is
/// authored, by the grid coordinate of one of its corners. A quad no
/// region owns is dropped, so an absent region costs its quads and
/// nothing else.
///
/// Quads that fall outside the terrain, and quads the finer level
/// already draws, are dropped the same way. The vertex lattice is emitted
/// whole: keeping it rectangular is what lets the outer ring find its
/// neighbours.
pub fn build_clipmap_mesh_data(
    heightmap: &Heightmap,
    level: &ClipmapLevel,
    present: impl Fn(i32, i32) -> bool,
) -> SurfaceMeshData {
    let res = heightmap.resolution;
    let last = res.saturating_sub(1) as i32;
    let cell = heightmap.cell_size();
    // Where the map says its first vertex sits: a terrain's cells are
    // wherever its regions are, not necessarily centred on the entity.
    let at = heightmap.origin;
    let step = level.step as i32;
    let side = level.vertices();

    let grid_at =
        |i: u32, j: u32| -> IVec2 { level.origin + IVec2::new(i as i32 * step, j as i32 * step) };
    let sample = |g: IVec2| -> f32 {
        let x = g.x.clamp(0, last) as u32;
        let z = g.y.clamp(0, last) as u32;
        heightmap.get_height(x, z)
    };

    let count = (side as usize) * (side as usize);
    let mut positions = Vec::with_capacity(count);
    let mut normals = Vec::with_capacity(count);
    let mut uvs = Vec::with_capacity(count);
    let mut grid = Vec::with_capacity(count);

    for j in 0..side {
        for i in 0..side {
            let g = grid_at(i, j);
            let mut height = sample(g);
            if level.snap_outer {
                // The ring this level hands over to the coarser one. A
                // vertex the coarse grid does not have takes the average
                // of the two it does, which puts the fine edge exactly on
                // the coarse edge.
                let on_x_edge = i == 0 || i == side - 1;
                let on_z_edge = j == 0 || j == side - 1;
                if on_x_edge && j % 2 == 1 {
                    height = 0.5 * (sample(grid_at(i, j - 1)) + sample(grid_at(i, j + 1)));
                } else if on_z_edge && i % 2 == 1 {
                    height = 0.5 * (sample(grid_at(i - 1, j)) + sample(grid_at(i + 1, j)));
                }
            }

            let clamped = IVec2::new(g.x.clamp(0, last), g.y.clamp(0, last));
            // The outermost level is the only one that may run past the
            // terrain and still has to reach its edge: a step that does
            // not divide the terrain evenly would otherwise leave a strip
            // along two sides with nothing under it. Inner levels drop
            // what hangs over instead, because the level outside them
            // already draws exactly that ground.
            let placed = if level.snap_outer { g } else { clamped };
            positions.push([
                placed.x as f32 * cell.x + at.x,
                height,
                placed.y as f32 * cell.y + at.y,
            ]);
            grid.push([clamped.x as u32, clamped.y as u32]);
            let denominator = last.max(1) as f32;
            uvs.push([
                clamped.x as f32 / denominator,
                clamped.y as f32 / denominator,
            ]);

            // Slopes across this level's own step, so a coarse level is
            // lit by the shape it actually draws rather than by detail it
            // stepped over.
            let dx = (sample(g + IVec2::new(step, 0)) - sample(g - IVec2::new(step, 0)))
                / (2.0 * cell.x * step as f32);
            let dz = (sample(g + IVec2::new(0, step)) - sample(g - IVec2::new(0, step)))
                / (2.0 * cell.y * step as f32);
            let length = (dx * dx + 1.0 + dz * dz).sqrt();
            normals.push([-dx / length, 1.0 / length, -dz / length]);
        }
    }

    SurfaceMeshData {
        positions,
        normals,
        uvs,
        indices: build_clipmap_indices(res, level, present),
        grid,
    }
}

/// The triangles of one level, over the vertex lattice
/// [`build_clipmap_mesh_data`] emits for the same level.
///
/// Which quads survive depends on the level's grid, the terrain's edge,
/// the square left to the finer level, and what `present` owns, never on a
/// height. Two levels that agree on [`ClipmapLevel::same_lattice`] share
/// every vertex and differ only here, so a moved hole rewrites indices
/// rather than remeshing the surface.
pub fn build_clipmap_indices(
    resolution: u32,
    level: &ClipmapLevel,
    present: impl Fn(i32, i32) -> bool,
) -> Vec<u32> {
    let last = resolution.saturating_sub(1) as i32;
    let step = level.step as i32;
    let side = level.vertices();
    let grid_at =
        |i: u32, j: u32| -> IVec2 { level.origin + IVec2::new(i as i32 * step, j as i32 * step) };

    let mut indices = Vec::new();
    for j in 0..level.cells {
        for i in 0..level.cells {
            let min = grid_at(i, j);
            let max = grid_at(i + 1, j + 1);
            let inside = min.x >= 0 && min.y >= 0 && max.x <= last && max.y <= last;
            if level.snap_outer {
                if !inside {
                    continue;
                }
            } else {
                // Clamped to the terrain: a quad wholly outside it
                // collapses to nothing and is dropped, and one straddling
                // the edge keeps the part that is inside.
                let a = min.clamp(IVec2::ZERO, IVec2::splat(last));
                let b = max.clamp(IVec2::ZERO, IVec2::splat(last));
                if a.x >= b.x || a.y >= b.y {
                    continue;
                }
            }
            // The finer level draws this only where the terrain is: what
            // it dropped for hanging over the edge has to be drawn here.
            if inside && level.hole.is_some_and(|hole| hole.contains_quad(min, max)) {
                continue;
            }
            // A coarse quad can span regions, so it stays as long as any
            // corner of it is owned.
            let a = min.clamp(IVec2::ZERO, IVec2::splat(last));
            let b = (max - IVec2::ONE).clamp(IVec2::ZERO, IVec2::splat(last));
            let owned = [(a.x, a.y), (b.x, a.y), (a.x, b.y), (b.x, b.y)]
                .into_iter()
                .any(|(x, z)| present(x, z));
            if !owned {
                continue;
            }

            let tl = j * side + i;
            let tr = tl + 1;
            let bl = (j + 1) * side + i;
            let br = bl + 1;
            indices.extend_from_slice(&[tl, bl, tr, tr, bl, br]);
        }
    }
    indices
}

/// How many levels may follow the camera to a new origin in one frame.
///
/// A gesture crossing many snap boundaries in a row re-snaps every level
/// on every frame of it, and the coarse levels are the expensive ones. One
/// level per frame keeps the ground under the camera exact and lets the
/// rest arrive over the following frames.
pub const REBUILD_BUDGET: usize = 1;

/// What each level has to be built as this frame.
///
/// `targets` is what [`clipmap_levels`] says the camera wants, `built` is
/// what each level is currently drawing (`None` for a level with no mesh
/// yet), and `forced` marks levels whose heights or region cover changed
/// and which therefore have to be remeshed. The result is one entry per
/// level: `Some(state)` for a level to build, `None` for one that stands.
///
/// Two decisions, in that order. First, which levels take their target
/// position: at most `budget` of them per frame, finest first. Being
/// forced does not count against the budget, since an edit changes the
/// ground under a level rather than where the level belongs, so a forced
/// level is remeshed where it stands. Second, what each level hands to
/// the one inside it.
///
/// Two invariants survive a level being left behind:
///
/// - Every level's hole is the square the finer level draws after this
///   frame's decisions, not the square it would draw had it been allowed
///   to move. A level left behind keeps its hole and the level inside it
///   keeps filling it, so no frame shows a gap.
/// - Every level's square holds the finer level's with a clear step of
///   its own grid to spare (see [`GridSquare::holds_clear_of`]), so the
///   finer ring abuts quads of the level it snapped against. Whichever of
///   a pair is lagging moves to restore that, budget or no budget; two
///   levels both at their targets always clear each other, which is what
///   makes the repair settle.
pub fn plan_rebuilds(
    targets: &[ClipmapLevel],
    built: &[Option<ClipmapLevel>],
    forced: &[bool],
    budget: usize,
) -> Vec<Option<ClipmapLevel>> {
    let count = targets.len();
    // The outermost level covers the whole terrain by construction, so
    // its square is the terrain's own extent.
    let Some(bound) = targets.last().map(ClipmapLevel::square) else {
        return Vec::new();
    };
    let standing: Vec<Option<ClipmapLevel>> = (0..count)
        .map(|index| built.get(index).copied().flatten())
        .collect();

    // Which levels sit at their target this frame. A level already
    // standing there costs nothing to keep there; one with nothing built
    // has no other position to take.
    let mut moves = vec![false; count];
    let mut spent = 0;
    for index in 0..count {
        match standing[index] {
            None => moves[index] = true,
            Some(level) if level.same_lattice(&targets[index]) => moves[index] = true,
            Some(_) if spent < budget => {
                moves[index] = true;
                spent += 1;
            }
            Some(_) => {}
        }
    }

    let square = |index: usize, moves: &[bool]| -> GridSquare {
        if moves[index] {
            targets[index].square()
        } else {
            standing[index]
                .expect("a level that stays has one to stay at")
                .square()
        }
    };
    loop {
        let mut again = false;
        for index in 1..count {
            let margin = targets[index].step as i32;
            if square(index, &moves).holds_clear_of(&square(index - 1, &moves), &bound, margin) {
                continue;
            }
            if !moves[index] {
                moves[index] = true;
            } else if !moves[index - 1] {
                moves[index - 1] = true;
            } else {
                // Both are at their targets and still too close, which
                // the layout `clipmap_levels` produces cannot do. Nothing
                // is left to move, so stop rather than spin.
                continue;
            }
            again = true;
        }
        if !again {
            break;
        }
    }

    let mut plan = vec![None; count];
    let mut inner: Option<GridSquare> = None;
    for index in 0..count {
        let mut chosen = if moves[index] {
            targets[index]
        } else {
            standing[index].expect("a level that stays has one to stay at")
        };
        chosen.hole = inner;
        inner = Some(chosen.square());
        // A forced level is remeshed where it stands: what changed is the
        // ground under it, which its position cannot show.
        if forced.get(index).copied().unwrap_or(false) || standing[index] != Some(chosen) {
            plan[index] = Some(chosen);
        }
    }
    plan
}

/// The same surface with nothing shared: three vertices per triangle,
/// each carrying that triangle's face normal.
///
/// What a quantized terrain is drawn with. Smoothed vertex normals read a
/// snapped height field as a soft ramp, hiding the terracing that is in
/// the data.
pub fn flat_shaded(data: SurfaceMeshData) -> SurfaceMeshData {
    let mut out = SurfaceMeshData {
        positions: Vec::with_capacity(data.indices.len()),
        normals: Vec::with_capacity(data.indices.len()),
        uvs: Vec::with_capacity(data.indices.len()),
        indices: Vec::with_capacity(data.indices.len()),
        grid: Vec::with_capacity(data.indices.len()),
    };
    for triangle in data.indices.chunks_exact(3) {
        let [a, b, c] = [
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        ];
        let normal = face_normal(data.positions[a], data.positions[b], data.positions[c]);
        for index in [a, b, c] {
            out.indices.push(out.positions.len() as u32);
            out.positions.push(data.positions[index]);
            out.uvs.push(data.uvs[index]);
            out.grid.push(data.grid[index]);
            out.normals.push(normal);
        }
    }
    out
}

/// Normal of the triangle `a, b, c`, wound counter-clockwise when seen
/// from the front. A degenerate triangle reports straight up rather than
/// a NaN that would blacken the surface.
fn face_normal(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    let ab = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let ac = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let n = [
        ab[1] * ac[2] - ab[2] * ac[1],
        ab[2] * ac[0] - ab[0] * ac[2],
        ab[0] * ac[1] - ab[1] * ac[0],
    ];
    let length = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if length <= f32::EPSILON {
        return [0.0, 1.0, 0.0];
    }
    [n[0] / length, n[1] / length, n[2] / length]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A terrain `resolution` points per edge at one world unit per cell.
    fn ground(resolution: u32) -> Heightmap {
        Heightmap::new(
            resolution,
            Vec2::splat((resolution - 1) as f32),
            (resolution) as f32,
        )
    }

    fn hills(resolution: u32) -> Heightmap {
        let mut map = ground(resolution);
        for z in 0..resolution {
            for x in 0..resolution {
                let h = ((x as f32) * 0.37).sin() * 3.0 + ((z as f32) * 0.21).cos() * 2.0;
                map.set_height(x, z, h);
            }
        }
        map
    }

    fn everywhere(_: i32, _: i32) -> bool {
        true
    }

    #[test]
    fn a_small_terrain_needs_one_level_and_a_large_one_needs_several() {
        assert_eq!(clipmap_levels(65, Vec2::splat(32.0)).len(), 1);
        assert_eq!(level_count(128), 1);
        assert_eq!(level_count(129), 2);
        assert_eq!(level_count(256), 2);
        assert_eq!(level_count(4096), 6);
    }

    /// Every level's origin lands on the next level's grid, which is what
    /// puts a level's boundary on grid lines the coarser one shares.
    #[test]
    fn every_level_is_snapped_to_the_coarser_grid() {
        for viewer in [
            Vec2::new(0.0, 0.0),
            Vec2::new(133.7, 41.2),
            Vec2::new(-9.5, 500.25),
        ] {
            for level in clipmap_levels(1025, viewer) {
                let snap = 2 * level.step as i32;
                assert_eq!(
                    level.origin.rem_euclid(IVec2::splat(snap)),
                    IVec2::ZERO,
                    "level {} origin {:?} is off the coarser grid",
                    level.level,
                    level.origin,
                );
                assert_eq!(level.cells % 2, 0, "an odd level cannot halve cleanly");
            }
        }
    }

    /// The finer level's square has to sit inside the coarser level's
    /// hole exactly, or a level draws over what the finer one is drawing.
    #[test]
    fn each_levels_hole_is_the_finer_levels_square() {
        let levels = clipmap_levels(1025, Vec2::new(511.0, 511.0));
        for pair in levels.windows(2) {
            assert_eq!(pair[1].hole, Some(pair[0].square()));
        }
        assert_eq!(levels[0].hole, None);
    }

    /// The outermost level covers the whole terrain whatever the camera
    /// is doing, so no view can leave ground undrawn.
    #[test]
    fn the_outermost_level_covers_the_whole_terrain() {
        for viewer in [
            Vec2::splat(0.0),
            Vec2::splat(1024.0),
            Vec2::new(-4000.0, 900.0),
        ] {
            let levels = clipmap_levels(1025, viewer);
            let outer = levels.last().expect("a terrain has levels");
            let square = outer.square();
            assert!(square.min.x <= 0 && square.min.y <= 0);
            assert!(square.max.x >= 1024 && square.max.y >= 1024, "{square:?}");
            assert!(!outer.snap_outer, "the outermost ring has nothing to meet");
        }
    }

    /// The join between two levels: every vertex the fine level puts on
    /// the shared edge has to lie on the surface the coarse level draws
    /// there, or the two part and a crack shows through.
    ///
    /// Two cases. A vertex the coarse grid also has must match its
    /// height. A vertex between two coarse ones has no counterpart, so it
    /// must lie on the straight segment the coarse level draws between
    /// them, measured against the coarse surface rather than against the
    /// fine level's own neighbours.
    #[test]
    fn the_ring_boundary_carries_no_crack() {
        let map = hills(257);
        let levels = clipmap_levels(257, Vec2::splat(128.0));
        assert!(levels.len() >= 2, "this terrain needs a ring boundary");

        let fine = &levels[0];
        let fine_data = build_clipmap_mesh_data(&map, fine, everywhere);
        let coarse_data = build_clipmap_mesh_data(&map, &levels[1], everywhere);
        let coarse_height = |x: f32, z: f32| -> Option<f32> {
            coarse_data
                .positions
                .iter()
                .find(|q| (q[0] - x).abs() < 1e-3 && (q[2] - z).abs() < 1e-3)
                .map(|q| q[1])
        };

        let side = fine.vertices();
        let at = |i: u32, j: u32| fine_data.positions[(j * side + i) as usize];
        let (mut shared, mut between) = (0, 0);
        for j in 0..side {
            for i in 0..side {
                if i != 0 && i != side - 1 && j != 0 && j != side - 1 {
                    continue;
                }
                let p = at(i, j);
                if let Some(height) = coarse_height(p[0], p[2]) {
                    assert!(
                        (height - p[1]).abs() < 1e-3,
                        "fine vertex {p:?} sits {height} away from the coarse surface",
                    );
                    shared += 1;
                    continue;
                }

                let along_z = i == 0 || i == side - 1;
                let varying = if along_z { j } else { i };
                assert!(
                    varying > 0 && varying < side - 1,
                    "a corner has no coarse counterpart at {p:?}",
                );
                let (a, b) = if along_z {
                    (at(i, j - 1), at(i, j + 1))
                } else {
                    (at(i - 1, j), at(i + 1, j))
                };
                let ends = [a, b].map(|end| {
                    coarse_height(end[0], end[2])
                        .unwrap_or_else(|| panic!("no coarse vertex beside {p:?} at {end:?}"))
                });
                let midpoint = [
                    0.5 * (a[0] + b[0]),
                    0.5 * (ends[0] + ends[1]),
                    0.5 * (a[2] + b[2]),
                ];
                let off = (0..3)
                    .map(|k| (p[k] - midpoint[k]).abs())
                    .fold(0.0f32, f32::max);
                assert!(
                    off < 1e-3,
                    "fine vertex {p:?} is {off} off the coarse segment through {midpoint:?}",
                );
                between += 1;
            }
        }
        assert!(shared > 0, "no shared vertex was compared");
        assert!(between > 0, "no averaged vertex was compared");
    }

    /// The vertices between two coarse ones are the average of them, so
    /// the fine edge is collinear with the coarse edge rather than
    /// following detail the coarse level stepped over.
    #[test]
    fn the_outer_ring_snaps_to_the_coarser_grid() {
        let map = hills(257);
        let levels = clipmap_levels(257, Vec2::splat(128.0));
        let fine = &levels[0];
        let data = build_clipmap_mesh_data(&map, fine, everywhere);
        let side = fine.vertices();

        let mut snapped = 0;
        for j in (1..side - 1).step_by(2) {
            let index = (j * side) as usize;
            let above = data.positions[((j - 1) * side) as usize][1];
            let below = data.positions[((j + 1) * side) as usize][1];
            assert!(
                (data.positions[index][1] - 0.5 * (above + below)).abs() < 1e-4,
                "the left edge vertex at row {j} did not snap",
            );
            snapped += 1;
        }
        assert!(snapped > 0);
    }

    /// A quad nothing authored is not drawn, and a level entirely over
    /// absent ground draws nothing at all.
    #[test]
    fn absent_ground_costs_no_triangles() {
        let map = ground(65);
        let levels = clipmap_levels(65, Vec2::splat(32.0));
        let all = build_clipmap_mesh_data(&map, &levels[0], everywhere);
        assert!(!all.indices.is_empty());

        let none = build_clipmap_mesh_data(&map, &levels[0], |_, _| false);
        assert!(none.indices.is_empty(), "absent ground still meshed");

        let half = build_clipmap_mesh_data(&map, &levels[0], |x, _| x < 32);
        assert!(half.indices.len() < all.indices.len());
        assert!(!half.indices.is_empty());
    }

    /// Nothing is drawn outside the terrain, however the level's square
    /// hangs over the edge.
    #[test]
    fn quads_outside_the_terrain_are_dropped() {
        let map = ground(65);
        let levels = clipmap_levels(65, Vec2::new(0.0, 0.0));
        let data = build_clipmap_mesh_data(&map, &levels[0], everywhere);
        for index in &data.indices {
            let p = data.positions[*index as usize];
            assert!(
                p[0] >= -32.001 && p[0] <= 32.001,
                "{p:?} is off the terrain"
            );
            assert!(
                p[2] >= -32.001 && p[2] <= 32.001,
                "{p:?} is off the terrain"
            );
        }
    }

    /// A ring whose square misses the terrain entirely draws nothing. The
    /// outermost level still covers the terrain, so it is never empty.
    #[test]
    fn a_ring_far_from_the_terrain_draws_nothing() {
        let map = ground(257);
        let levels = clipmap_levels(257, Vec2::splat(5000.0));
        let ring = build_clipmap_mesh_data(&map, &levels[0], everywhere);
        assert!(ring.indices.is_empty());
        let outer = build_clipmap_mesh_data(&map, levels.last().unwrap(), everywhere);
        assert!(!outer.indices.is_empty(), "the terrain is still drawn");
    }

    #[test]
    fn the_flat_shaded_surface_shares_no_vertices() {
        let map = hills(65);
        let levels = clipmap_levels(65, Vec2::splat(32.0));
        let smooth = build_clipmap_mesh_data(&map, &levels[0], everywhere);
        let flat = flat_shaded(smooth.clone());
        assert_eq!(flat.positions.len(), smooth.indices.len());
        assert_eq!(flat.grid.len(), flat.positions.len());
        let expected: Vec<u32> = (0..flat.positions.len() as u32).collect();
        assert_eq!(flat.indices, expected);
    }

    #[test]
    fn a_vertex_carries_the_grid_point_it_was_sampled_from() {
        let map = ground(65);
        let levels = clipmap_levels(65, Vec2::splat(32.0));
        let data = build_clipmap_mesh_data(&map, &levels[0], everywhere);
        assert_eq!(data.grid.len(), data.positions.len());
        assert_eq!(data.uvs.len(), data.positions.len());
        assert_eq!(data.normals.len(), data.positions.len());
        for (position, grid) in data.positions.iter().zip(&data.grid) {
            let expected = grid[0] as f32 - 32.0;
            assert!((position[0] - expected).abs() < 1e-3 || position[0] < -32.0);
        }
    }

    /// A zero-size terrain has no levels rather than a degenerate one.
    #[test]
    fn a_terrain_with_no_cells_has_no_levels() {
        assert!(clipmap_levels(1, Vec2::ZERO).is_empty());
        assert!(clipmap_levels(0, Vec2::ZERO).is_empty());
    }

    /// The indices the whole mesher emits are the ones the index-only
    /// path emits, so rewriting a level's triangles for a moved hole
    /// leaves exactly the surface a full remesh would have.
    #[test]
    fn the_index_only_build_matches_the_whole_mesher() {
        let map = hills(257);
        for viewer in [Vec2::splat(128.0), Vec2::new(30.0, 200.0), Vec2::ZERO] {
            for level in clipmap_levels(257, viewer) {
                let whole = build_clipmap_mesh_data(&map, &level, everywhere);
                assert_eq!(
                    build_clipmap_indices(257, &level, everywhere),
                    whole.indices,
                    "level {} disagreed with the whole mesher",
                    level.level,
                );
            }
        }
    }

    /// A level that only handed a different square to the finer level
    /// samples the same vertices, which is what lets it take a new index
    /// buffer instead of a new mesh.
    #[test]
    fn a_moved_hole_leaves_the_lattice_alone() {
        let map = hills(257);
        let mut level = clipmap_levels(257, Vec2::splat(128.0))[1];
        let before = build_clipmap_mesh_data(&map, &level, everywhere);

        let mut moved = level;
        moved.hole = Some(GridSquare {
            min: IVec2::splat(40),
            max: IVec2::splat(104),
        });
        assert!(level.same_lattice(&moved));
        let after = build_clipmap_mesh_data(&map, &moved, everywhere);
        assert_eq!(before.positions, after.positions);
        assert_eq!(before.normals, after.normals);
        assert_eq!(before.uvs, after.uvs);
        assert_eq!(before.grid, after.grid);
        assert_ne!(before.indices, after.indices, "the hole must matter");

        level.origin += IVec2::splat(2);
        assert!(
            !level.same_lattice(&moved),
            "a moved origin is a new lattice"
        );
    }

    /// Levels planned for a whole gesture, so the invariants below are
    /// checked against a run of frames rather than one.
    ///
    /// `heading` turns the dolly: one path shape can walk a clipmap past
    /// a whole class of boundary without ever landing on it.
    fn dolly(
        resolution: u32,
        frames: u32,
        speed: f32,
        heading: f32,
        budget: usize,
    ) -> (Vec<Vec<ClipmapLevel>>, usize) {
        let mut built: Vec<Option<ClipmapLevel>> = Vec::new();
        let mut history = Vec::new();
        let mut rebuilds = 0;
        let step = Vec2::new(heading.cos(), heading.sin()) * speed;
        let from = Vec2::splat(resolution as f32 * 0.5) - step * (frames as f32 * 0.5);
        for frame in 0..frames {
            let at = from + step * frame as f32;
            let targets = clipmap_levels(resolution, at);
            built.resize(targets.len(), None);
            let forced = vec![false; targets.len()];
            for (index, planned) in plan_rebuilds(&targets, &built, &forced, budget)
                .into_iter()
                .enumerate()
            {
                if let Some(level) = planned {
                    built[index] = Some(level);
                    rebuilds += 1;
                }
            }
            history.push(built.iter().map(|level| level.expect("built")).collect());
        }
        (history, rebuilds)
    }

    /// Every heading a dolly is checked along. Diagonals and shallow
    /// angles put the levels' corners and sides against each other in
    /// different orders. Sixteen steps of an eighth-turn cover the
    /// circle, offset so no heading lands on an axis.
    fn headings() -> Vec<f32> {
        (0..16)
            .map(|i| i as f32 * std::f32::consts::FRAC_PI_8 + 0.13)
            .collect()
    }

    /// What every frame of a planned run has to be able to say for
    /// itself, whatever the budget left behind.
    fn assert_seam_holds(frame: &[ClipmapLevel], resolution: u32) {
        assert_eq!(frame[0].hole, None, "the finest level cuts nothing out");
        let terrain = frame.last().expect("a terrain has levels").square();
        for pair in frame.windows(2) {
            let (fine, coarse) = (pair[0], pair[1]);
            assert_eq!(
                coarse.hole,
                Some(fine.square()),
                "level {} cut out something other than level {}",
                coarse.level,
                fine.level,
            );
            assert!(
                coarse
                    .square()
                    .holds_clear_of(&fine.square(), &terrain, coarse.step as i32),
                "level {} sits flush inside level {} at {:?} against {:?}",
                fine.level,
                coarse.level,
                fine.square(),
                coarse.square(),
            );
        }
        let last = (resolution - 1) as i32;
        assert!(terrain.min.x <= 0 && terrain.min.y <= 0);
        assert!(terrain.max.x >= last && terrain.max.y >= last);
    }

    /// The join between two levels never opens, however far behind the
    /// budget leaves a level: what a level cuts out of itself is the
    /// square the level inside it is drawing this frame, and it holds
    /// that square with a step of its own grid to spare rather than
    /// merely containing it.
    #[test]
    fn a_budget_never_parts_a_level_from_the_one_inside_it() {
        for resolution in [257, 513, 2049] {
            for heading in headings() {
                // Far faster than a person can scroll, so every level's
                // snap period is crossed on every frame of it.
                let (frames, _) = dolly(resolution, 60, 37.0, heading, REBUILD_BUDGET);
                for frame in &frames {
                    assert_seam_holds(frame, resolution);
                }
            }
        }
    }

    /// A slow dolly, a few cells a frame, walks the fine square out to
    /// its coarse neighbour's edge one step at a time rather than jumping
    /// past it.
    #[test]
    fn a_slow_dolly_never_walks_a_level_flush_against_its_neighbour() {
        for resolution in [513, 1025, 2049] {
            for heading in headings() {
                let (frames, _) = dolly(resolution, 400, 3.0, heading, REBUILD_BUDGET);
                for frame in &frames {
                    assert_seam_holds(frame, resolution);
                }
            }
        }
    }

    /// Every terrain cell the whole clipmap draws, and every cell two
    /// levels both draw.
    ///
    /// Read back from the mesher's own triangles rather than from a
    /// second copy of the quad rules. A triangle's grid corners span the
    /// quad it came from, and both triangles of a quad span the same
    /// cells.
    fn coverage(map: &Heightmap, frame: &[ClipmapLevel]) -> (usize, usize) {
        let side = map.resolution.saturating_sub(1) as usize;
        let mut drawn_by = vec![-1i32; side * side];
        let mut overlaps = 0;
        for level in frame {
            let data = build_clipmap_mesh_data(map, level, everywhere);
            for triangle in data.indices.chunks_exact(3) {
                let corners: Vec<[u32; 2]> = triangle
                    .iter()
                    .map(|index| data.grid[*index as usize])
                    .collect();
                let (x0, x1) = (
                    corners.iter().map(|c| c[0]).min().expect("three corners"),
                    corners.iter().map(|c| c[0]).max().expect("three corners"),
                );
                let (z0, z1) = (
                    corners.iter().map(|c| c[1]).min().expect("three corners"),
                    corners.iter().map(|c| c[1]).max().expect("three corners"),
                );
                for z in z0..z1 {
                    for x in x0..x1 {
                        let slot = &mut drawn_by[z as usize * side + x as usize];
                        if *slot >= 0 && *slot != level.level as i32 {
                            overlaps += 1;
                        }
                        *slot = level.level as i32;
                    }
                }
            }
        }
        (
            drawn_by.iter().filter(|level| **level >= 0).count(),
            overlaps,
        )
    }

    /// Plan a run and return what every level ends up drawing.
    fn run_to(
        resolution: u32,
        frames: u32,
        stride: Vec2,
        from: Vec2,
        budget: usize,
    ) -> (Vec<Option<ClipmapLevel>>, Vec2) {
        let mut built: Vec<Option<ClipmapLevel>> = Vec::new();
        let mut at = from;
        for _ in 0..frames {
            at += stride;
            let targets = clipmap_levels(resolution, at);
            built.resize(targets.len(), None);
            let forced = vec![false; targets.len()];
            for (index, planned) in plan_rebuilds(&targets, &built, &forced, budget)
                .into_iter()
                .enumerate()
            {
                if let Some(level) = planned {
                    built[index] = Some(level);
                }
            }
        }
        (built, at)
    }

    /// An edit remeshes a level where it stands rather than moving it.
    ///
    /// A stroke marks only the levels whose ground it landed on, which
    /// outside the fine squares is the coarse ones. A level that took
    /// that as licence to jump to its target would leave the lagging
    /// level inside it drawing the same ground.
    #[test]
    fn an_edit_never_lets_a_level_jump_past_a_lagging_one() {
        let resolution = 1025;
        let map = ground(resolution);
        // Off the terrain footprint, where the fine squares draw nothing
        // and the budget can leave them a long way behind.
        let (mut built, at) = run_to(
            resolution,
            200,
            Vec2::new(11.0, 7.0),
            Vec2::splat(512.0),
            REBUILD_BUDGET,
        );
        // One more step, so the finest level spends the budget and the
        // level behind it has to stay where it is.
        let targets = clipmap_levels(resolution, at + Vec2::new(11.0, 7.0));
        assert!(targets.len() >= 4, "this terrain needs four levels");
        let forced: Vec<bool> = (0..targets.len()).map(|index| index >= 2).collect();

        for (index, planned) in plan_rebuilds(&targets, &built, &forced, REBUILD_BUDGET)
            .into_iter()
            .enumerate()
        {
            if let Some(level) = planned {
                built[index] = Some(level);
            }
        }
        let frame: Vec<ClipmapLevel> = built.iter().map(|level| level.expect("built")).collect();
        let (_, overlaps) = coverage(&map, &frame);
        assert_eq!(overlaps, 0, "two levels drew the same ground after an edit");
        assert_seam_holds(&frame, resolution);
    }

    /// Whatever the budget leaves behind, every cell of the terrain is
    /// drawn by exactly one level: none twice, and none by nobody.
    #[test]
    fn a_budgeted_clipmap_draws_every_cell_exactly_once() {
        let resolution = 513;
        let map = ground(resolution);
        let side = (resolution - 1) as usize;
        for stride in [
            Vec2::new(3.0, 0.0),
            Vec2::new(3.0, 2.0),
            Vec2::new(-2.0, 5.0),
            Vec2::new(9.0, -9.0),
        ] {
            for frames in [17, 64, 150] {
                let (built, _) = run_to(
                    resolution,
                    frames,
                    stride,
                    Vec2::splat(256.0),
                    REBUILD_BUDGET,
                );
                let frame: Vec<ClipmapLevel> =
                    built.iter().map(|level| level.expect("built")).collect();
                assert_seam_holds(&frame, resolution);
                let (drawn, overlaps) = coverage(&map, &frame);
                assert_eq!(overlaps, 0, "two levels drew the same ground");
                assert_eq!(drawn, side * side, "ground was left undrawn");
            }
        }
    }

    /// The budget is what a gesture pays for: without it every level
    /// re-snaps on every frame, and the coarse ones are the expensive
    /// ones.
    #[test]
    fn a_budget_moves_fewer_levels_than_an_unbudgeted_frame() {
        // A brisk zoom-out: several grid steps a frame, which re-snaps
        // every level of an unbudgeted clipmap on every frame of it.
        let (_, unbudgeted) = dolly(2049, 60, 9.0, 0.6, usize::MAX);
        let (_, budgeted) = dolly(2049, 60, 9.0, 0.6, REBUILD_BUDGET);
        assert!(
            budgeted * 3 < unbudgeted * 2,
            "budgeting saved too little: {budgeted} against {unbudgeted}",
        );
    }

    /// A level whose ground was edited is rebuilt whatever the budget
    /// says, and spends none of it: the budget rations following the
    /// camera, not remeshing changed heights.
    #[test]
    fn edited_levels_are_rebuilt_past_the_budget() {
        let targets = clipmap_levels(2049, Vec2::splat(1024.0));
        let built: Vec<Option<ClipmapLevel>> = targets.iter().copied().map(Some).collect();
        let forced = vec![true; targets.len()];
        let plan = plan_rebuilds(&targets, &built, &forced, 0);
        assert_eq!(plan.len(), targets.len());
        for (index, planned) in plan.iter().enumerate() {
            assert_eq!(
                *planned,
                Some(targets[index]),
                "level {index} was not remeshed",
            );
        }
    }

    /// A level with no mesh yet is built at once: there is nothing on
    /// screen for it to keep.
    #[test]
    fn a_level_with_nothing_built_is_never_deferred() {
        let targets = clipmap_levels(2049, Vec2::splat(1024.0));
        let built = vec![None; targets.len()];
        let forced = vec![false; targets.len()];
        let plan = plan_rebuilds(&targets, &built, &forced, 0);
        for (index, planned) in plan.iter().enumerate() {
            assert!(planned.is_some(), "level {index} was left unbuilt");
        }
    }

    #[test]
    fn a_still_camera_plans_nothing() {
        let targets = clipmap_levels(1025, Vec2::splat(512.0));
        let mut built: Vec<Option<ClipmapLevel>> = targets.iter().copied().map(Some).collect();
        // The finest level's hole is `None`, and every coarser level's is
        // the square the finer one draws, which is what the first frame
        // establishes.
        for index in 1..built.len() {
            let inner = built[index - 1].expect("built").square();
            built[index] = built[index].map(|mut level| {
                level.hole = Some(inner);
                level
            });
        }
        let forced = vec![false; targets.len()];
        assert!(
            plan_rebuilds(&targets, &built, &forced, REBUILD_BUDGET)
                .iter()
                .all(Option::is_none),
        );
    }

    /// Vertices are placed where the map says its grid sits, not on an
    /// assumption that it is centred: a terrain sculpted outwards starts
    /// wherever its first region does, and a mesh placed elsewhere
    /// disagrees with the region overlay and the picker.
    #[test]
    fn vertices_land_where_the_map_says_its_grid_sits() {
        let origin = Vec2::new(-512.0, 100.0);
        let map = Heightmap::new_at(9, Vec2::splat(8.0), 4.0, origin);
        let levels = clipmap_levels(map.resolution, Vec2::splat(4.0));
        let built = build_clipmap_mesh_data(&map, &levels[0], |_, _| true);

        let first = built.positions[0];
        assert_eq!(
            Vec2::new(first[0], first[2]),
            origin,
            "the first vertex is the grid's own corner",
        );

        // And the far corner is one cell size short of a full span away.
        let far = built
            .positions
            .iter()
            .fold(Vec2::splat(f32::MIN), |acc, p| {
                acc.max(Vec2::new(p[0], p[2]))
            });
        assert_eq!(far, origin + map.size);
    }
}
