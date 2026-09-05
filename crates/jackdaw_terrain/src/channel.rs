//! Named per-cell integer layers painted over a heightmap.
//!
//! Jackdaw stores the name, the values and the width, and never interprets
//! them. A channel holds one small number per terrain cell: a material
//! index, a biome id, walkability, a spawn weight.

use bevy_math::Vec2;

use crate::brush::compute_falloff;

/// Storage width a channel's values are written with on disk.
///
/// Values are held in memory as `u16` whatever the width. The width decides
/// how many bytes a channel costs in the sidecar and the largest paintable
/// value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ChannelElement {
    /// One byte per cell, values `0..=255`.
    #[default]
    U8,
    /// Two bytes per cell, values `0..=65535`.
    U16,
}

impl ChannelElement {
    /// Largest value this width can hold. Painting clamps to it.
    pub fn max_value(self) -> u16 {
        match self {
            Self::U8 => u16::from(u8::MAX),
            Self::U16 => u16::MAX,
        }
    }

    /// Bytes one cell occupies on disk.
    pub fn byte_width(self) -> usize {
        match self {
            Self::U8 => 1,
            Self::U16 => 2,
        }
    }

    /// Wire tag used by the sidecar format.
    pub fn tag(self) -> u8 {
        match self {
            Self::U8 => 0,
            Self::U16 => 1,
        }
    }

    /// Inverse of [`ChannelElement::tag`]. Returns `None` for unknown tags.
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            0 => Some(Self::U8),
            1 => Some(Self::U16),
            _ => None,
        }
    }
}

/// What a terrain declares one channel to be: its name and its width.
///
/// The directory entry only. Per-cell values live in the regions, beside
/// the heights they describe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelDescriptor {
    /// Project-defined name. Opaque to jackdaw; used as the export filename.
    pub name: String,
    /// Storage width, and therefore the value ceiling.
    pub element: ChannelElement,
}

impl ChannelDescriptor {
    pub fn new(name: impl Into<String>, element: ChannelElement) -> Self {
        Self {
            name: name.into(),
            element,
        }
    }
}

/// One named layer of per-cell values, row-major, `resolution^2` long.
///
/// The inlet for reading a sidecar written as a dense grid over the whole
/// terrain. Loading slices it into the regions it covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelData {
    /// Project-defined name. Opaque to jackdaw; used as the export filename.
    pub name: String,
    /// Storage width, and therefore the value ceiling.
    pub element: ChannelElement,
    /// Row-major values, length `resolution^2`.
    pub values: Vec<u16>,
}

impl ChannelData {
    /// A channel of `resolution^2` zeroes.
    pub fn new(name: impl Into<String>, element: ChannelElement, resolution: u32) -> Self {
        Self {
            name: name.into(),
            element,
            values: vec![0; (resolution as usize) * (resolution as usize)],
        }
    }

    /// Value at integer grid coordinates, or 0 out of bounds.
    ///
    /// A channel whose length does not match `resolution^2` reads 0
    /// everywhere rather than only past its end: the row stride is the
    /// caller's, so an index landing inside the stored values names a
    /// different cell than the one asked for.
    pub fn get(&self, resolution: u32, x: u32, z: u32) -> u16 {
        let side = resolution as usize;
        if x >= resolution || z >= resolution || self.values.len() != side * side {
            return 0;
        }
        self.values[z as usize * side + x as usize]
    }

    /// Grow or shrink to `resolution^2`, zero-filling any new cells.
    pub fn resize_to(&mut self, resolution: u32) {
        let want = (resolution as usize) * (resolution as usize);
        self.values.resize(want, 0);
    }
}

/// Stamp `value` into a channel under a circular brush.
///
/// Integer channels cannot blend, so the brush is a threshold stamp rather
/// than an accumulation: a cell is written when its falloff is at least
/// `threshold`, and left alone otherwise. `threshold` of 0 writes the whole
/// disc; raising it shrinks the written area inside the same radius.
///
/// `center` and `radius` are in grid cells. `value` is clamped to `element`.
/// Returns the number of cells changed, so a caller can skip an undo entry
/// for a stroke that did nothing.
pub fn apply_channel_brush(
    values: &mut [u16],
    resolution: u32,
    element: ChannelElement,
    center: Vec2,
    radius: f32,
    falloff: f32,
    threshold: f32,
    value: u16,
) -> usize {
    if resolution == 0 || radius <= 0.0 {
        return 0;
    }
    let value = value.min(element.max_value());
    let res = resolution as i32;

    let min_x = ((center.x - radius).floor() as i32).clamp(0, res - 1);
    let max_x = ((center.x + radius).ceil() as i32).clamp(0, res - 1);
    let min_z = ((center.y - radius).floor() as i32).clamp(0, res - 1);
    let max_z = ((center.y + radius).ceil() as i32).clamp(0, res - 1);

    let mut changed = 0;
    for gz in min_z..=max_z {
        for gx in min_x..=max_x {
            let dist = ((gx as f32 - center.x).powi(2) + (gz as f32 - center.y).powi(2)).sqrt();
            if compute_falloff(dist, radius, falloff) < threshold.max(f32::EPSILON) {
                continue;
            }
            let idx = (gz * res + gx) as usize;
            if values[idx] != value {
                values[idx] = value;
                changed += 1;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every read of a mismatched channel is 0, including the ones whose
    /// index lands inside the stored values: at the caller's stride those
    /// name a different cell.
    #[test]
    fn a_resolution_the_stored_values_do_not_match_reads_zero() {
        let mut channel = ChannelData::new("mask", ChannelElement::U16, 4);
        channel.values[5] = 9;
        assert_eq!(channel.get(4, 1, 1), 9);
        assert_eq!(channel.get(8, 7, 7), 0);
        assert_eq!(channel.get(8, 1, 1), 0);
        assert_eq!(channel.get(2, 1, 1), 0);
    }

    #[test]
    fn element_widths_and_ceilings() {
        assert_eq!(ChannelElement::U8.byte_width(), 1);
        assert_eq!(ChannelElement::U8.max_value(), 255);
        assert_eq!(ChannelElement::U16.byte_width(), 2);
        assert_eq!(ChannelElement::U16.max_value(), 65535);
    }

    #[test]
    fn element_tags_round_trip() {
        for e in [ChannelElement::U8, ChannelElement::U16] {
            assert_eq!(ChannelElement::from_tag(e.tag()), Some(e));
        }
        assert_eq!(ChannelElement::from_tag(7), None);
    }

    #[test]
    fn new_channel_is_zeroed_at_the_right_length() {
        let c = ChannelData::new("biome", ChannelElement::U8, 8);
        assert_eq!(c.values.len(), 64);
        assert!(c.values.iter().all(|v| *v == 0));
    }

    #[test]
    fn resize_zero_fills_new_cells_and_keeps_old_ones() {
        let mut c = ChannelData::new("m", ChannelElement::U16, 2);
        c.values = vec![1, 2, 3, 4];
        c.resize_to(3);
        assert_eq!(c.values.len(), 9);
        assert_eq!(&c.values[..4], &[1, 2, 3, 4]);
        assert!(c.values[4..].iter().all(|v| *v == 0));
    }

    #[test]
    fn brush_stamps_a_disc_and_reports_changed_cells() {
        let mut values = vec![0u16; 100];
        let changed = apply_channel_brush(
            &mut values,
            10,
            ChannelElement::U8,
            Vec2::new(5.0, 5.0),
            2.0,
            1.0,
            0.0,
            3,
        );
        assert!(changed > 0);
        assert_eq!(values[5 * 10 + 5], 3);
        // A cell well outside the radius is untouched.
        assert_eq!(values[0], 0);
        // Painting the same value again changes nothing.
        let again = apply_channel_brush(
            &mut values,
            10,
            ChannelElement::U8,
            Vec2::new(5.0, 5.0),
            2.0,
            1.0,
            0.0,
            3,
        );
        assert_eq!(again, 0);
    }

    #[test]
    fn brush_clamps_value_to_the_element_ceiling() {
        let mut values = vec![0u16; 25];
        apply_channel_brush(
            &mut values,
            5,
            ChannelElement::U8,
            Vec2::new(2.0, 2.0),
            1.5,
            1.0,
            0.0,
            9000,
        );
        assert_eq!(values[2 * 5 + 2], 255);
    }

    #[test]
    fn brush_at_the_edge_does_not_wrap_or_panic() {
        let mut values = vec![0u16; 25];
        apply_channel_brush(
            &mut values,
            5,
            ChannelElement::U16,
            Vec2::new(0.0, 0.0),
            3.0,
            1.0,
            0.0,
            42,
        );
        assert_eq!(values[0], 42);
        // Nothing wrapped onto the opposite edge of row 0.
        assert_eq!(values[4], 0);
    }

    #[test]
    fn brush_outside_the_terrain_is_a_no_op() {
        let mut values = vec![0u16; 25];
        let changed = apply_channel_brush(
            &mut values,
            5,
            ChannelElement::U8,
            Vec2::new(50.0, 50.0),
            2.0,
            1.0,
            0.0,
            1,
        );
        assert_eq!(changed, 0);
        assert!(values.iter().all(|v| *v == 0));
    }

    #[test]
    fn a_high_threshold_writes_fewer_cells_than_a_low_one() {
        let mut soft = vec![0u16; 400];
        let wide = apply_channel_brush(
            &mut soft,
            20,
            ChannelElement::U8,
            Vec2::new(10.0, 10.0),
            6.0,
            1.0,
            0.0,
            1,
        );
        let mut hard = vec![0u16; 400];
        let narrow = apply_channel_brush(
            &mut hard,
            20,
            ChannelElement::U8,
            Vec2::new(10.0, 10.0),
            6.0,
            1.0,
            0.7,
            1,
        );
        assert!(narrow < wide, "narrow {narrow} should be under wide {wide}");
        assert!(narrow > 0);
    }
}
