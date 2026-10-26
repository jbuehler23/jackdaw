//! Sparse chunked cell storage. Only chunks holding at least one non-empty
//! cell are stored; an absent chunk reads as all empty. Chunks are kept in
//! row-major [`ChunkCoord`] order so iteration is deterministic.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use glam::IVec2;

use crate::grid::{CHUNK_AREA, CellCoord, CellRect, ChunkCoord};

/// A value stored per cell, with a designated empty value.
pub trait CellValue: Copy + PartialEq + core::fmt::Debug {
    const EMPTY: Self;

    fn is_empty(&self) -> bool {
        *self == Self::EMPTY
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Chunk<T> {
    cells: Box<[T; CHUNK_AREA]>,
    filled: usize,
}

/// An unbounded layer of cells stored as sparse 32x32 chunks.
#[derive(Clone, Debug)]
pub struct ChunkedLayer<T: CellValue> {
    chunks: BTreeMap<ChunkCoord, Chunk<T>>,
    dirty: BTreeSet<ChunkCoord>,
}

/// Compares stored cells only; pending dirty chunks are not part of the value.
impl<T: CellValue> PartialEq for ChunkedLayer<T> {
    fn eq(&self, other: &Self) -> bool {
        self.chunks == other.chunks
    }
}

impl<T: CellValue> Default for ChunkedLayer<T> {
    fn default() -> Self {
        Self {
            chunks: BTreeMap::new(),
            dirty: BTreeSet::new(),
        }
    }
}

impl<T: CellValue> ChunkedLayer<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, cell: CellCoord) -> T {
        let (coord, local) = cell.to_chunk_local();
        self.chunks
            .get(&coord)
            .map_or(T::EMPTY, |chunk| chunk.cells[local])
    }

    /// Writes `value` and returns the previous value. Writing the empty value
    /// erases. A chunk is marked dirty only when a cell actually changes.
    pub fn set(&mut self, cell: CellCoord, value: T) -> T {
        let (coord, local) = cell.to_chunk_local();
        let chunk = match self.chunks.entry(coord) {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                if value.is_empty() {
                    return T::EMPTY;
                }
                entry.insert(Chunk {
                    cells: Box::new([T::EMPTY; CHUNK_AREA]),
                    filled: 0,
                })
            }
        };
        let previous = chunk.cells[local];
        if previous == value {
            return previous;
        }
        chunk.cells[local] = value;
        if previous.is_empty() {
            chunk.filled += 1;
        } else if value.is_empty() {
            chunk.filled -= 1;
        }
        if chunk.filled == 0 {
            self.chunks.remove(&coord);
        }
        self.dirty.insert(coord);
        previous
    }

    /// Installs a whole chunk without marking it dirty. An all-empty chunk is ignored.
    pub(crate) fn insert_chunk(&mut self, coord: ChunkCoord, cells: Box<[T; CHUNK_AREA]>) {
        let filled = cells.iter().filter(|value| !value.is_empty()).count();
        if filled > 0 {
            self.chunks.insert(coord, Chunk { cells, filled });
        }
    }

    pub fn clear(&mut self, cell: CellCoord) -> T {
        self.set(cell, T::EMPTY)
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }

    /// Cells of a stored chunk, indexed by [`CellCoord::local_index`].
    pub fn chunk(&self, coord: ChunkCoord) -> Option<&[T; CHUNK_AREA]> {
        self.chunks.get(&coord).map(|chunk| &*chunk.cells)
    }

    /// Non-empty chunks in row-major chunk order.
    pub fn chunks(&self) -> impl Iterator<Item = (ChunkCoord, &[T; CHUNK_AREA])> {
        self.chunks
            .iter()
            .map(|(coord, chunk)| (*coord, &*chunk.cells))
    }

    /// Non-empty cells, grouped by chunk in row-major chunk order.
    pub fn cells(&self) -> impl Iterator<Item = (CellCoord, T)> + '_ {
        self.chunks().flat_map(|(coord, cells)| {
            cells
                .iter()
                .enumerate()
                .filter(|(_, value)| !value.is_empty())
                .map(move |(local, value)| (coord.cell(local), *value))
        })
    }

    /// Drains the chunks changed since the last call, in sorted order.
    /// Includes chunks that were removed because they became empty.
    pub fn take_dirty(&mut self) -> Vec<ChunkCoord> {
        core::mem::take(&mut self.dirty).into_iter().collect()
    }

    /// Bounds of the non-empty cells, or `None` when the layer is empty.
    pub fn painted_bounds(&self) -> Option<CellRect> {
        self.cells().fold(None, |bounds, (cell, _)| {
            let (min, max) = match bounds {
                Some(CellRect { min, max }) => {
                    (min.min(cell.0), max.max(cell.0.saturating_add(IVec2::ONE)))
                }
                None => (cell.0, cell.0.saturating_add(IVec2::ONE)),
            };
            Some(CellRect { min, max })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tile::{TerrainId, TileRef};

    fn c(x: i32, y: i32) -> CellCoord {
        CellCoord::new(x, y)
    }

    fn rect(min: (i32, i32), max: (i32, i32)) -> CellRect {
        CellRect {
            min: IVec2::new(min.0, min.1),
            max: IVec2::new(max.0, max.1),
        }
    }

    #[test]
    fn unset_cells_read_empty() {
        let layer = ChunkedLayer::<TileRef>::new();
        assert_eq!(layer.get(c(5, -9)), TileRef::EMPTY);
        assert!(layer.is_empty());
        assert_eq!(layer.painted_bounds(), None);
    }

    #[test]
    fn set_get_and_previous_value() {
        let mut layer = ChunkedLayer::<TileRef>::new();
        assert_eq!(layer.set(c(-33, 4), TileRef::new(2)), TileRef::EMPTY);
        assert_eq!(layer.get(c(-33, 4)), TileRef::new(2));
        assert_eq!(layer.set(c(-33, 4), TileRef::new(3)), TileRef::new(2));
        assert_eq!(layer.get(c(-33, 4)), TileRef::new(3));
        assert_eq!(layer.get(c(-32, 4)), TileRef::EMPTY);
        assert_eq!(layer.chunk_count(), 1);
        let chunk = layer.chunk(ChunkCoord::new(-2, 0)).expect("chunk stored");
        assert_eq!(chunk[4 * 32 + 31], TileRef::new(3));
    }

    #[test]
    fn erasing_last_cell_removes_chunk() {
        let mut layer = ChunkedLayer::<TerrainId>::new();
        layer.set(c(1, 1), 4);
        layer.set(c(2, 1), 4);
        layer.clear(c(1, 1));
        assert_eq!(layer.chunk_count(), 1);
        layer.clear(c(2, 1));
        assert_eq!(layer.chunk_count(), 0);
        assert!(layer.is_empty());
        assert!(layer.chunk(ChunkCoord::new(0, 0)).is_none());
        assert_eq!(layer.chunks().count(), 0);
    }

    #[test]
    fn setting_empty_value_equals_erase() {
        let mut layer = ChunkedLayer::<TileRef>::new();
        layer.set(c(0, 0), TileRef::new(1));
        assert_eq!(layer.set(c(0, 0), TileRef::EMPTY), TileRef::new(1));
        assert!(layer.is_empty());
        assert_eq!(layer.clear(c(9, 9)), TileRef::EMPTY);
        assert!(layer.is_empty());
    }

    #[test]
    fn chunks_iterate_in_row_major_order() {
        let mut layer = ChunkedLayer::<TerrainId>::new();
        for (x, y) in [(40, 40), (-1, 0), (0, -1), (33, 0)] {
            layer.set(c(x, y), 1);
        }
        let coords: Vec<_> = layer.chunks().map(|(coord, _)| coord).collect();
        assert_eq!(
            coords,
            vec![
                ChunkCoord::new(0, -1),
                ChunkCoord::new(-1, 0),
                ChunkCoord::new(1, 0),
                ChunkCoord::new(1, 1),
            ]
        );
    }

    #[test]
    fn cells_yield_only_non_empty() {
        let mut layer = ChunkedLayer::<TerrainId>::new();
        layer.set(c(-1, -1), 2);
        layer.set(c(3, 0), 5);
        layer.set(c(4, 0), 6);
        layer.clear(c(4, 0));
        let cells: Vec<_> = layer.cells().collect();
        assert_eq!(cells, vec![(c(-1, -1), 2), (c(3, 0), 5)]);
    }

    #[test]
    fn dirty_tracking_drains_sorted_and_deduplicated() {
        let mut layer = ChunkedLayer::<TerrainId>::new();
        layer.set(c(100, 0), 1);
        layer.set(c(-1, 0), 1);
        layer.set(c(-2, 0), 1);
        assert_eq!(
            layer.take_dirty(),
            vec![ChunkCoord::new(-1, 0), ChunkCoord::new(3, 0)]
        );
        assert!(layer.take_dirty().is_empty());
    }

    #[test]
    fn unchanged_writes_do_not_mark_dirty() {
        let mut layer = ChunkedLayer::<TerrainId>::new();
        layer.set(c(0, 0), 1);
        layer.take_dirty();
        layer.set(c(0, 0), 1);
        layer.clear(c(50, 50));
        assert!(layer.take_dirty().is_empty());
    }

    #[test]
    fn removed_chunk_is_reported_dirty() {
        let mut layer = ChunkedLayer::<TerrainId>::new();
        layer.set(c(0, 0), 1);
        layer.take_dirty();
        layer.clear(c(0, 0));
        assert_eq!(layer.take_dirty(), vec![ChunkCoord::new(0, 0)]);
    }

    #[test]
    fn painted_bounds_are_half_open_over_live_cells() {
        let mut layer = ChunkedLayer::<TerrainId>::new();
        layer.set(c(0, 0), 1);
        assert_eq!(layer.painted_bounds(), Some(rect((0, 0), (1, 1))));
        layer.set(c(-3, 2), 1);
        layer.set(c(5, -4), 1);
        let bounds = layer.painted_bounds().expect("non-empty");
        assert_eq!(bounds, rect((-3, -4), (6, 3)));
        assert_eq!(bounds.size(), glam::UVec2::new(9, 7));
    }

    #[test]
    fn painted_bounds_saturate_at_max_coordinate() {
        let mut layer = ChunkedLayer::<TerrainId>::new();
        layer.set(c(i32::MAX, 0), 1);
        let bounds = layer.painted_bounds().expect("non-empty");
        assert_eq!(bounds.max.x, i32::MAX);
        assert_eq!(bounds.min.x, i32::MAX);
    }

    #[test]
    fn layers_clone_and_compare() {
        let mut layer = ChunkedLayer::<TerrainId>::new();
        layer.set(c(1, 1), 2);
        let copy = layer.clone();
        assert_eq!(layer, copy);
        layer.set(c(1, 1), 3);
        assert_ne!(layer, copy);
    }

    #[test]
    fn painted_bounds_shrink_on_erase() {
        let mut layer = ChunkedLayer::<TerrainId>::new();
        layer.set(c(-3, 2), 1);
        layer.set(c(5, -4), 1);
        layer.set(c(70, 70), 1);
        layer.clear(c(70, 70));
        layer.clear(c(5, -4));
        assert_eq!(layer.painted_bounds(), Some(rect((-3, 2), (-2, 3))));
        layer.clear(c(-3, 2));
        assert_eq!(layer.painted_bounds(), None);
    }

    #[test]
    fn equality_ignores_pending_dirty_chunks() {
        let mut a = ChunkedLayer::<u8>::new();
        a.set(CellCoord::new(3, 4), 2);
        let mut b = a.clone();
        b.take_dirty();
        assert_eq!(a, b);
    }

    #[test]
    fn insert_chunk_is_clean_and_ignores_empty() {
        let mut cells = Box::new([0u8; CHUNK_AREA]);
        cells[5] = 3;
        cells[6] = 4;
        let mut layer = ChunkedLayer::<u8>::new();
        layer.insert_chunk(ChunkCoord::new(1, 0), cells);
        layer.insert_chunk(ChunkCoord::new(2, 0), Box::new([0u8; CHUNK_AREA]));
        assert_eq!(layer.chunk_count(), 1);
        assert_eq!(layer.get(c(32 + 5, 0)), 3);
        assert!(layer.take_dirty().is_empty());
        layer.clear(c(32 + 5, 0));
        layer.clear(c(32 + 6, 0));
        assert!(layer.is_empty());
    }
}
