use core::ops::Range;

/// Provides information on the content of a cell column in a [`CompactHeightfield`].
///
/// See the rcCompactHeightfield documentation for an example of how compact cells are used to iterate the heightfield.
///
/// Useful instances of this type can only by obtained from a [`CompactHeightfield`].
///
/// [`CompactHeightfield`]: crate::compact_heightfield::CompactHeightfield
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CompactCell {
    // original: 24 bits
    /// Index to the first span in the column.
    index: u32,
    // original: 8 bits
    /// Number of spans in the column.
    count: u8,
}

impl CompactCell {
    /// Returns the index of the first span in the column.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Returns the number of spans in the column.
    pub fn count(&self) -> u8 {
        self.count
    }

    /// Sets the index of the first span in the column.
    pub fn set_index(&mut self, index: u32) {
        self.index = index;
    }

    /// Sets the number of spans in the column.
    pub fn set_count(&mut self, count: u8) {
        self.count = count;
    }

    /// Increments the number of spans in the column by 1.
    pub fn inc_count(&mut self) {
        self.count += 1;
    }

    /// Returns a range over the indices of spans in the cell column.
    pub fn index_range(&self) -> Range<usize> {
        self.index as usize..self.index as usize + self.count as usize
    }
}
