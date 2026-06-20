//! Engine-agnostic snapping math.
//!
//! [`SnapSettings`] holds the grid power and the per-tool snap toggles and
//! increments, and computes snapped translation, rotation, and scale values.
//! The math here is pure arithmetic over [`glam`] vectors with no engine
//! dependency, so it can drive snapping in any host. The editor wraps this in
//! a Bevy resource newtype that derefs to it.

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// Lowest grid power offered by the editor's grid stepping. The grid size is
/// `2^GRID_POWER_MIN`.
pub const GRID_POWER_MIN: i32 = -5;
/// Highest grid power offered by the editor's grid stepping. The grid size is
/// `2^GRID_POWER_MAX`.
pub const GRID_POWER_MAX: i32 = 8;

/// Snap toggles, increments, and the grid power. The grid size is derived from
/// the power; the per-tool increments and flags drive the snap methods.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapSettings {
    pub translate_snap: bool,
    pub translate_increment: f32,
    pub rotate_snap: bool,
    pub rotate_increment: f32,
    pub scale_snap: bool,
    pub scale_increment: f32,
    /// Exponential grid power. Actual grid size = `2^grid_power`.
    pub grid_power: i32,
}

impl Default for SnapSettings {
    fn default() -> Self {
        let grid_power = -2;
        Self {
            // Snapping ships off; the viewport magnet toggle turns it on,
            // and Ctrl inverts it per operation (loop cut, slides, gizmo).
            translate_snap: false,
            translate_increment: 2.0_f32.powi(grid_power),
            rotate_snap: false,
            rotate_increment: 15.0_f32.to_radians(),
            scale_snap: false,
            scale_increment: 0.1,
            grid_power,
        }
    }
}

impl SnapSettings {
    /// Actual grid size derived from `grid_power`: `2^grid_power`.
    pub fn grid_size(&self) -> f32 {
        2.0_f32.powi(self.grid_power)
    }

    /// Snap a world position to the nearest grid line on each axis.
    /// Independent of the per-tool snap flags; callers gate on the
    /// relevant toggle (e.g. `scale_active`).
    pub fn snap_position_to_grid(&self, v: Vec3) -> Vec3 {
        let g = self.grid_size();
        if g > 0.0 {
            Vec3::new(
                (v.x / g).round() * g,
                (v.y / g).round() * g,
                (v.z / g).round() * g,
            )
        } else {
            v
        }
    }

    /// Snap a translation value to the nearest increment.
    pub fn snap_translate(&self, value: f32) -> f32 {
        if self.translate_snap && self.translate_increment > 0.0 {
            (value / self.translate_increment).round() * self.translate_increment
        } else {
            value
        }
    }

    /// Snap a translation vector.
    pub fn snap_translate_vec3(&self, v: Vec3) -> Vec3 {
        Vec3::new(
            self.snap_translate(v.x),
            self.snap_translate(v.y),
            self.snap_translate(v.z),
        )
    }

    /// Snap a rotation angle to the nearest increment.
    pub fn snap_rotate(&self, angle: f32) -> f32 {
        if self.rotate_snap && self.rotate_increment > 0.0 {
            (angle / self.rotate_increment).round() * self.rotate_increment
        } else {
            angle
        }
    }

    /// Snap a scale value to the nearest increment.
    pub fn snap_scale(&self, value: f32) -> f32 {
        if self.scale_snap && self.scale_increment > 0.0 {
            (value / self.scale_increment).round() * self.scale_increment
        } else {
            value
        }
    }

    /// Snap a scale vector.
    pub fn snap_scale_vec3(&self, v: Vec3) -> Vec3 {
        Vec3::new(
            self.snap_scale(v.x),
            self.snap_scale(v.y),
            self.snap_scale(v.z),
        )
    }

    /// Check if translate snapping should be active (Ctrl held = toggle snap).
    pub fn translate_active(&self, ctrl_held: bool) -> bool {
        self.translate_snap ^ ctrl_held
    }

    /// Check if rotate snapping should be active (Ctrl held = toggle snap).
    pub fn rotate_active(&self, ctrl_held: bool) -> bool {
        self.rotate_snap ^ ctrl_held
    }

    /// Check if scale snapping should be active (Ctrl held = toggle snap).
    pub fn scale_active(&self, ctrl_held: bool) -> bool {
        self.scale_snap ^ ctrl_held
    }

    /// Conditionally snap a translation vector based on Ctrl state.
    pub fn snap_translate_vec3_if(&self, v: Vec3, ctrl_held: bool) -> Vec3 {
        if self.translate_active(ctrl_held) && self.translate_increment > 0.0 {
            Vec3::new(
                (v.x / self.translate_increment).round() * self.translate_increment,
                (v.y / self.translate_increment).round() * self.translate_increment,
                (v.z / self.translate_increment).round() * self.translate_increment,
            )
        } else {
            v
        }
    }

    /// Conditionally snap a rotation angle based on Ctrl state.
    pub fn snap_rotate_if(&self, angle: f32, ctrl_held: bool) -> f32 {
        if self.rotate_active(ctrl_held) && self.rotate_increment > 0.0 {
            (angle / self.rotate_increment).round() * self.rotate_increment
        } else {
            angle
        }
    }

    /// Conditionally snap a scale vector based on Ctrl state.
    pub fn snap_scale_vec3_if(&self, v: Vec3, ctrl_held: bool) -> Vec3 {
        if self.scale_active(ctrl_held) && self.scale_increment > 0.0 {
            Vec3::new(
                (v.x / self.scale_increment).round() * self.scale_increment,
                (v.y / self.scale_increment).round() * self.scale_increment,
                (v.z / self.scale_increment).round() * self.scale_increment,
            )
        } else {
            v
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_size_is_two_to_the_power() {
        let mut s = SnapSettings {
            grid_power: 0,
            ..SnapSettings::default()
        };
        assert!((s.grid_size() - 1.0).abs() < 1e-6);
        s.grid_power = 3;
        assert!((s.grid_size() - 8.0).abs() < 1e-6);
        s.grid_power = -2;
        assert!((s.grid_size() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn translate_rounds_to_nearest_increment() {
        let mut s = SnapSettings {
            translate_snap: true,
            translate_increment: 1.0,
            ..SnapSettings::default()
        };
        assert!((s.snap_translate(0.4) - 0.0).abs() < 1e-6);
        assert!((s.snap_translate(0.6) - 1.0).abs() < 1e-6);
        assert!((s.snap_translate(2.5) - 3.0).abs() < 1e-6);

        // A different increment rounds to a different lattice.
        s.translate_increment = 0.25;
        assert!((s.snap_translate(0.3) - 0.25).abs() < 1e-6);
        assert!((s.snap_translate(0.4) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn translate_vec3_snaps_each_axis() {
        let s = SnapSettings {
            translate_snap: true,
            translate_increment: 1.0,
            ..SnapSettings::default()
        };
        let out = s.snap_translate_vec3(Vec3::new(0.6, 1.4, -0.6));
        assert!((out - Vec3::new(1.0, 1.0, -1.0)).length() < 1e-6);
    }

    #[test]
    fn rotate_rounds_to_nearest_increment() {
        let s = SnapSettings {
            rotate_snap: true,
            rotate_increment: 15.0_f32.to_radians(),
            ..SnapSettings::default()
        };
        // 20 degrees snaps up to 15-degree lattice -> 15 degrees.
        let out = s.snap_rotate(20.0_f32.to_radians());
        assert!((out - 15.0_f32.to_radians()).abs() < 1e-6);
        // 24 degrees snaps to 30 degrees.
        let out = s.snap_rotate(24.0_f32.to_radians());
        assert!((out - 30.0_f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn scale_rounds_to_nearest_increment() {
        let s = SnapSettings {
            scale_snap: true,
            scale_increment: 0.1,
            ..SnapSettings::default()
        };
        assert!((s.snap_scale(1.04) - 1.0).abs() < 1e-6);
        assert!((s.snap_scale(1.06) - 1.1).abs() < 1e-6);
    }

    #[test]
    fn snapping_off_passes_through() {
        let s = SnapSettings::default(); // all snap flags off
        assert!((s.snap_translate(0.37) - 0.37).abs() < 1e-6);
        assert!((s.snap_rotate(0.37) - 0.37).abs() < 1e-6);
        assert!((s.snap_scale(0.37) - 0.37).abs() < 1e-6);
    }

    #[test]
    fn position_to_grid_independent_of_snap_flags() {
        // snap_position_to_grid ignores the per-tool toggles.
        let s = SnapSettings {
            grid_power: 0, // grid size 1.0
            ..SnapSettings::default()
        };
        let out = s.snap_position_to_grid(Vec3::new(0.6, -0.6, 2.4));
        assert!((out - Vec3::new(1.0, -1.0, 2.0)).length() < 1e-6);
    }

    #[test]
    fn active_flags_xor_ctrl() {
        let mut s = SnapSettings::default();
        // Snap off: Ctrl turns it on.
        assert!(!s.translate_active(false));
        assert!(s.translate_active(true));
        // Snap on: Ctrl turns it off.
        s.translate_snap = true;
        assert!(!s.translate_active(true));
        assert!(s.translate_active(false));
    }

    #[test]
    fn conditional_translate_passes_through_when_inactive() {
        // Snap off and no Ctrl: value passes through unchanged.
        let s = SnapSettings {
            translate_snap: false,
            translate_increment: 1.0,
            ..SnapSettings::default()
        };
        let v = Vec3::new(0.37, 1.42, -2.61);
        assert!((s.snap_translate_vec3_if(v, false) - v).length() < 1e-6);
        // Ctrl held flips it on, so it snaps.
        let out = s.snap_translate_vec3_if(v, true);
        assert!((out - Vec3::new(0.0, 1.0, -3.0)).length() < 1e-6);
    }

    #[test]
    fn conditional_rotate_and_scale_respect_ctrl() {
        // Rotate: snap off, Ctrl held -> snaps.
        let s = SnapSettings {
            rotate_snap: false,
            rotate_increment: 15.0_f32.to_radians(),
            scale_snap: false,
            scale_increment: 0.1,
            ..SnapSettings::default()
        };
        let r = 20.0_f32.to_radians();
        assert!((s.snap_rotate_if(r, false) - r).abs() < 1e-6);
        assert!((s.snap_rotate_if(r, true) - 15.0_f32.to_radians()).abs() < 1e-6);

        let v = Vec3::new(1.06, 1.04, 0.94);
        assert!((s.snap_scale_vec3_if(v, false) - v).length() < 1e-6);
        let out = s.snap_scale_vec3_if(v, true);
        assert!((out - Vec3::new(1.1, 1.0, 0.9)).length() < 1e-6);
    }
}
