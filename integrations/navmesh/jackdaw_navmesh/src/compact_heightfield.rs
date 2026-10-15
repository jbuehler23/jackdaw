use crate::{
    Aabb3d,
    compact_cell::CompactCell,
    compact_span::CompactSpan,
    heightfield::Heightfield,
    math::{dir_offset_x, dir_offset_z},
    region::RegionId,
    span::AreaType,
};
use alloc::vec::Vec;

/// A compact, static heightfield representing unobstructed space.
///
/// For this type of heightfield, the spans represent the open (unobstructed) space above the solid surfaces of a voxel field.
/// It is usually created from a [`Heightfield`] object. Data is stored in a compact, efficient manner,
/// but the structure is not condusive to adding and removing spans.
///
/// The standard process for buidling a compact heightfield is to build it using [`Heightfield::into_compact`], then run it through the various helper functions to generate neighbor and region data.
///
/// Connected neighbor spans form non-overlapping surfaces. When neighbor information is generated, spans will include data that can be used to locate axis-neighbors. Axis-neighbors are connected spans that are offset from the current cell column as follows:
///
/// ```txt
/// Direction 0 = (-1, 0)
/// Direction 1 = (0, 1)
/// Direction 2 = (1, 0)
/// Direction 3 = (0, -1)
/// ```
/// Example of iterating and inspecting spans, including connected neighbors:
///
/// ```rust
/// # use rerecast::*;
/// # let chf = CompactHeightfield::default();
/// // Where chf is an instance of a CompactHeightfield.
///
/// let cs = chf.cell_size;
/// let ch = chf.cell_height;
///
/// for z in 0..chf.height {
///     for x in 0..chf.width {
///         // Deriving the minimum corner of the grid location.
///         let fx = chf.aabb.min.x + x as f32 * cs;
///         let fz = chf.aabb.min.z + z as f32 * cs;
///         println!("Corners: {fx},  {fz}");
///
///         // Get the cell for the grid location then iterate
///         // up the column.
///         let c = chf.cell_at(x, z);
///         for i in c.index_range() {
///             let s = &chf.spans[i];
///
///             // Deriving the minimum (floor) of the span.
///             let fy = chf.aabb.min.y + (s.y + 1) as f32 * ch;
///             println!("Span floor: {fy}");
///
///             // Testing the area assignment of the span.
///             if chf.areas[i] == AreaType::DEFAULT_WALKABLE {
///                 // The span is in the default 'walkable area'.
///             } else if chf.areas[i] == AreaType::NOT_WALKABLE {
///                 // The surface is not considered walkable.
///                 // E.g. It was filtered out during the build processes.
///             } else {
///                 // Do something. (Only applicable for custom build
///                 // build processes.)
///             }
///
///             // Iterating the connected axis-neighbor spans.
///             for dir in 0..4 {
///                 if let Some(con) = s.con(dir) {
///                     // There is a neighbor in this direction.
///                     let (_nx, _ny, ni) = chf.con_indices(x as i32, z as i32, dir, con);
///                     let ns = &chf.spans[ni];
///                     // Do something with the neighbor span.
///                     println!("Neighbor span: {ns:?}");
///                 }
///             }
///         }
///     }
/// }
/// ```
#[derive(Debug, Default, Clone)]
pub struct CompactHeightfield {
    /// The width of the heightfield along the x-axis in cell units
    pub width: u16,
    /// The height of the heightfield along the z-axis in cell units
    pub height: u16,
    /// The walkable height used during the build of the field
    /// (See: [`Config::walkable_height`](crate::Config::walkable_height))
    pub walkable_height: u16,
    /// The walkable climb used during the build of the field.
    /// (See: [`Config::walkable_climb`](crate::Config::walkable_climb))
    pub walkable_climb: u16,
    /// The AABB border size used during the build of the field.
    /// (See: [`Config::border_size`](crate::Config::border_size))
    pub border_size: u16,
    /// The maximum distance value of any span within the field.
    pub max_distance: u16,
    /// The maximum region id of any span within the field.
    pub max_region: RegionId,
    /// The AABB of the heightfield
    pub aabb: Aabb3d,
    /// The size of each cell on the xz-plane
    pub cell_size: f32,
    /// The size of each cell along the y-axis
    pub cell_height: f32,
    /// The cells in the heightfield [Size: `width * height`]
    pub cells: Vec<CompactCell>,
    /// All walkable spans in the heightfield
    pub spans: Vec<CompactSpan>,
    /// Vector containing border distance data. [Size: `spans.len()`]
    pub dist: Vec<u16>,
    /// Vector containing area type data. [Size: `spans.len()`]
    pub areas: Vec<AreaType>,
}

impl Heightfield {
    const MAX_HEIGHT: u16 = u16::MAX;

    /// Builds a compact heightfield from a heightfield.
    ///
    /// # Errors
    ///
    /// Returns an error if the heightfield has too many layers.
    pub fn into_compact(
        self,
        walkable_height: u16,
        walkable_climb: u16,
    ) -> Result<CompactHeightfield, CompactHeightfieldError> {
        let walkable_span_count = self
            .allocated_spans
            .values()
            .filter(|span| span.area.is_walkable())
            .count();

        let mut compact_heightfield = CompactHeightfield {
            width: self.width,
            height: self.height,
            walkable_height,
            walkable_climb,
            border_size: 0,
            aabb: self.aabb,
            max_distance: 0,
            max_region: RegionId::NONE,
            cell_size: self.cell_size,
            cell_height: self.cell_height,
            cells: vec![CompactCell::default(); self.width as usize * self.height as usize],
            spans: vec![CompactSpan::default(); walkable_span_count],
            dist: vec![],
            areas: vec![AreaType::NOT_WALKABLE; walkable_span_count],
        };
        compact_heightfield.aabb.max.y += walkable_height as f32 * compact_heightfield.cell_height;

        let mut cell_index = 0_usize;
        // Fill in cells and spans
        for z in 0..self.height {
            for x in 0..self.width {
                let Some(span_key) = self.span_key_at(x, z) else {
                    // If there are no spans at this cell, just leave the data to index=0, count=0.
                    continue;
                };
                let mut span_key_iter = Some(span_key);
                let column_index = self.column_index(x, z);

                let cell = &mut compact_heightfield.cells[column_index];
                cell.set_index(cell_index as u32);
                cell.set_count(0);

                while let Some(span_key) = span_key_iter {
                    let span = self.span(span_key);
                    span_key_iter = span.next;
                    if !span.area.is_walkable() {
                        continue;
                    }
                    let bot = span.max;
                    let top = span
                        .next
                        .map(|span| self.span(span).min)
                        .unwrap_or(Self::MAX_HEIGHT);
                    compact_heightfield.spans[cell_index].y = bot.clamp(0, Self::MAX_HEIGHT);
                    let height = (top.saturating_sub(bot)).min(u8::MAX.into()) as u8;
                    compact_heightfield.spans[cell_index].set_height(height);
                    compact_heightfield.areas[cell_index] = span.area;
                    cell_index += 1;
                    cell.inc_count();
                }
            }
        }

        // Find neighbour connections
        const MAX_LAYERS: u8 = CompactSpan::NOT_CONNECTED - 1;
        let mut max_layer_index = 0_u32;
        for z in 0..self.height {
            for x in 0..self.width {
                let column_index = x as usize + z as usize * self.width as usize;
                let cell = &mut compact_heightfield.cells[column_index];
                let index_count = cell.index() as usize + cell.count() as usize;
                for i in cell.index() as usize..index_count {
                    for dir in 0..4_u8 {
                        compact_heightfield.spans[i].set_con(dir, None);
                        let neighbor_x = x as i32 + dir_offset_x(dir) as i32;
                        let neighbor_z = z as i32 + dir_offset_z(dir) as i32;
                        // First check that the neighbour cell is in bounds.
                        if !self.contains(neighbor_x, neighbor_z) {
                            continue;
                        }
                        let neighbor_x = neighbor_x as u16;
                        let neighbor_z = neighbor_z as u16;

                        // Iterate over all neighbour spans and check if any of the is
                        // accessible from current cell.
                        let column_index = self.column_index(neighbor_x, neighbor_z);
                        let neighbor_cell = &compact_heightfield.cells[column_index];
                        let neighbor_index_count =
                            neighbor_cell.index() as usize + neighbor_cell.count() as usize;
                        let span_clone = compact_heightfield.spans[i].clone();
                        for k in neighbor_cell.index() as usize..neighbor_index_count as usize {
                            let neighbor_span = &compact_heightfield.spans[k];
                            let bot = span_clone.y.max(neighbor_span.y);
                            let top = (span_clone.y + span_clone.height() as u16)
                                .min(neighbor_span.y + neighbor_span.height() as u16);

                            // Check that the gap between the spans is walkable,
                            // and that the climb height between the gaps is not too high.
                            let is_walkable = (top as i32 - bot as i32) >= walkable_height as i32;
                            let is_climbable = (neighbor_span.y as i32 - span_clone.y as i32).abs()
                                <= walkable_climb as i32;
                            if !is_walkable || !is_climbable {
                                continue;
                            }
                            // Mark direction as walkable.
                            let layer_index = k as i32 - neighbor_cell.index() as i32;
                            if layer_index < 0 || layer_index >= MAX_LAYERS as i32 {
                                max_layer_index = max_layer_index.max(layer_index as u32);
                                continue;
                            }
                            let layer_index = layer_index as u8;
                            compact_heightfield.spans[i].set_con(dir, Some(layer_index));
                            break;
                        }
                    }
                }
            }
        }
        if max_layer_index > MAX_LAYERS as u32 {
            return Err(CompactHeightfieldError::TooManyLayers {
                max_layer_index: MAX_LAYERS,
                layer_index: max_layer_index,
            });
        }
        Ok(compact_heightfield)
    }
}

impl CompactHeightfield {
    #[inline]
    pub(crate) fn column_index(&self, x: u16, z: u16) -> usize {
        x as usize + z as usize * self.width as usize
    }

    /// Returns the cell at the given coordinates. Returns `None` if the coordinates are invalid.
    #[inline]
    pub fn get_cell_at(&self, x: u16, z: u16) -> Option<&CompactCell> {
        let Some(cell) = self.cells.get(self.column_index(x, z)) else {
            // Invalid coordinates
            return None;
        };
        Some(cell)
    }

    /// Returns the cell at the given coordinates. Panics if the coordinates are invalid.
    #[inline]
    pub fn cell_at(&self, x: u16, z: u16) -> &CompactCell {
        &self.cells[self.column_index(x, z)]
    }

    /// Returns the cell mutably at the given coordinates. Returns `None` if the coordinates are invalid.
    #[inline]
    pub fn get_cell_at_mut(&mut self, x: u16, z: u16) -> Option<&mut CompactCell> {
        let index = self.column_index(x, z);
        let Some(cell) = self.cells.get_mut(index) else {
            // Invalid coordinates
            return None;
        };
        Some(cell)
    }

    /// Returns the cell mutably at the given coordinates. Panics if the coordinates are invalid.
    #[inline]
    pub fn cell_at_mut(&mut self, x: u16, z: u16) -> &mut CompactCell {
        let index = self.column_index(x, z);
        &mut self.cells[index]
    }

    /// Given a span at the indices `(x, z)`, a direction `dir`, and a connection `con`, returns:
    /// - The x index of the neighbor span
    /// - The z index of the neighbor span
    /// - The index of the neighbor span in [`Self::spans`]
    #[inline]
    pub fn con_indices(&self, x: i32, z: i32, dir: u8, con: u8) -> (i32, i32, usize) {
        let a_x = x + dir_offset_x(dir) as i32;
        let a_z = z + dir_offset_z(dir) as i32;
        let cell_index = (a_x + a_z * self.width as i32) as usize;
        let a_i = self.cells[cell_index].index() as usize + con as usize;
        (a_x, a_z, a_i)
    }
}

/// Errors that can occur when building a compact heightfield.
#[derive(Debug, thiserror::Error)]
pub enum CompactHeightfieldError {
    /// The heightfield has too many layers.
    #[error(
        "Heightfield has too many layers. Max layer index is {max_layer_index}, but got {layer_index}"
    )]
    TooManyLayers {
        /// The maximum layer index.
        max_layer_index: u8,
        /// The layer index that caused the error.
        layer_index: u32,
    },
}
