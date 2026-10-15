use core::ops::{Add, AddAssign};

/// A region in a [`CompactHeightfield`](crate::compact_heightfield::CompactHeightfield).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(transparent)]
pub struct RegionId(u16);
bitflags::bitflags! {
    impl RegionId: u16 {
        /// The default region, which is used for spans that are not in a region, i.e. not walkable.
        const NONE = 0;
        /// Heightfield border flag.
        /// If a heightfield region ID has this bit set, then the region is a border
        /// region and its spans are considered un-walkable.
        /// (Used during the region and contour build process.)
        const BORDER_REGION = 0x8000;
        /// The maximum region ID.
        const MAX = u16::MAX;
    }
}

impl Add<u16> for RegionId {
    type Output = Self;
    fn add(self, other: u16) -> Self::Output {
        RegionId::from(self.bits() + other)
    }
}

impl AddAssign<u16> for RegionId {
    fn add_assign(&mut self, other: u16) {
        *self = RegionId::from(self.bits() + other);
    }
}

impl Default for RegionId {
    fn default() -> Self {
        Self::NONE
    }
}

impl From<u16> for RegionId {
    fn from(value: u16) -> Self {
        RegionId::from_bits_retain(value)
    }
}
