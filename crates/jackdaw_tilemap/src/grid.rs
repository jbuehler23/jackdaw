//! Square grid math. Every world/cell/chunk conversion lives here so other
//! grid shapes only need to touch this module.

use core::cmp::Ordering;

use glam::{IVec2, UVec2, Vec2};

/// Cells per chunk side.
pub const CHUNK_SIZE: i32 = 32;
/// Cells per chunk.
pub const CHUNK_AREA: usize = (CHUNK_SIZE * CHUNK_SIZE) as usize;

/// Integer cell coordinate. Ordered by row (y) then column (x).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct CellCoord(pub IVec2);

/// Integer chunk coordinate, in chunk units. Ordered by row (y) then column (x).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ChunkCoord(pub IVec2);

/// Half-open cell rectangle: `min` inclusive, `max` exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellRect {
    pub min: IVec2,
    pub max: IVec2,
}

/// Axis-aligned world rectangle, `min` to `max`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldRect {
    pub min: Vec2,
    pub max: Vec2,
}

/// A square grid whose cells are `cell` world units wide and tall.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub cell: UVec2,
}

impl Grid {
    pub fn new(cell: UVec2) -> Self {
        Self {
            cell: cell.max(UVec2::ONE),
        }
    }

    pub fn world_to_cell(&self, world: Vec2) -> CellCoord {
        CellCoord((world / self.cell.as_vec2()).floor().as_ivec2())
    }

    pub fn cell_min(&self, cell: CellCoord) -> Vec2 {
        cell.0.as_vec2() * self.cell.as_vec2()
    }

    pub fn cell_center(&self, cell: CellCoord) -> Vec2 {
        self.cell_min(cell) + self.cell.as_vec2() * 0.5
    }

    pub fn cell_rect(&self, cell: CellCoord) -> WorldRect {
        let min = self.cell_min(cell);
        WorldRect {
            min,
            max: min + self.cell.as_vec2(),
        }
    }
}

impl CellCoord {
    pub const fn new(x: i32, y: i32) -> Self {
        Self(IVec2::new(x, y))
    }

    pub fn chunk(self) -> ChunkCoord {
        ChunkCoord(self.0.div_euclid(IVec2::splat(CHUNK_SIZE)))
    }

    /// Index of this cell inside its chunk: `y * CHUNK_SIZE + x` in local coordinates.
    pub fn local_index(self) -> usize {
        let local = self.0.rem_euclid(IVec2::splat(CHUNK_SIZE));
        (local.y * CHUNK_SIZE + local.x) as usize
    }

    pub fn to_chunk_local(self) -> (ChunkCoord, usize) {
        (self.chunk(), self.local_index())
    }
}

impl ChunkCoord {
    pub const fn new(x: i32, y: i32) -> Self {
        Self(IVec2::new(x, y))
    }

    pub fn min_cell(self) -> CellCoord {
        CellCoord(self.0 * CHUNK_SIZE)
    }

    /// Inverse of [`CellCoord::to_chunk_local`]. Panics if `local >= CHUNK_AREA`.
    pub fn cell(self, local: usize) -> CellCoord {
        assert!(local < CHUNK_AREA, "local index {local} out of range");
        let local = local as i32;
        CellCoord(self.min_cell().0 + IVec2::new(local % CHUNK_SIZE, local / CHUNK_SIZE))
    }
}

impl CellRect {
    pub fn size(&self) -> UVec2 {
        self.max
            .saturating_sub(self.min)
            .max(IVec2::ZERO)
            .as_uvec2()
    }

    pub fn contains(&self, cell: CellCoord) -> bool {
        cell.0.cmpge(self.min).all() && cell.0.cmplt(self.max).all()
    }
}

fn row_major(a: IVec2, b: IVec2) -> Ordering {
    a.y.cmp(&b.y).then(a.x.cmp(&b.x))
}

impl Ord for CellCoord {
    fn cmp(&self, other: &Self) -> Ordering {
        row_major(self.0, other.0)
    }
}

impl PartialOrd for CellCoord {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ChunkCoord {
    fn cmp(&self, other: &Self) -> Ordering {
        row_major(self.0, other.0)
    }
}

impl PartialOrd for ChunkCoord {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const G16: Grid = Grid {
        cell: UVec2::new(16, 16),
    };

    fn cell_x(world_x: f32) -> i32 {
        G16.world_to_cell(Vec2::new(world_x, 0.0)).0.x
    }

    #[test]
    fn grid_new_clamps_zero_cell_axes() {
        assert_eq!(Grid::new(UVec2::new(0, 16)).cell, UVec2::new(1, 16));
        assert_eq!(Grid::new(UVec2::ZERO).cell, UVec2::ONE);
    }

    #[test]
    fn inverted_rect_has_zero_size() {
        let r = CellRect {
            min: IVec2::new(5, 0),
            max: IVec2::new(2, 3),
        };
        assert_eq!(r.size(), UVec2::new(0, 3));
        let wide = CellRect {
            min: IVec2::splat(i32::MIN),
            max: IVec2::splat(i32::MAX),
        };
        assert_eq!(wide.size(), UVec2::splat(i32::MAX as u32));
    }

    #[test]
    fn world_to_cell_floors_negatives() {
        assert_eq!(cell_x(-0.0001), -1);
        assert_eq!(cell_x(-0.1), -1);
        assert_eq!(cell_x(-16.0), -1);
        assert_eq!(cell_x(-16.1), -2);
        assert_eq!(cell_x(15.999), 0);
    }

    #[test]
    fn boundary_belongs_to_cell_with_that_min_corner() {
        assert_eq!(G16.world_to_cell(Vec2::ZERO), CellCoord::new(0, 0));
        assert_eq!(
            G16.world_to_cell(Vec2::new(16.0, 16.0)),
            CellCoord::new(1, 1)
        );
        assert_eq!(
            G16.world_to_cell(Vec2::new(-32.0, 32.0)),
            CellCoord::new(-2, 2)
        );
    }

    #[test]
    fn non_square_cells_convert_per_axis() {
        let grid = Grid::new(UVec2::new(8, 4));
        assert_eq!(
            grid.world_to_cell(Vec2::new(9.0, -1.0)),
            CellCoord::new(1, -1)
        );
        assert_eq!(grid.cell_min(CellCoord::new(1, -1)), Vec2::new(8.0, -4.0));
        assert_eq!(
            grid.cell_center(CellCoord::new(1, -1)),
            Vec2::new(12.0, -2.0)
        );
    }

    #[test]
    fn cell_min_center_and_rect_are_y_up_from_origin() {
        let c = CellCoord::new(-1, 2);
        assert_eq!(G16.cell_min(c), Vec2::new(-16.0, 32.0));
        assert_eq!(G16.cell_center(c), Vec2::new(-8.0, 40.0));
        assert_eq!(
            G16.cell_rect(c),
            WorldRect {
                min: Vec2::new(-16.0, 32.0),
                max: Vec2::new(0.0, 48.0),
            }
        );
    }

    #[test]
    fn cell_min_and_center_round_trip_through_world_to_cell() {
        for x in -40..40 {
            for y in [-33, -1, 0, 31, 32] {
                let c = CellCoord::new(x, y);
                assert_eq!(G16.world_to_cell(G16.cell_min(c)), c);
                assert_eq!(G16.world_to_cell(G16.cell_center(c)), c);
            }
        }
    }

    #[test]
    fn negative_cells_map_to_euclidean_chunks() {
        assert_eq!(
            CellCoord::new(-1, 0).to_chunk_local(),
            (ChunkCoord::new(-1, 0), 31)
        );
        assert_eq!(
            CellCoord::new(-32, 0).to_chunk_local(),
            (ChunkCoord::new(-1, 0), 0)
        );
        assert_eq!(
            CellCoord::new(-33, 0).to_chunk_local(),
            (ChunkCoord::new(-2, 0), 31)
        );
        assert_eq!(
            CellCoord::new(31, 0).to_chunk_local(),
            (ChunkCoord::new(0, 0), 31)
        );
        assert_eq!(
            CellCoord::new(32, 0).to_chunk_local(),
            (ChunkCoord::new(1, 0), 0)
        );
    }

    #[test]
    fn local_index_is_row_major() {
        assert_eq!(CellCoord::new(0, 1).local_index(), 32);
        assert_eq!(CellCoord::new(3, 2).local_index(), 2 * 32 + 3);
        assert_eq!(CellCoord::new(-1, -1).local_index(), 31 * 32 + 31);
        assert_eq!(ChunkCoord::new(-1, 2).min_cell(), CellCoord::new(-32, 64));
    }

    #[test]
    fn chunk_local_inverse_round_trips() {
        for x in -70..70 {
            for y in [-65, -33, -32, -1, 0, 31, 32, 64] {
                let c = CellCoord::new(x, y);
                let (chunk, local) = c.to_chunk_local();
                assert!(local < CHUNK_AREA);
                assert_eq!(chunk.cell(local), c);
            }
        }
    }

    #[test]
    fn coords_order_row_major() {
        let mut v = vec![
            ChunkCoord::new(1, 0),
            ChunkCoord::new(0, 1),
            ChunkCoord::new(-1, 0),
            ChunkCoord::new(5, -1),
        ];
        v.sort();
        assert_eq!(
            v,
            vec![
                ChunkCoord::new(5, -1),
                ChunkCoord::new(-1, 0),
                ChunkCoord::new(1, 0),
                ChunkCoord::new(0, 1),
            ]
        );
    }

    #[test]
    fn cell_rect_contains_is_half_open() {
        let r = CellRect {
            min: IVec2::new(-1, -1),
            max: IVec2::new(2, 1),
        };
        assert_eq!(r.size(), UVec2::new(3, 2));
        assert!(r.contains(CellCoord::new(-1, -1)));
        assert!(r.contains(CellCoord::new(1, 0)));
        assert!(!r.contains(CellCoord::new(2, 0)));
        assert!(!r.contains(CellCoord::new(0, 1)));
    }
}
