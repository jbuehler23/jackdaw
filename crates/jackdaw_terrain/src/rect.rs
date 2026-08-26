//! The block of grid points one edit touched.
//!
//! A brush writes a disc a few dozen cells across; the arrays it writes
//! into are `resolution^2`. Recording, re-uploading or re-snapping a
//! stroke works over that difference, and every caller has to agree on
//! the same bounds: an undo entry storing a smaller rectangle than the
//! brush wrote would restore a stroke only partly.
//!
//! [`GridRect::brush`] is the one definition of a brush's footprint,
//! computed the way [`crate::apply_brush`] and [`crate::quantize_region`]
//! compute their loop bounds.

use bevy_math::Vec2;

/// An axis-aligned block of grid points.
///
/// Row-major, like every per-cell array in this crate: the values for
/// this rect are `height` runs of `width`, read from rows `z .. z +
/// height` of a dense grid at stride `resolution`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridRect {
    pub x: u32,
    pub z: u32,
    pub width: u32,
    pub height: u32,
}

impl GridRect {
    /// The whole grid of a `resolution`-per-edge terrain.
    pub fn whole(resolution: u32) -> Self {
        Self {
            x: 0,
            z: 0,
            width: resolution,
            height: resolution,
        }
    }

    /// The grid points a brush of this shape writes, or `None` when it
    /// writes none: an empty grid, a degenerate radius, or a centre far
    /// enough outside the terrain that its footprint misses entirely.
    ///
    /// The bounds are [`crate::apply_brush`]'s own loop bounds, so a
    /// caller recording or re-snapping a stroke covers every cell the
    /// stroke wrote.
    ///
    /// They run one cell wider than the brushed disc on each unclamped
    /// side: the loop walks `floor(centre - radius)` through
    /// `ceil(centre + radius)`, and that outermost ring always falls
    /// outside the falloff. A rect-scoped record therefore carries a
    /// one-cell margin of unchanged values, which is what computing the
    /// footprint from the brush alone, without evaluating the falloff per
    /// cell, costs.
    pub fn brush(resolution: u32, center: Vec2, radius: f32) -> Option<Self> {
        if resolution == 0 || !radius.is_finite() || radius <= 0.0 {
            return None;
        }
        if !center.x.is_finite() || !center.y.is_finite() {
            return None;
        }
        let last = resolution as i32 - 1;
        let min_x = ((center.x - radius).floor() as i32).clamp(0, last);
        let max_x = ((center.x + radius).ceil() as i32).clamp(0, last);
        let min_z = ((center.y - radius).floor() as i32).clamp(0, last);
        let max_z = ((center.y + radius).ceil() as i32).clamp(0, last);
        // A footprint entirely off one edge clamps to that edge, which
        // would name a one-cell rect the brush never reached.
        if (center.x + radius) < 0.0
            || (center.x - radius) > last as f32
            || (center.y + radius) < 0.0
            || (center.y - radius) > last as f32
        {
            return None;
        }
        Some(Self {
            x: min_x as u32,
            z: min_z as u32,
            width: (max_x - min_x) as u32 + 1,
            height: (max_z - min_z) as u32 + 1,
        })
    }

    /// Number of grid points inside.
    pub fn cells(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The smallest rect covering both.
    pub fn union(self, other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        let x = self.x.min(other.x);
        let z = self.z.min(other.z);
        let right = (self.x + self.width).max(other.x + other.width);
        let bottom = (self.z + self.height).max(other.z + other.height);
        Self {
            x,
            z,
            width: right - x,
            height: bottom - z,
        }
    }

    /// Copy this rect's values out of a dense `resolution^2` grid.
    ///
    /// A row that runs past the end of `grid` contributes what is there
    /// and nothing more, so a grid shorter than the terrain claims reads
    /// short rather than panicking.
    pub fn read<T: Copy>(&self, grid: &[T], resolution: u32) -> Vec<T> {
        let mut out = Vec::with_capacity(self.cells());
        for row in self.rows(resolution) {
            out.extend_from_slice(grid.get(row).unwrap_or(&[]));
        }
        out
    }

    /// Write this rect's values back into a dense `resolution^2` grid.
    ///
    /// `values` is the row-major run [`Self::read`] produces. A short
    /// `values`, or a grid shorter than the terrain claims, writes what
    /// fits and leaves the rest alone.
    pub fn write<T: Copy>(&self, grid: &mut [T], resolution: u32, values: &[T]) {
        let mut at = 0;
        for row in self.rows(resolution) {
            let take = row.len().min(values.len().saturating_sub(at));
            if take == 0 {
                break;
            }
            let start = row.start;
            if start >= grid.len() {
                break;
            }
            let end = (start + take).min(grid.len());
            grid[start..end].copy_from_slice(&values[at..at + (end - start)]);
            at += take;
        }
    }

    /// Copy this rect's rows from one dense grid into another of the
    /// same shape, leaving everything outside the rect alone.
    pub fn copy_into<T: Copy>(&self, from: &[T], to: &mut [T], resolution: u32) {
        for row in self.rows(resolution) {
            let end = row.end.min(from.len()).min(to.len());
            if row.start >= end {
                continue;
            }
            to[row.start..end].copy_from_slice(&from[row.start..end]);
        }
    }

    /// Index ranges of this rect's rows in a dense `resolution^2` grid.
    pub fn rows(&self, resolution: u32) -> impl Iterator<Item = core::ops::Range<usize>> + use<> {
        let stride = resolution as usize;
        let x = self.x as usize;
        let width = self.width as usize;
        (self.z..self.z + self.height).map(move |gz| {
            let start = gz as usize * stride + x;
            start..start + width
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rect has to name exactly the cells `apply_brush` writes, or an
    /// undo entry scoped to it would restore a stroke only partly.
    #[test]
    fn the_brush_rect_matches_what_apply_brush_writes() {
        use crate::{Heightmap, SculptTool, apply_brush};

        let resolution = 32;
        for (center, radius) in [
            (Vec2::new(16.0, 16.0), 5.0),
            (Vec2::new(0.5, 0.5), 4.0),
            (Vec2::new(31.0, 20.0), 7.5),
            (Vec2::new(3.25, 28.75), 2.0),
        ] {
            let mut hm = Heightmap::new(resolution, Vec2::splat(31.0), 10.0);
            apply_brush(
                &mut hm,
                SculptTool::Raise,
                center,
                radius,
                10.0,
                2.0,
                1.0 / 60.0,
                None,
            );
            let rect = GridRect::brush(resolution, center, radius).expect("brush touches the grid");
            for gz in 0..resolution {
                for gx in 0..resolution {
                    let inside = gx >= rect.x
                        && gx < rect.x + rect.width
                        && gz >= rect.z
                        && gz < rect.z + rect.height;
                    let written = hm.heights[(gz * resolution + gx) as usize] != 0.0;
                    assert!(
                        !written || inside,
                        "({gx},{gz}) was written but sits outside {rect:?}",
                    );
                }
            }
        }
    }

    #[test]
    fn a_brush_that_misses_the_grid_names_no_cells() {
        assert_eq!(GridRect::brush(0, Vec2::ZERO, 4.0), None);
        assert_eq!(GridRect::brush(16, Vec2::ZERO, 0.0), None);
        assert_eq!(GridRect::brush(16, Vec2::ZERO, f32::NAN), None);
        assert_eq!(GridRect::brush(16, Vec2::splat(f32::NAN), 4.0), None);
        assert_eq!(GridRect::brush(16, Vec2::new(-40.0, 8.0), 4.0), None);
        assert_eq!(GridRect::brush(16, Vec2::new(8.0, 90.0), 4.0), None);
    }

    #[test]
    fn a_brush_hanging_over_an_edge_keeps_only_the_cells_on_the_grid() {
        let rect = GridRect::brush(16, Vec2::new(1.0, 1.0), 4.0).expect("overlaps");
        assert_eq!(
            rect,
            GridRect {
                x: 0,
                z: 0,
                width: 6,
                height: 6
            }
        );
    }

    #[test]
    fn a_rect_round_trips_through_a_dense_grid() {
        let resolution = 8;
        let grid: Vec<u32> = (0..64).collect();
        let rect = GridRect {
            x: 2,
            z: 3,
            width: 3,
            height: 2,
        };
        let values = rect.read(&grid, resolution);
        assert_eq!(values, vec![26, 27, 28, 34, 35, 36]);

        let mut blank = vec![0u32; 64];
        rect.write(&mut blank, resolution, &values);
        assert_eq!(blank[26], 26);
        assert_eq!(blank[36], 36);
        assert_eq!(blank[25], 0, "outside the rect stays untouched");
        assert_eq!(blank[29], 0);
    }

    #[test]
    fn a_union_covers_both_rects_and_ignores_an_empty_one() {
        let a = GridRect {
            x: 2,
            z: 2,
            width: 2,
            height: 2,
        };
        let b = GridRect {
            x: 5,
            z: 1,
            width: 1,
            height: 6,
        };
        assert_eq!(
            a.union(b),
            GridRect {
                x: 2,
                z: 1,
                width: 4,
                height: 6
            }
        );
        let empty = GridRect {
            x: 9,
            z: 9,
            width: 0,
            height: 0,
        };
        assert_eq!(a.union(empty), a);
        assert_eq!(empty.union(a), a);
    }

    /// A grid shorter than the terrain claims -- an unpainted layer, a
    /// document mid-resize -- must read short rather than panic.
    #[test]
    fn a_short_grid_reads_and_writes_what_is_there() {
        let rect = GridRect {
            x: 0,
            z: 0,
            width: 4,
            height: 4,
        };
        assert!(rect.read(&[1u8, 2, 3], 4).len() < rect.cells());
        let mut short = vec![0u8; 3];
        rect.write(&mut short, 4, &[9u8; 16]);
        assert_eq!(short, vec![9, 9, 9]);
    }
}
