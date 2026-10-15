//! The heightfield module contains the types and functions for working with [`Heightfield`]s.
//!
//! A heightfield is a 3D grid of [`Span`]s, where each column contains 0, 1, or more spans.

use alloc::vec::Vec;
use thiserror::Error;

use crate::{
    Aabb3d, TriMesh,
    rasterize::RasterizationError,
    span::{Span, SpanKey, Spans},
};

/// A dynamic heightfield representing obstructed space.
///
/// The grid of a heightfield is laid out on the xz-plane based on the value of cell_size.
/// Spans exist within the grid columns with the span min/max values at increments of cell_height from the base of the grid.
/// The smallest possible span size is cell_size * cell_size * cell_height. (Which is a single voxel.)
///
/// The standard process for building a heightfield is to build it with [`HeightfieldBuilder`],
/// then add spans using the various helper functions such as [`Heightfield::rasterize_triangles].
///
/// Building a heightfield is one of the first steps in creating a polygon mesh from source geometry. After it is populated, it is used to build a rcCompactHeightfield.
///
/// Example of iterating the spans in a heightfield:
/// ```rust
/// # use rerecast::*;
/// # let hf = Heightfield::default();
/// // Where hf is a heightfield.
///
/// let orig = hf.aabb.min;
/// let cs = hf.cell_size;
/// let ch = hf.cell_height;
///
/// let w = hf.width as usize;
/// let h = hf.height as usize;
///
/// for z in 0..h {
///     for x in 0..w {
///         // Deriving the minimum corner of the grid location.
///         let fx = orig.x + x as f32 * cs;
///         let fz = orig.z + z as f32 * cs;
///         // The base span in the column. (May be null.)
///         let mut s_iter = hf.spans[x + z * w];
///         while let Some(s) = s_iter {
///             let s = &hf.allocated_spans[s];
///             // Detriving the minium and maximum world position of the span.
///             let fymin = orig[1] + s.min as f32 * ch;
///             let fymax = orig[1] + s.max as f32 * ch;
///             // Do other things with the span before moving up the column.
///             s_iter = s.next;
///         }
///     }
/// }
/// ```
#[derive(Debug, Default, Clone)]
pub struct Heightfield {
    /// The width of the heightfield along the x-axis in cell units
    pub width: u16,
    /// The height of the heightfield along the z-axis in cell units
    pub height: u16,
    /// The AABB of the heightfield
    pub aabb: Aabb3d,
    /// The size of each cell on the xz-plane
    pub cell_size: f32,
    /// The size of each cell along the y-axis
    pub cell_height: f32,
    /// The indices to the spans in the heightfield in width*height order
    /// Each index corresponds to a column in the heightfield by pointing to the lowest span in the column
    pub spans: Vec<Option<SpanKey>>,
    /// All spans in the heightfield
    pub allocated_spans: Spans,
}

impl Heightfield {
    /// Rasterizes the triangles of a [`TriMesh`] into a [`Heightfield`].
    ///
    /// # Arguments
    ///
    /// - `trimesh` - The [`TriMesh`] to rasterize.
    /// - `walkable_height` Minimum floor to 'ceiling' height that will still allow the floor area to be considered walkable. [Limit: >= 3] [Units: vx]
    /// - `walkable_climb` - Minimum floor to 'ceiling' height that will still allow the floor area to be considered walkable. [Limit: >= 3] [Units: vx]
    ///
    pub fn populate_from_trimesh(
        &mut self,
        trimesh: TriMesh,
        walkable_height: u16,
        walkable_climb: u16,
    ) -> Result<(), RasterizationError> {
        // Implementation note: flag_merge_threshold and walkable_climb_height are the same thing in practice, so we just chose one name for the param.

        // Find triangles which are walkable based on their slope and rasterize them.
        for (i, triangle) in trimesh.indices.iter().enumerate() {
            let triangle = [
                trimesh.vertices[triangle[0] as usize],
                trimesh.vertices[triangle[1] as usize],
                trimesh.vertices[triangle[2] as usize],
            ];
            let area_type = trimesh.area_types[i];
            self.rasterize_triangle(triangle, area_type, walkable_climb)?;
        }
        // Once all geometry is rasterized, we do initial pass of filtering to
        // remove unwanted overhangs caused by the conservative rasterization
        // as well as filter spans where the character cannot possibly stand.
        self.filter_low_hanging_walkable_obstacles(walkable_climb);
        self.filter_ledge_spans(walkable_height, walkable_climb);
        self.filter_walkable_low_height_spans(walkable_height);
        Ok(())
    }

    /// https://github.com/recastnavigation/recastnavigation/blob/bd98d84c274ee06842bf51a4088ca82ac71f8c2d/Recast/Source/RecastRasterization.cpp#L105
    #[inline]
    pub(crate) fn add_span(&mut self, insertion: SpanInsertion) -> Result<(), SpanInsertionError> {
        let column_index = self.column_index(insertion.x, insertion.z);
        if column_index >= self.spans.len() {
            return Err(SpanInsertionError::ColumnIndexOutOfBounds {
                x: insertion.x,
                y: insertion.z,
            });
        }

        let mut new_span = insertion.span;
        let mut previous_span_key = None;
        let mut current_span_key_iter = self.spans[column_index];
        // Insert the new span, possibly merging it with existing spans.
        while let Some(current_span_key) = current_span_key_iter {
            let current_span = self.span_mut(current_span_key);
            current_span_key_iter = current_span.next;
            if current_span.min > new_span.max {
                // Current span is completely below the new span, break.
                break;
            }
            if current_span.max < new_span.min {
                // Current span is completely above the new span.  Keep going.
                previous_span_key.replace(current_span_key);
                continue;
            }
            // The new span overlaps with an existing span.  Merge them.
            if current_span.min < new_span.min {
                new_span.min = current_span.min;
            }
            if current_span.max > new_span.max {
                new_span.max = current_span.max;
            }

            // Merge flags.
            if (new_span.max as i32 - current_span.max as i32).unsigned_abs()
                <= insertion.flag_merge_threshold as u32
            {
                // Higher area ID numbers indicate higher resolution priority.
                let area = new_span.area.max(current_span.area.0);
                new_span.area = area.into();
            }

            // Remove the current span since it's now merged with newSpan.
            // Keep going because there might be other overlapping spans that also need to be merged.
            let next_key = current_span.next;
            self.allocated_spans.remove(current_span_key);
            if let Some(previous_span_key) = previous_span_key {
                self.span_mut(previous_span_key).next = next_key;
            } else {
                self.spans[column_index] = next_key;
            }
        }

        if let Some(previous_span_key) = previous_span_key {
            // Insert new span after prev
            new_span.next = self.span(previous_span_key).next;
            let new_span_key = self.allocated_spans.insert(new_span);
            self.span_mut(previous_span_key).next = Some(new_span_key);
        } else {
            // This span should go before the others in the list
            let lowest_span_key = self.spans[column_index];
            new_span.next = lowest_span_key;
            let new_span_key = self.allocated_spans.insert(new_span);
            self.spans[column_index] = Some(new_span_key);
        }

        Ok(())
    }

    #[inline]
    pub(crate) fn column_index(&self, x: u16, z: u16) -> usize {
        x as usize + z as usize * self.width as usize
    }

    #[inline]
    pub(crate) fn contains(&self, x: i32, z: i32) -> bool {
        // Jan: Interesting that `width` and `height` are not included in the bounds check, isn't it?
        x >= 0 && x < self.width as i32 && z >= 0 && z < self.height as i32
    }

    /// Returns the key of the lowest span in the column at the given coordinates.
    /// `None` if either the index is out of bounds or there is no span in the column.
    #[inline]
    pub fn span_key_at(&self, x: u16, z: u16) -> Option<SpanKey> {
        let column_index = self.column_index(x, z);
        let Some(span_key) = self.spans.get(column_index) else {
            // Invalid coordinates
            return None;
        };
        *span_key
    }

    /// Returns the span at the given coordinates.
    /// `None` if either the index is out of bounds or there is no span in the column.
    #[inline]
    pub fn span_at(&self, x: u16, z: u16) -> Option<&Span> {
        let Some(span_key) = self.span_key_at(x, z) else {
            // No span in this column
            return None;
        };
        Some(self.span(span_key))
    }

    /// Returns a mutable reference to the span at the given coordinates.
    /// `None` if either the index is out of bounds or there is no span in the column.
    #[inline]
    pub fn span_at_mut(&mut self, x: u16, z: u16) -> Option<&mut Span> {
        let Some(span_key) = self.span_key_at(x, z) else {
            // No span in this column
            return None;
        };
        Some(self.span_mut(span_key))
    }

    /// Returns a reference to the span with the given key.
    /// # Panics
    /// Panics if the key is not found.
    #[inline]
    pub fn span(&self, key: SpanKey) -> &Span {
        &self.allocated_spans[key]
    }

    /// Returns a mutable reference to the span with the given key.
    /// # Panics
    /// Panics if the key is not found.
    #[inline]
    pub fn span_mut(&mut self, key: SpanKey) -> &mut Span {
        &mut self.allocated_spans[key]
    }
}

/// A builder for [`Heightfield`]s.
pub struct HeightfieldBuilder {
    /// The AABB of the heightfield
    pub aabb: Aabb3d,
    /// The size of each cell on the xz-plane
    pub cell_size: f32,
    /// The size of each cell along the y-axis
    pub cell_height: f32,
}

impl HeightfieldBuilder {
    /// Builds the heightfield.
    ///
    /// # Panics
    ///
    /// Panics if the column count is above `usize::MAX`.
    pub fn build(self) -> Result<Heightfield, HeightfieldBuilderError> {
        let width = (self.aabb.max.x - self.aabb.min.x) / self.cell_size + 0.5;
        let height = (self.aabb.max.z - self.aabb.min.z) / self.cell_size + 0.5;
        let column_count = width as u128 * height as u128;
        if column_count > usize::MAX as u128 {
            return Err(HeightfieldBuilderError::ColumnCountTooLarge { width, height });
        }
        let column_count = column_count as usize;
        Ok(Heightfield {
            width: width as u16,
            height: height as u16,
            aabb: self.aabb,
            cell_size: self.cell_size,
            cell_height: self.cell_height,
            spans: vec![None; column_count],
            allocated_spans: Spans::with_min_capacity(column_count),
        })
    }
}

/// Errors that can occur when building a [`Heightfield`] with [`HeightfieldBuilder::build`].
#[derive(Error, Debug)]
pub enum HeightfieldBuilderError {
    /// Happens when the column count is too large.
    #[error("Column count (width*height) is too large, got {width}*{height}={column_count} but max is {max}", column_count = width * height, max = usize::MAX)]
    ColumnCountTooLarge {
        /// The width of the heightfield along the x-axis in cell units
        width: f32,
        /// The height of the heightfield along the z-axis in cell units
        height: f32,
    },
}

/// Errors that can occur when inserting a span into a [`Heightfield`]
#[derive(Error, Debug)]
pub enum SpanInsertionError {
    /// Happens when the column index is out of bounds.
    #[error("column index out of bounds: x={x}, y={y}")]
    ColumnIndexOutOfBounds {
        /// The x-coordinate of the span
        x: u16,
        /// The z-coordinate of the span
        y: u16,
    },
}

pub(crate) struct SpanInsertion {
    /// The x-coordinate of the span
    pub(crate) x: u16,
    /// The z-coordinate of the span
    pub(crate) z: u16,
    /// Maximum difference between the ceilings of two spans to merge area type IDs
    pub(crate) flag_merge_threshold: u16,
    /// The span to insert
    pub(crate) span: Span,
}

#[cfg(test)]
mod tests {

    use glam::Vec3A;

    use crate::{
        Aabb3d,
        span::{AreaType, SpanBuilder},
    };

    use super::*;

    fn height_field() -> Heightfield {
        HeightfieldBuilder {
            aabb: Aabb3d::new(Vec3A::ZERO, [5.0, 5.0, 5.0]),
            cell_size: 1.0,
            cell_height: 1.0,
        }
        .build()
        .unwrap()
    }

    fn span_low() -> SpanBuilder {
        SpanBuilder {
            min: 2,
            max: 4,
            area: AreaType(2),
            next: None,
        }
    }

    fn span_mid() -> SpanBuilder {
        SpanBuilder {
            min: 4,
            max: 7,
            area: AreaType(2),
            next: None,
        }
    }

    fn span_high() -> SpanBuilder {
        SpanBuilder {
            min: 7,
            max: 10,
            area: AreaType(2),
            next: None,
        }
    }

    #[test]
    fn can_create_heightfield() {
        let _heightfield = height_field();
    }

    #[test]
    fn can_add_span() {
        let mut heightfield = height_field();
        let expected_span = span_low().build();
        heightfield
            .add_span(SpanInsertion {
                x: 1,
                z: 3,
                flag_merge_threshold: 0,
                span: expected_span.clone(),
            })
            .unwrap();
        let span = heightfield.span_at(1, 3).unwrap();
        assert_eq!(*span, expected_span);

        let empty_span = heightfield.span_at(3, 1);
        assert_eq!(empty_span, None);
    }

    #[test]
    fn can_add_multiple_spans_next_to_each_other() {
        let mut heightfield = height_field();
        let expected_span_1 = span_low().build();
        heightfield
            .add_span(SpanInsertion {
                x: 1,
                z: 3,
                flag_merge_threshold: 0,
                span: expected_span_1.clone(),
            })
            .unwrap();

        let expected_span_2 = span_mid().build();
        heightfield
            .add_span(SpanInsertion {
                x: 2,
                z: 3,
                flag_merge_threshold: 0,
                span: expected_span_2.clone(),
            })
            .unwrap();

        let span = heightfield.span_at(1, 3).unwrap();
        assert_eq!(*span, expected_span_1);
        let span = heightfield.span_at(2, 3).unwrap();
        assert_eq!(*span, expected_span_2);

        let empty_span = heightfield.span_at(3, 1);
        assert_eq!(empty_span, None);
    }

    #[test]
    fn can_add_higher_span_in_same_column() {
        let mut heightfield = height_field();
        let span_low = span_low().build();
        heightfield
            .add_span(SpanInsertion {
                x: 1,
                z: 3,
                flag_merge_threshold: 0,
                span: span_low.clone(),
            })
            .unwrap();

        let span_high = span_high().build();
        heightfield
            .add_span(SpanInsertion {
                x: 1,
                z: 3,
                flag_merge_threshold: 0,
                span: span_high.clone(),
            })
            .unwrap();

        let span = heightfield.span_at(1, 3).unwrap();
        assert_eq_without_next(span, &span_low);
        let next_span = span.next.unwrap();
        let next_span = heightfield.span(next_span);
        assert_eq_without_next(next_span, &span_high);

        let empty_span = heightfield.span_at(3, 1);
        assert_eq!(empty_span, None);
    }

    #[test]
    fn can_add_lower_span_in_same_column() {
        let mut heightfield = height_field();
        let span_high = span_high().build();
        heightfield
            .add_span(SpanInsertion {
                x: 1,
                z: 3,
                flag_merge_threshold: 0,
                span: span_high.clone(),
            })
            .unwrap();

        let span_low = span_low().build();
        heightfield
            .add_span(SpanInsertion {
                x: 1,
                z: 3,
                flag_merge_threshold: 0,
                span: span_low.clone(),
            })
            .unwrap();

        let span = heightfield.span_at(1, 3).unwrap();
        assert_eq_without_next(span, &span_low);
        let next_span = span.next.unwrap();
        let next_span = heightfield.span(next_span);
        assert_eq_without_next(next_span, &span_high);

        let empty_span = heightfield.span_at(3, 1);
        assert_eq!(empty_span, None);
    }

    #[test]
    fn can_merge_spans() {
        let mut heightfield = height_field();
        let span_low = span_low().build();
        heightfield
            .add_span(SpanInsertion {
                x: 1,
                z: 3,
                flag_merge_threshold: 0,
                span: span_low.clone(),
            })
            .unwrap();

        let span_mid: Span = span_mid().build();
        heightfield
            .add_span(SpanInsertion {
                x: 1,
                z: 3,
                flag_merge_threshold: 0,
                span: span_mid.clone(),
            })
            .unwrap();

        let merged_span = SpanBuilder {
            min: span_low.min,
            max: span_mid.max,
            area: span_mid.area,
            next: None,
        }
        .build();

        let span = heightfield.span_at(1, 3).unwrap();
        assert_eq!(*span, merged_span);

        let empty_span = heightfield.span_at(3, 1);
        assert_eq!(empty_span, None);
    }

    #[track_caller]
    fn assert_eq_without_next(span: &Span, expected_span: &Span) {
        assert_eq!(span.min, expected_span.min, "min is not equal");
        assert_eq!(span.max, expected_span.max, "max is not equal");
        assert_eq!(span.area, expected_span.area, "area is not equal");
    }
}
