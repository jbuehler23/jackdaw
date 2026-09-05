//! Packed per-cell control value: base texture id, overlay texture id, and
//! the blend between them. Scales past 16 textures without one weight map
//! per texture.
//!
//! The bit layout is a persisted contract: written verbatim into sidecar
//! format version 2 region data. Changing it breaks every terrain already
//! saved.
//!
//! ```text
//! bit     width   field
//! 0       5       base texture id, 0..=31
//! 5       5       overlay texture id, 0..=31
//! 10      8       blend, 0..=255 (0 = pure base, 255 = pure overlay)
//! 18      1       manual: a hand painted this cell
//! 19      13      reserved, must be 0
//! ```
//!
//! Call sites use the typed accessors below rather than shifting or
//! masking a raw `u32`; each accessor touches only its own field, leaving
//! every other bit -- including reserved -- unchanged. `Control` is
//! `#[repr(transparent)]`, so a region's control layer stores `[Control]`
//! directly rather than `[u32]`.
//!
//! # The manual bit
//!
//! `manual` says a person painted this cell, so whatever it names is what
//! it draws. A cell without it is textured from the geometry instead --
//! see the autoterrain settings in [`crate::sidecar`]. A word written
//! before this bit was assigned carries `manual = 0`, which is ambiguous:
//! a cell painted to base 0, overlay 0, blend 0 is bit-for-bit an
//! unpainted cell and reads as unclaimed, and the two cannot be told
//! apart. Autoterrain is off per terrain until somebody turns it on, so
//! the ambiguity only reaches such a terrain once its author opts in. The
//! paint brush sets the bit on every cell it touches.

use bevy_math::Vec2;

use crate::brush::compute_falloff;

// Crate-visible because the splat shader hand-writes the same six
// numbers, and its pin test compares them against these.
pub(crate) const BASE_SHIFT: u32 = 0;
pub(crate) const BASE_MASK: u32 = 0x1F;
pub(crate) const OVERLAY_SHIFT: u32 = 5;
pub(crate) const OVERLAY_MASK: u32 = 0x1F;
pub(crate) const BLEND_SHIFT: u32 = 10;
pub(crate) const BLEND_MASK: u32 = 0xFF;
const MANUAL_SHIFT: u32 = 18;
const MANUAL_MASK: u32 = 0x1;
const RESERVED_SHIFT: u32 = 19;
const RESERVED_MASK: u32 = 0x1FFF;

/// Largest value the base/overlay texture id field can hold.
pub const MAX_TEXTURE_ID: u8 = BASE_MASK as u8;
/// Largest value the blend field can hold.
pub const MAX_BLEND: u8 = BLEND_MASK as u8;
/// The manual bit in place, for a shader or an exporter that has to test
/// it against a raw word rather than through [`Control::manual`].
pub const MANUAL_BIT: u32 = MANUAL_MASK << MANUAL_SHIFT;

/// A packed per-cell control value: base texture, overlay texture, blend.
///
/// The raw `u32` is the on-disk representation exactly; [`Control::from_raw`]
/// and [`Control::to_raw`] are lossless and the only way in or out of the
/// packed form. Every other constructor and accessor goes through the typed
/// field methods so a call site can never touch the wrong bits.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Control(u32);

impl Control {
    /// Wrap a raw packed value. Does not validate reserved bits; callers
    /// that must reject unknown reserved bits check [`Control::reserved`].
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    /// The raw packed value, as written to a sidecar.
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

    /// Blend between base and overlay, `0..=255` (0 = pure base, 255 = pure
    /// overlay).
    pub fn blend(self) -> u8 {
        ((self.0 >> BLEND_SHIFT) & BLEND_MASK) as u8
    }

    /// Set the blend value (no clamping needed; the field is a full `u8`).
    /// Every other field, including reserved bits, is left unchanged.
    #[must_use]
    pub fn with_blend(self, blend: u8) -> Self {
        let blend = blend as u32;
        Self((self.0 & !(BLEND_MASK << BLEND_SHIFT)) | (blend << BLEND_SHIFT))
    }

    /// Whether a hand painted this cell. A cell without it is textured
    /// from the geometry wherever a terrain has autoterrain on.
    pub fn manual(self) -> bool {
        (self.0 >> MANUAL_SHIFT) & MANUAL_MASK != 0
    }

    /// Claim or release the cell. Every other field, including reserved
    /// bits, is left unchanged -- releasing a cell keeps the ids and blend
    /// it was painted with, so re-claiming it restores exactly that.
    #[must_use]
    pub fn with_manual(self, manual: bool) -> Self {
        let bit = u32::from(manual) << MANUAL_SHIFT;
        Self((self.0 & !(MANUAL_MASK << MANUAL_SHIFT)) | bit)
    }

    /// The reserved 13 bits, unshifted. Must be 0; decode rejects a
    /// nonzero value.
    pub fn reserved(self) -> u16 {
        ((self.0 >> RESERVED_SHIFT) & RESERVED_MASK) as u16
    }
}

/// Paint a texture id into a control-word layer under a circular brush.
///
/// Unlike [`crate::channel::apply_channel_brush`]'s threshold stamp, blend
/// is continuous: each call nudges it by a falloff-scaled amount rather
/// than snapping it, so a soft brush edge stays visible in the painted
/// result and a held stroke ramps up smoothly across repeated calls.
///
/// - Primary (`secondary = false`) sets the cell's base id to `texture_id`
///   and lowers blend toward 0 (pure base).
/// - Secondary (`secondary = true` -- the modifier-held stroke) sets the
///   cell's overlay id to `texture_id` and raises blend toward
///   [`MAX_BLEND`] (pure overlay).
///
/// Either stroke claims every cell it touches: [`Control::manual`] goes on
/// and stays on until [`apply_restore_brush`] releases it.
///
/// `center` and `radius` are in grid cells. `opacity` is blend range
/// crossed per second at full brush strength, scaled here by frame `dt`,
/// so a slow machine and a fast one paint the same stroke at the same
/// rate. Returns the number of cells changed, so a
/// caller can skip an undo entry for a stroke that did nothing.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors apply_brush/apply_channel_brush's flat parameter list; a settings \
              struct would just move the same fields without reducing what a caller must supply"
)]
pub fn apply_control_brush(
    control: &mut [Control],
    resolution: u32,
    center: Vec2,
    radius: f32,
    falloff: f32,
    opacity: f32,
    dt: f32,
    secondary: bool,
    texture_id: u8,
) -> usize {
    // Mirrors `apply_brush`'s guard: a non-finite radius, opacity or dt
    // must not poison the control map with NaN-derived deltas.
    if resolution == 0
        || !radius.is_finite()
        || radius <= 0.0
        || !opacity.is_finite()
        || opacity <= 0.0
        || !dt.is_finite()
        || dt <= 0.0
    {
        return 0;
    }
    let texture_id = texture_id.min(MAX_TEXTURE_ID);
    let res = resolution as i32;

    let min_x = ((center.x - radius).floor() as i32).clamp(0, res - 1);
    let max_x = ((center.x + radius).ceil() as i32).clamp(0, res - 1);
    let min_z = ((center.y - radius).floor() as i32).clamp(0, res - 1);
    let max_z = ((center.y + radius).ceil() as i32).clamp(0, res - 1);

    let mut changed = 0;
    for gz in min_z..=max_z {
        for gx in min_x..=max_x {
            let dist = ((gx as f32 - center.x).powi(2) + (gz as f32 - center.y).powi(2)).sqrt();
            let f = compute_falloff(dist, radius, falloff);
            if f <= 0.0 {
                continue;
            }
            let idx = (gz * res + gx) as usize;
            let before = control[idx];
            let delta = (opacity * f * dt * MAX_BLEND as f32).round() as i32;
            let after = if secondary {
                let blend = (before.blend() as i32 + delta).clamp(0, MAX_BLEND as i32) as u8;
                before.with_overlay_id(texture_id).with_blend(blend)
            } else {
                let blend = (before.blend() as i32 - delta).clamp(0, MAX_BLEND as i32) as u8;
                before.with_base_id(texture_id).with_blend(blend)
            }
            .with_manual(true);
            if after != before {
                control[idx] = after;
                changed += 1;
            }
        }
    }
    changed
}

/// Release cells back to autoterrain under a circular brush.
///
/// The inverse of what [`apply_control_brush`] claims, and only that: the
/// base id, overlay id and blend a released cell was painted with are left
/// as they are, so re-claiming the cell restores the same picture.
///
/// A binary flip, so this stamps like [`crate::channel::apply_channel_brush`]
/// rather than accumulating like the paint brush: a cell whose falloff
/// clears `threshold` is released outright. Returns the number of cells
/// changed, so a caller can skip an undo entry for a stroke that released
/// nothing.
pub fn apply_restore_brush(
    control: &mut [Control],
    resolution: u32,
    center: Vec2,
    radius: f32,
    falloff: f32,
    threshold: f32,
) -> usize {
    if resolution == 0 || !radius.is_finite() || radius <= 0.0 {
        return 0;
    }
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
            let before = control[idx];
            if !before.manual() {
                continue;
            }
            control[idx] = before.with_manual(false);
            changed += 1;
        }
    }
    changed
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
        assert!(!Control::default().manual());
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
        for blend in [0u8, 1, 254, 255] {
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
        // blend fills a full u8; nothing to clamp.
        assert_eq!(MAX_BLEND, u8::MAX);
    }

    #[test]
    fn fields_do_not_overlap_at_max_values() {
        let c = Control::default()
            .with_base_id(31)
            .with_overlay_id(31)
            .with_blend(255);
        assert_eq!(c.base_id(), 31);
        assert_eq!(c.overlay_id(), 31);
        assert_eq!(c.blend(), 255);
        assert!(!c.manual());
        assert_eq!(c.reserved(), 0);
        // Every occupied bit sits in 0..18; nothing has leaked upward.
        assert_eq!(c.to_raw(), 0x0003_FFFF);
    }

    /// The manual bit is one bit directly above blend, and the three
    /// fields below it are unreachable from it.
    #[test]
    fn manual_occupies_the_single_bit_above_blend() {
        let claimed = Control::default().with_manual(true);
        assert_eq!(claimed.to_raw(), MANUAL_BIT);
        assert_eq!(MANUAL_BIT, 1 << 18);
        assert!(claimed.manual());
        assert_eq!(claimed.base_id(), 0);
        assert_eq!(claimed.overlay_id(), 0);
        assert_eq!(claimed.blend(), 0);
        assert_eq!(claimed.reserved(), 0);

        let full = Control::default()
            .with_base_id(31)
            .with_overlay_id(31)
            .with_blend(255)
            .with_manual(true);
        assert_eq!(full.to_raw(), 0x0007_FFFF);
        assert_eq!(full.reserved(), 0);
    }

    /// Releasing a cell keeps the paint it was released from, so
    /// claiming it again draws what it drew.
    #[test]
    fn manual_round_trips_without_touching_the_paint_under_it() {
        let painted = Control::default()
            .with_base_id(7)
            .with_overlay_id(3)
            .with_blend(90)
            .with_manual(true);
        let released = painted.with_manual(false);
        assert!(!released.manual());
        assert_eq!(released.base_id(), 7);
        assert_eq!(released.overlay_id(), 3);
        assert_eq!(released.blend(), 90);
        assert_eq!(released.with_manual(true), painted);
    }

    #[test]
    fn setters_preserve_the_manual_bit_and_reserved_bits_already_present() {
        let with_reserved = Control::from_raw(0x1555 << RESERVED_SHIFT).with_manual(true);
        assert_eq!(with_reserved.reserved(), 0x1555);

        let edited = with_reserved
            .with_base_id(5)
            .with_overlay_id(9)
            .with_blend(200);
        assert_eq!(edited.reserved(), 0x1555);
        assert!(edited.manual(), "editing a field must not release the cell");
        assert_eq!(edited.base_id(), 5);
        assert_eq!(edited.overlay_id(), 9);
        assert_eq!(edited.blend(), 200);
        assert_eq!(edited.with_manual(false).reserved(), 0x1555);
    }

    #[test]
    fn setters_only_touch_their_own_field() {
        let c = Control::default().with_base_id(3).with_overlay_id(7);
        let blended = c.with_blend(11);
        assert_eq!(blended.base_id(), 3);
        assert_eq!(blended.overlay_id(), 7);
        assert_eq!(blended.blend(), 11);
    }

    #[test]
    fn is_repr_transparent_over_u32() {
        assert_eq!(core::mem::size_of::<Control>(), core::mem::size_of::<u32>());
        assert_eq!(
            core::mem::align_of::<Control>(),
            core::mem::align_of::<u32>()
        );
    }

    // --- apply_control_brush ---

    #[test]
    fn primary_sets_base_id_and_lowers_blend_toward_zero() {
        let mut control = vec![Control::default().with_overlay_id(2).with_blend(200); 25];
        let changed = apply_control_brush(
            &mut control,
            5,
            Vec2::new(2.0, 2.0),
            2.0,
            1.0,
            1.0,
            1.0,
            false,
            3,
        );
        assert!(changed > 0);
        let cell = control[2 * 5 + 2];
        assert_eq!(cell.base_id(), 3);
        assert!(cell.blend() < 200, "primary must lower blend toward base");
    }

    #[test]
    fn secondary_sets_overlay_id_and_raises_blend_toward_max() {
        let mut control = vec![Control::default().with_base_id(1); 25];
        let changed = apply_control_brush(
            &mut control,
            5,
            Vec2::new(2.0, 2.0),
            2.0,
            1.0,
            1.0,
            1.0,
            true,
            4,
        );
        assert!(changed > 0);
        let cell = control[2 * 5 + 2];
        assert_eq!(cell.overlay_id(), 4);
        assert!(
            cell.blend() > 0,
            "secondary must raise blend toward overlay"
        );
    }

    #[test]
    fn a_cell_outside_the_radius_is_untouched() {
        let mut control = vec![Control::default(); 25];
        apply_control_brush(
            &mut control,
            5,
            Vec2::new(2.0, 2.0),
            1.0,
            1.0,
            1.0,
            1.0,
            false,
            3,
        );
        assert_eq!(control[0], Control::default());
    }

    #[test]
    fn blend_clamps_rather_than_wrapping_at_either_end() {
        let mut control = vec![Control::default(); 25];
        // Primary drive on an already-zero blend must not wrap negative.
        apply_control_brush(
            &mut control,
            5,
            Vec2::new(2.0, 2.0),
            2.0,
            1.0,
            10.0,
            1.0,
            false,
            1,
        );
        assert_eq!(control[2 * 5 + 2].blend(), 0);

        let mut control = vec![Control::default(); 25];
        // Secondary drive from zero must not overshoot past MAX_BLEND.
        apply_control_brush(
            &mut control,
            5,
            Vec2::new(2.0, 2.0),
            2.0,
            1.0,
            10.0,
            1.0,
            true,
            1,
        );
        assert_eq!(control[2 * 5 + 2].blend(), MAX_BLEND);
    }

    #[test]
    fn repeated_calls_ramp_the_blend_up_across_a_held_stroke() {
        let mut control = vec![Control::default(); 25];
        let center = Vec2::new(2.0, 2.0);
        apply_control_brush(&mut control, 5, center, 2.0, 1.0, 0.1, 1.0, true, 2);
        let first = control[2 * 5 + 2].blend();
        apply_control_brush(&mut control, 5, center, 2.0, 1.0, 0.1, 1.0, true, 2);
        let second = control[2 * 5 + 2].blend();
        assert!(second > first, "a held stroke must keep raising blend");
    }

    #[test]
    fn texture_id_clamps_to_max_texture_id() {
        let mut control = vec![Control::default(); 25];
        apply_control_brush(
            &mut control,
            5,
            Vec2::new(2.0, 2.0),
            2.0,
            1.0,
            1.0,
            1.0,
            false,
            250,
        );
        assert_eq!(control[2 * 5 + 2].base_id(), MAX_TEXTURE_ID);
    }

    #[test]
    fn a_non_finite_opacity_leaves_the_control_map_untouched() {
        let mut control = vec![Control::default(); 25];
        let changed = apply_control_brush(
            &mut control,
            5,
            Vec2::new(2.0, 2.0),
            2.0,
            1.0,
            f32::NAN,
            1.0,
            false,
            1,
        );
        assert_eq!(changed, 0);
        assert!(control.iter().all(|c| *c == Control::default()));
    }

    /// Base and overlay naming the same texture is a degenerate but legal
    /// word, and must still come out in range rather than stuck.
    #[test]
    fn painting_overlay_matching_the_existing_base_id_still_produces_a_sane_word() {
        let mut control = vec![Control::default().with_base_id(5); 25];
        let changed = apply_control_brush(
            &mut control,
            5,
            Vec2::new(2.0, 2.0),
            2.0,
            1.0,
            1.0,
            1.0,
            true,
            5,
        );
        assert!(changed > 0);
        let cell = control[2 * 5 + 2];
        assert_eq!(
            cell.base_id(),
            5,
            "a secondary stroke never touches base_id"
        );
        assert_eq!(
            cell.overlay_id(),
            5,
            "overlay is set to the painted id even though it now equals base"
        );
        assert!(
            cell.blend() > 0,
            "blend still moves toward overlay even though base and overlay now agree"
        );
    }

    /// Same degenerate case from the primary side: painting the base with
    /// the id the cell's overlay already carries.
    #[test]
    fn painting_base_matching_the_existing_overlay_id_still_produces_a_sane_word() {
        let mut control = vec![Control::default().with_overlay_id(5).with_blend(MAX_BLEND); 25];
        let changed = apply_control_brush(
            &mut control,
            5,
            Vec2::new(2.0, 2.0),
            2.0,
            1.0,
            1.0,
            1.0,
            false,
            5,
        );
        assert!(changed > 0);
        let cell = control[2 * 5 + 2];
        assert_eq!(
            cell.overlay_id(),
            5,
            "a primary stroke never touches overlay_id"
        );
        assert_eq!(
            cell.base_id(),
            5,
            "base is set to the painted id even though it now equals overlay"
        );
        assert!(
            cell.blend() < MAX_BLEND,
            "blend still moves toward base even though base and overlay now agree"
        );
    }

    #[test]
    fn a_degenerate_resolution_or_radius_is_a_no_op_rather_than_panicking() {
        let mut control = vec![Control::default(); 4];
        assert_eq!(
            apply_control_brush(&mut control, 0, Vec2::ZERO, 2.0, 1.0, 1.0, 1.0, false, 1),
            0
        );
        assert_eq!(
            apply_control_brush(&mut control, 2, Vec2::ZERO, 0.0, 1.0, 1.0, 1.0, false, 1),
            0
        );
    }

    /// Both strokes claim what they touch, and a cell outside the brush
    /// is left free.
    #[test]
    fn either_stroke_claims_every_cell_it_paints() {
        for secondary in [false, true] {
            let mut control = vec![Control::default(); 25];
            apply_control_brush(
                &mut control,
                5,
                Vec2::new(2.0, 2.0),
                2.0,
                1.0,
                1.0,
                1.0,
                secondary,
                3,
            );
            assert!(
                control[2 * 5 + 2].manual(),
                "secondary {secondary}: a painted cell is claimed"
            );
            assert!(
                !control[0].manual(),
                "secondary {secondary}: a cell outside the brush stays unclaimed"
            );
        }
    }

    /// A cell already carrying the paint the brush would lay down is
    /// still claimed, so the stroke is not a no-op.
    #[test]
    fn painting_a_cell_that_already_looks_right_still_claims_it() {
        let mut control = vec![Control::default(); 25];
        let changed = apply_control_brush(
            &mut control,
            5,
            Vec2::new(2.0, 2.0),
            2.0,
            1.0,
            1.0,
            1.0,
            false,
            0,
        );
        assert!(changed > 0);
        assert!(control[2 * 5 + 2].manual());
    }

    // --- apply_restore_brush ---

    #[test]
    fn restoring_releases_claimed_cells_and_leaves_their_paint_alone() {
        let painted = Control::default()
            .with_base_id(4)
            .with_overlay_id(6)
            .with_blend(180)
            .with_manual(true);
        let mut control = vec![painted; 25];

        let changed = apply_restore_brush(&mut control, 5, Vec2::new(2.0, 2.0), 2.0, 1.0, 0.5);

        assert!(changed > 0);
        let cell = control[2 * 5 + 2];
        assert!(!cell.manual(), "the cell is back to autoterrain");
        assert_eq!(cell.base_id(), 4);
        assert_eq!(cell.overlay_id(), 6);
        assert_eq!(cell.blend(), 180);
    }

    #[test]
    fn restoring_leaves_cells_outside_the_brush_claimed() {
        let mut control = vec![Control::default().with_manual(true); 25];
        apply_restore_brush(&mut control, 5, Vec2::new(2.0, 2.0), 1.0, 1.0, 0.5);
        assert!(control[0].manual());
    }

    /// A stroke over ground no hand has claimed reports no change,
    /// so it leaves no undo entry behind.
    #[test]
    fn restoring_unclaimed_ground_changes_nothing() {
        let mut control = vec![Control::default(); 25];
        let changed = apply_restore_brush(&mut control, 5, Vec2::new(2.0, 2.0), 2.0, 1.0, 0.5);
        assert_eq!(changed, 0);
        assert!(control.iter().all(|c| *c == Control::default()));
    }

    #[test]
    fn a_degenerate_restore_brush_is_a_no_op_rather_than_panicking() {
        let mut control = vec![Control::default().with_manual(true); 4];
        assert_eq!(
            apply_restore_brush(&mut control, 0, Vec2::ZERO, 2.0, 1.0, 0.5),
            0
        );
        assert_eq!(
            apply_restore_brush(&mut control, 2, Vec2::ZERO, 0.0, 1.0, 0.5),
            0
        );
        assert_eq!(
            apply_restore_brush(&mut control, 2, Vec2::ZERO, f32::NAN, 1.0, 0.5),
            0
        );
    }
}
