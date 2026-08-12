//! Packed per-cell control value: which textures a splat cell paints with.
//!
//! `Terrain3D`'s proven encoding, adopted so the format scales past 16
//! textures without one weight map per texture. The bit layout is a
//! PERSISTED CONTRACT -- it is written verbatim into sidecar v3 region
//! data, so changing it breaks every terrain saved with the old layout.
//!
//! ```text
//! bit     width   field
//! 0       5       base texture id, 0..=31
//! 5       5       overlay texture id, 0..=31
//! 10      6       blend, 0..=63 (0 = pure base, 63 = pure overlay)
//! 16      16      reserved (future: holes, autoshader flags), must be 0
//! ```
//!
//! Call sites never shift or mask a raw `u32` themselves; they go through
//! the typed accessors below, which read and write only their own field
//! and leave every other bit -- including the reserved range -- untouched.

const BASE_SHIFT: u32 = 0;
const BASE_MASK: u32 = 0x1F;
const OVERLAY_SHIFT: u32 = 5;
const OVERLAY_MASK: u32 = 0x1F;
const BLEND_SHIFT: u32 = 10;
const BLEND_MASK: u32 = 0x3F;
const RESERVED_SHIFT: u32 = 16;
const RESERVED_MASK: u32 = 0xFFFF;

/// Largest value the base/overlay texture id field can hold.
pub const MAX_TEXTURE_ID: u8 = BASE_MASK as u8;
/// Largest value the blend field can hold.
pub const MAX_BLEND: u8 = BLEND_MASK as u8;

/// A packed per-cell control value: base texture, overlay texture, blend.
///
/// The raw `u32` is the on-disk representation exactly; [`Control::from_raw`]
/// and [`Control::to_raw`] are lossless and the only way in or out of the
/// packed form. Every other constructor and accessor goes through the typed
/// field methods so a call site can never touch the wrong bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Control(u32);

impl Control {
    /// Wrap a raw packed value. Does not validate the reserved bits; a
    /// decoder that must refuse unknown reserved bits checks
    /// [`Control::reserved`] itself before trusting the rest.
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// The raw packed value, exactly as it would be written to a sidecar.
    pub const fn to_raw(self) -> u32 {
        self.0
    }

    /// Base texture id, `0..=31`.
    pub fn base_id(self) -> u8 {
        ((self.0 >> BASE_SHIFT) & BASE_MASK) as u8
    }

    /// Set the base texture id, clamping to `0..=31` rather than wrapping.
    /// Every other field, including reserved bits, is left unchanged.
    #[must_use]
    pub fn with_base_id(self, id: u8) -> Self {
        let id = id.min(MAX_TEXTURE_ID) as u32;
        Self((self.0 & !(BASE_MASK << BASE_SHIFT)) | (id << BASE_SHIFT))
    }

    /// Overlay texture id, `0..=31`.
    pub fn overlay_id(self) -> u8 {
        ((self.0 >> OVERLAY_SHIFT) & OVERLAY_MASK) as u8
    }

    /// Set the overlay texture id, clamping to `0..=31` rather than
    /// wrapping. Every other field, including reserved bits, is left
    /// unchanged.
    #[must_use]
    pub fn with_overlay_id(self, id: u8) -> Self {
        let id = id.min(MAX_TEXTURE_ID) as u32;
        Self((self.0 & !(OVERLAY_MASK << OVERLAY_SHIFT)) | (id << OVERLAY_SHIFT))
    }

    /// Blend between base and overlay, `0..=63` (0 = pure base, 63 = pure
    /// overlay).
    pub fn blend(self) -> u8 {
        ((self.0 >> BLEND_SHIFT) & BLEND_MASK) as u8
    }

    /// Set the blend value, clamping to `0..=63` rather than wrapping.
    /// Every other field, including reserved bits, is left unchanged.
    #[must_use]
    pub fn with_blend(self, blend: u8) -> Self {
        let blend = blend.min(MAX_BLEND) as u32;
        Self((self.0 & !(BLEND_MASK << BLEND_SHIFT)) | (blend << BLEND_SHIFT))
    }

    /// The reserved 16 bits, unshifted. Zero on every control value this
    /// build writes; a decoder rejects a file where this is nonzero,
    /// because it means something a future build understands and this one
    /// does not.
    pub fn reserved(self) -> u16 {
        ((self.0 >> RESERVED_SHIFT) & RESERVED_MASK) as u16
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_all_zero() {
        assert_eq!(Control::default().to_raw(), 0);
        assert_eq!(Control::default().base_id(), 0);
        assert_eq!(Control::default().overlay_id(), 0);
        assert_eq!(Control::default().blend(), 0);
        assert_eq!(Control::default().reserved(), 0);
    }

    #[test]
    fn from_raw_to_raw_round_trips() {
        for raw in [0u32, 1, 0xFFFF_FFFF, 0xDEAD_BEEF, 0x1234_5678] {
            assert_eq!(Control::from_raw(raw).to_raw(), raw);
        }
    }

    #[test]
    fn base_id_round_trips_at_field_boundaries() {
        for id in [0u8, 1, 30, 31] {
            let c = Control::default().with_base_id(id);
            assert_eq!(c.base_id(), id);
            assert_eq!(c.overlay_id(), 0);
            assert_eq!(c.blend(), 0);
            assert_eq!(c.reserved(), 0);
        }
    }

    #[test]
    fn overlay_id_round_trips_at_field_boundaries() {
        for id in [0u8, 1, 30, 31] {
            let c = Control::default().with_overlay_id(id);
            assert_eq!(c.overlay_id(), id);
            assert_eq!(c.base_id(), 0);
            assert_eq!(c.blend(), 0);
            assert_eq!(c.reserved(), 0);
        }
    }

    #[test]
    fn blend_round_trips_at_field_boundaries() {
        for blend in [0u8, 1, 62, 63] {
            let c = Control::default().with_blend(blend);
            assert_eq!(c.blend(), blend);
            assert_eq!(c.base_id(), 0);
            assert_eq!(c.overlay_id(), 0);
            assert_eq!(c.reserved(), 0);
        }
    }

    #[test]
    fn out_of_range_ids_and_blend_clamp_rather_than_wrap() {
        assert_eq!(Control::default().with_base_id(32).base_id(), 31);
        assert_eq!(Control::default().with_base_id(255).base_id(), 31);
        assert_eq!(Control::default().with_overlay_id(200).overlay_id(), 31);
        assert_eq!(Control::default().with_blend(64).blend(), 63);
        assert_eq!(Control::default().with_blend(255).blend(), 63);
    }

    #[test]
    fn fields_do_not_overlap_at_max_values() {
        let c = Control::default()
            .with_base_id(31)
            .with_overlay_id(31)
            .with_blend(63);
        assert_eq!(c.base_id(), 31);
        assert_eq!(c.overlay_id(), 31);
        assert_eq!(c.blend(), 63);
        assert_eq!(c.reserved(), 0);
        // Every occupied bit sits in 0..16; nothing has leaked upward.
        assert_eq!(c.to_raw(), 0x0000_FFFF);
    }

    #[test]
    fn setters_preserve_reserved_bits_already_present() {
        let with_reserved = Control::from_raw(0xBEEF_0000);
        assert_eq!(with_reserved.reserved(), 0xBEEF);

        let edited = with_reserved
            .with_base_id(5)
            .with_overlay_id(9)
            .with_blend(20);
        assert_eq!(edited.reserved(), 0xBEEF);
        assert_eq!(edited.base_id(), 5);
        assert_eq!(edited.overlay_id(), 9);
        assert_eq!(edited.blend(), 20);
    }

    #[test]
    fn setters_only_touch_their_own_field() {
        let c = Control::default().with_base_id(3).with_overlay_id(7);
        let blended = c.with_blend(11);
        assert_eq!(blended.base_id(), 3);
        assert_eq!(blended.overlay_id(), 7);
        assert_eq!(blended.blend(), 11);
    }
}
