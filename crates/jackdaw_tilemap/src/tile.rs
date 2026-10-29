//! Per-cell values: resolved tile references and authoring terrain ids.

use crate::layer::CellValue;

/// Terrain painted into an authoring cell. `0` is empty.
pub type TerrainId = u8;

impl CellValue for TerrainId {
    const EMPTY: Self = 0;
}

/// A resolved tile: an index into the tile set plus orientation bits.
///
/// Orientation uses the 8-element dihedral group (flip x, flip y, transpose)
/// and a hidden flag, so an all-zero byte is a plain visible tile; the remaining bits are reserved and always zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileRef {
    index: u16,
    orientation: u8,
}

impl TileRef {
    pub const EMPTY_INDEX: u16 = 0xFFFF;
    pub const EMPTY: Self = Self {
        index: Self::EMPTY_INDEX,
        orientation: 0,
    };

    pub const FLIP_X: u8 = 1 << 0;
    pub const FLIP_Y: u8 = 1 << 1;
    /// Swap x and y in the tile image (x right, y down): a flip across the
    /// diagonal from its top-left corner. Applied before the flips.
    pub const TRANSPOSE: u8 = 1 << 2;
    pub const HIDDEN: u8 = 1 << 3;
    pub const ORIENTATION_MASK: u8 = Self::FLIP_X | Self::FLIP_Y | Self::TRANSPOSE | Self::HIDDEN;

    /// A visible, unrotated tile.
    pub const fn new(index: u16) -> Self {
        Self::from_raw(index, 0)
    }

    /// Builds a tile from raw bits, zeroing reserved bits. An empty index yields [`TileRef::EMPTY`].
    pub const fn from_raw(index: u16, orientation: u8) -> Self {
        if index == Self::EMPTY_INDEX {
            return Self::EMPTY;
        }
        Self {
            index,
            orientation: orientation & Self::ORIENTATION_MASK,
        }
    }

    const fn has(self, bit: u8) -> bool {
        self.orientation & bit != 0
    }

    const fn with_bit(self, bit: u8, on: bool) -> Self {
        let orientation = if on {
            self.orientation | bit
        } else {
            self.orientation & !bit
        };
        Self::from_raw(self.index, orientation)
    }

    pub const fn index(self) -> u16 {
        self.index
    }

    pub const fn orientation(self) -> u8 {
        self.orientation
    }

    pub const fn is_empty(self) -> bool {
        self.index == Self::EMPTY_INDEX
    }

    pub const fn flip_x(self) -> bool {
        self.has(Self::FLIP_X)
    }

    pub const fn flip_y(self) -> bool {
        self.has(Self::FLIP_Y)
    }

    pub const fn transpose(self) -> bool {
        self.has(Self::TRANSPOSE)
    }

    pub const fn visible(self) -> bool {
        !self.has(Self::HIDDEN)
    }

    pub const fn with_flip_x(self, on: bool) -> Self {
        self.with_bit(Self::FLIP_X, on)
    }

    pub const fn with_flip_y(self, on: bool) -> Self {
        self.with_bit(Self::FLIP_Y, on)
    }

    pub const fn with_transpose(self, on: bool) -> Self {
        self.with_bit(Self::TRANSPOSE, on)
    }

    pub const fn with_visible(self, on: bool) -> Self {
        self.with_bit(Self::HIDDEN, !on)
    }
}

impl Default for TileRef {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl CellValue for TileRef {
    const EMPTY: Self = TileRef::EMPTY;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_tile_is_visible_and_unoriented() {
        let t = TileRef::new(0);
        assert!(!t.is_empty());
        assert!(t.visible());
        assert!(!t.flip_x() && !t.flip_y() && !t.transpose());
        assert_eq!(t.orientation(), 0);
    }

    #[test]
    fn empty_tile_uses_reserved_index() {
        assert!(TileRef::EMPTY.is_empty());
        assert_eq!(TileRef::EMPTY.index(), 0xFFFF);
        assert_eq!(TileRef::default(), TileRef::EMPTY);
        assert_eq!(TileRef::new(0xFFFF), TileRef::EMPTY);
        assert_eq!(TileRef::from_raw(0xFFFF, TileRef::FLIP_X), TileRef::EMPTY);
    }

    #[test]
    fn orientation_bits_toggle_independently() {
        let t = TileRef::new(7)
            .with_flip_x(true)
            .with_transpose(true)
            .with_visible(false);
        assert!(t.flip_x());
        assert!(!t.flip_y());
        assert!(t.transpose());
        assert!(!t.visible());
        assert_eq!(t.index(), 7);
        let t = t.with_flip_x(false).with_flip_y(true);
        assert_eq!(
            t.orientation(),
            TileRef::FLIP_Y | TileRef::TRANSPOSE | TileRef::HIDDEN
        );
        let t = t.with_visible(true);
        assert!(t.visible());
        assert_eq!(t.orientation(), TileRef::FLIP_Y | TileRef::TRANSPOSE);
    }

    #[test]
    fn from_raw_zeroes_reserved_bits() {
        let t = TileRef::from_raw(3, 0xFF);
        assert_eq!(t.orientation(), TileRef::ORIENTATION_MASK);
        assert_eq!(t, TileRef::from_raw(3, TileRef::ORIENTATION_MASK));
    }

    #[test]
    fn terrain_zero_is_empty() {
        assert_eq!(<TerrainId as CellValue>::EMPTY, 0);
        assert!(CellValue::is_empty(&(0 as TerrainId)));
        assert!(!CellValue::is_empty(&(1 as TerrainId)));
    }
}
