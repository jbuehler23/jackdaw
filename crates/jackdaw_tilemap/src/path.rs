//! Per-cell link data written by Path strokes, and the stroke operations.
//!
//! A Path stroke joins only consecutive cells of the stroke, so two parallel
//! roads of one terrain stay apart. The links are stored per cell so every
//! later resolve keeps them. A Connect stroke clears the links of the cells it
//! paints, which then join neighbours by terrain alone. Links only affect
//! sets that match sides; a `Corners` set ignores them.

use crate::{CellCoord, CellValue, ChunkedLayer, Peer, TerrainId};

/// Link bits of one authoring cell: a bit per side, plus [`Links::PATH`].
///
/// | bit | meaning                               |
/// |-----|---------------------------------------|
/// | 0   | joined across the top side            |
/// | 1   | joined across the left side           |
/// | 2   | joined across the right side          |
/// | 3   | joined across the bottom side         |
/// | 4   | the cell was painted by a Path stroke |
/// | 5-7 | reserved, always 0                    |
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Links(u8);

impl CellValue for Links {
    const EMPTY: Self = Links(0);
}

impl Links {
    pub const EMPTY: Self = Links(0);
    pub const TOP: u8 = 1 << 0;
    pub const LEFT: u8 = 1 << 1;
    pub const RIGHT: u8 = 1 << 2;
    pub const BOTTOM: u8 = 1 << 3;
    pub const PATH: u8 = 1 << 4;
    pub const MASK: u8 = Self::TOP | Self::LEFT | Self::RIGHT | Self::BOTTOM | Self::PATH;

    /// The links held in `bits`, or `None` when a reserved bit is set.
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::MASK != 0 {
            return None;
        }
        Some(Links(bits))
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    /// The bit for a side peer; 0 for a corner or the centre.
    pub const fn side_bit(side: Peer) -> u8 {
        match side {
            Peer::Top => Self::TOP,
            Peer::Left => Self::LEFT,
            Peer::Right => Self::RIGHT,
            Peer::Bottom => Self::BOTTOM,
            Peer::TopLeft
            | Peer::TopRight
            | Peer::Centre
            | Peer::BottomLeft
            | Peer::BottomRight => 0,
        }
    }

    pub const fn has(self, side: Peer) -> bool {
        let bit = Self::side_bit(side);
        bit != 0 && self.0 & bit != 0
    }

    /// Adds the link across `side`; a corner or the centre leaves it unchanged.
    pub const fn with(self, side: Peer) -> Self {
        Links(self.0 | Self::side_bit(side))
    }

    pub const fn with_path(self) -> Self {
        Links(self.0 | Self::PATH)
    }

    pub const fn is_path(self) -> bool {
        self.0 & Self::PATH != 0
    }
}

/// Whether cell `a` joins its neighbour `b` of the same terrain across `side`
/// of `a`: Connect cells always join; once either is a Path cell, both must
/// mark the shared side.
pub(crate) const fn links_join(a: Links, b: Links, side: Peer) -> bool {
    if a.is_path() || b.is_path() {
        a.has(side) && b.has(side.opposite())
    } else {
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrokeError {
    /// Path strokes paint a terrain; erasing is a Connect stroke with id 0.
    EmptyTerrain,
    /// `cells[index]` is not an orthogonal neighbour of `cells[index - 1]`.
    NotContiguous { index: usize },
}

impl core::fmt::Display for StrokeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyTerrain => write!(f, "a path stroke needs a terrain"),
            Self::NotContiguous { index } => {
                write!(
                    f,
                    "path stroke cell {index} does not touch the previous cell"
                )
            }
        }
    }
}

impl std::error::Error for StrokeError {}

fn side_towards(from: CellCoord, to: CellCoord) -> Option<Peer> {
    Peer::SIDES
        .into_iter()
        .find(|side| from.0.checked_add(side.offset()) == Some(to.0))
}

fn sorted(cells: &[CellCoord]) -> Vec<CellCoord> {
    let mut dirty = cells.to_vec();
    dirty.sort_unstable();
    dirty.dedup();
    dirty
}

/// Paints `id` along `cells` as a Path stroke, linking consecutive cells. A
/// cell keeps links from earlier strokes only when its terrain is already `id`.
/// Returns the painted cells, sorted, for [`crate::resolve_region`]. On error
/// nothing is written. A `Corners` set ignores links, so it draws the stroke
/// as Connect would. [`orthogonal_line`] turns sampled points into a valid stroke.
pub fn apply_path_stroke(
    terrain: &mut ChunkedLayer<TerrainId>,
    links: &mut ChunkedLayer<Links>,
    cells: &[CellCoord],
    id: TerrainId,
) -> Result<Vec<CellCoord>, StrokeError> {
    if id == 0 {
        return Err(StrokeError::EmptyTerrain);
    }
    let mut steps = Vec::with_capacity(cells.len().saturating_sub(1));
    for (i, pair) in cells.windows(2).enumerate() {
        let side =
            side_towards(pair[0], pair[1]).ok_or(StrokeError::NotContiguous { index: i + 1 })?;
        steps.push(side);
    }
    for &cell in cells {
        let kept = if terrain.set(cell, id) == id {
            links.get(cell)
        } else {
            Links::EMPTY
        };
        links.set(cell, kept.with_path());
    }
    for (pair, side) in cells.windows(2).zip(steps) {
        links.set(pair[0], links.get(pair[0]).with(side));
        links.set(pair[1], links.get(pair[1]).with(side.opposite()));
    }
    Ok(sorted(cells))
}

/// The 4-connected cells from `from` to `to`, both included, stepping one
/// axis at a time along the straight line between them (x first on a tie).
pub fn orthogonal_line(from: CellCoord, to: CellCoord) -> Vec<CellCoord> {
    let (x0, y0) = (i64::from(from.0.x), i64::from(from.0.y));
    let (dx, dy) = (i64::from(to.0.x) - x0, i64::from(to.0.y) - y0);
    let (nx, ny) = (dx.unsigned_abs(), dy.unsigned_abs());
    let (mut ix, mut iy) = (0u64, 0u64);
    let mut cells = vec![from];
    while ix < nx || iy < ny {
        let x_first =
            u128::from(2 * ix + 1) * u128::from(ny) <= u128::from(2 * iy + 1) * u128::from(nx);
        if iy == ny || (ix < nx && x_first) {
            ix += 1;
        } else {
            iy += 1;
        }
        let x = x0 + dx.signum() * ix as i64;
        let y = y0 + dy.signum() * iy as i64;
        cells.push(CellCoord::new(x as i32, y as i32));
    }
    cells
}

/// Paints `id` over `cells` as a Connect stroke, clearing their links; id 0
/// erases. Returns the painted cells, sorted, for [`crate::resolve_region`].
pub fn apply_connect_stroke(
    terrain: &mut ChunkedLayer<TerrainId>,
    links: &mut ChunkedLayer<Links>,
    cells: &[CellCoord],
    id: TerrainId,
) -> Vec<CellCoord> {
    for &cell in cells {
        terrain.set(cell, id);
        links.clear(cell);
    }
    sorted(cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(x: i32, y: i32) -> CellCoord {
        CellCoord::new(x, y)
    }

    #[test]
    fn link_bits_follow_the_side_order() {
        let sides: Vec<_> = Peer::SIDES.map(Links::side_bit).to_vec();
        assert_eq!(sides, [1, 2, 4, 8]);
        let links = Links::EMPTY.with(Peer::Top).with(Peer::TopLeft);
        assert!(links.has(Peer::Top) && !links.has(Peer::Bottom) && !links.is_path());
        assert_eq!(links.bits(), Links::TOP);
        assert!(links.with_path().is_path());
        assert_eq!(Links::from_bits(0x1F), Some(Links(0x1F)));
        assert_eq!(Links::from_bits(0x20), None);
    }

    #[test]
    fn path_stroke_links_consecutive_cells_both_ways() {
        let mut terrain = ChunkedLayer::new();
        let mut links = ChunkedLayer::new();
        let cells = [c(0, 0), c(1, 0), c(1, -1)];
        let dirty = apply_path_stroke(&mut terrain, &mut links, &cells, 2).unwrap();
        assert_eq!(dirty, [c(1, -1), c(0, 0), c(1, 0)]);
        assert!(cells.iter().all(|cell| terrain.get(*cell) == 2));
        let path = Links::EMPTY.with_path();
        assert_eq!(links.get(c(0, 0)), path.with(Peer::Right));
        assert_eq!(links.get(c(1, 0)), path.with(Peer::Left).with(Peer::Bottom));
        assert_eq!(links.get(c(1, -1)), path.with(Peer::Top));
    }

    #[test]
    fn bad_path_strokes_write_nothing() {
        let mut terrain = ChunkedLayer::new();
        let mut links = ChunkedLayer::new();
        apply_path_stroke(&mut terrain, &mut links, &[c(5, 5)], 1).unwrap();
        let (before_terrain, before_links) = (terrain.clone(), links.clone());
        let gap = [c(0, 0), c(0, 1), c(0, 3)];
        let diagonal = [c(0, 0), c(1, 1)];
        let repeat = [c(0, 0), c(0, 0)];
        let edge = [c(i32::MAX, 0), c(i32::MIN, 0)];
        for (cells, index) in [(&gap[..], 2), (&diagonal, 1), (&repeat, 1), (&edge, 1)] {
            let err = apply_path_stroke(&mut terrain, &mut links, cells, 2);
            assert_eq!(err, Err(StrokeError::NotContiguous { index }));
        }
        let err = apply_path_stroke(&mut terrain, &mut links, &[c(0, 0)], 0);
        assert_eq!(err, Err(StrokeError::EmptyTerrain));
        assert_eq!((terrain, links), (before_terrain, before_links));
    }

    #[test]
    fn connect_stroke_and_erase_clear_links() {
        let mut terrain = ChunkedLayer::new();
        let mut links = ChunkedLayer::new();
        apply_path_stroke(&mut terrain, &mut links, &[c(0, 0), c(0, 1), c(0, 2)], 2).unwrap();
        let dirty = apply_connect_stroke(&mut terrain, &mut links, &[c(0, 1)], 3);
        assert_eq!(dirty, [c(0, 1)]);
        assert_eq!(
            (terrain.get(c(0, 1)), links.get(c(0, 1))),
            (3, Links::EMPTY)
        );
        apply_connect_stroke(&mut terrain, &mut links, &[c(0, 0)], 0);
        assert_eq!(
            (terrain.get(c(0, 0)), links.get(c(0, 0))),
            (0, Links::EMPTY)
        );
        assert!(links.get(c(0, 2)).is_path());
    }

    #[test]
    fn repainting_a_cell_with_another_terrain_drops_its_old_links() {
        let mut terrain = ChunkedLayer::new();
        let mut links = ChunkedLayer::new();
        apply_path_stroke(&mut terrain, &mut links, &[c(0, 0), c(1, 0)], 2).unwrap();
        apply_path_stroke(&mut terrain, &mut links, &[c(0, 0), c(0, 1)], 4).unwrap();
        apply_path_stroke(&mut terrain, &mut links, &[c(1, 0), c(1, 1)], 4).unwrap();
        let path = Links::EMPTY.with_path();
        assert_eq!(links.get(c(0, 0)), path.with(Peer::Top));
        assert_eq!(links.get(c(1, 0)), path.with(Peer::Top));
        assert!(!links_join(
            links.get(c(0, 0)),
            links.get(c(1, 0)),
            Peer::Right
        ));
        apply_path_stroke(&mut terrain, &mut links, &[c(1, 0), c(2, 0)], 4).unwrap();
        assert_eq!(links.get(c(1, 0)), path.with(Peer::Top).with(Peer::Right));
    }

    #[test]
    fn orthogonal_lines_are_valid_path_strokes() {
        let ends = [
            (c(0, 0), c(0, 0)),
            (c(0, 0), c(3, 3)),
            (c(2, -1), c(-5, 4)),
            (c(0, 0), c(40, -3)),
            (c(-7, 9), c(-7, -30)),
            (c(i32::MAX - 2, i32::MIN), c(i32::MAX, i32::MIN + 3)),
        ];
        for (from, to) in ends {
            let line = orthogonal_line(from, to);
            let span = from.0.x.abs_diff(to.0.x) + from.0.y.abs_diff(to.0.y);
            assert_eq!(line.len() as u32, span + 1);
            assert_eq!((line[0], line[line.len() - 1]), (from, to));
            assert_eq!(line, orthogonal_line(from, to));
            let mut terrain = ChunkedLayer::new();
            let mut links = ChunkedLayer::new();
            assert!(apply_path_stroke(&mut terrain, &mut links, &line, 1).is_ok());
        }
        assert_eq!(
            orthogonal_line(c(0, 0), c(2, 2)),
            [c(0, 0), c(1, 0), c(1, 1), c(2, 1), c(2, 2)]
        );
        let samples = [c(0, 0), c(5, 2), c(5, 2), c(-1, -4)];
        let mut stroke = vec![samples[0]];
        for pair in samples.windows(2) {
            stroke.extend(orthogonal_line(pair[0], pair[1]).into_iter().skip(1));
        }
        let mut terrain = ChunkedLayer::new();
        let mut links = ChunkedLayer::new();
        assert!(apply_path_stroke(&mut terrain, &mut links, &stroke, 1).is_ok());
    }
}
