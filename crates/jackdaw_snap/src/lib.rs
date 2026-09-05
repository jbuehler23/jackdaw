//! Engine-agnostic snapping math.
//!
//! [`SnapSettings`] holds the grid size and the per-tool snap toggles and
//! increments, and computes snapped translation, rotation, and scale values.
//! The math here is pure arithmetic over [`glam`] vectors with no engine
//! dependency, so it can drive snapping in any host. The editor wraps this in
//! a Bevy resource newtype that derefs to it.

use glam::{Vec2, Vec3};
use serde::{Deserialize, Serialize};

/// Lowest grid power offered by the editor's grid stepping. The grid size is
/// `2^GRID_POWER_MIN`.
pub const GRID_POWER_MIN: i32 = -5;
/// Highest grid power offered by the editor's grid stepping. The grid size is
/// `2^GRID_POWER_MAX`.
pub const GRID_POWER_MAX: i32 = 8;

/// Snap toggles, increments, and the grid size. The grid size is the
/// explicit [`SnapSettings::grid_increment`] when one is set and the
/// power-of-two ladder otherwise; the per-tool increments and flags drive
/// the snap methods.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SnapSettings {
    pub translate_snap: bool,
    pub translate_increment: f32,
    pub rotate_snap: bool,
    pub rotate_increment: f32,
    pub scale_snap: bool,
    pub scale_increment: f32,
    /// Exponential grid power. The grid size is `2^grid_power` unless
    /// [`SnapSettings::grid_increment`] overrides it.
    pub grid_power: i32,
    /// Explicit grid size in world units, or `0.0` for "derived from
    /// `grid_power`".
    ///
    /// The power ladder is the right control for a world with no metric
    /// of its own -- halving and doubling is how you find a working grid
    /// by feel. A game built on 1.5 m or 2.5 m cells has already
    /// decided, and no power of two will ever be its cell. This is the
    /// way to say the number outright.
    ///
    /// `#[serde(default)]` so settings without this field keep loading;
    /// the zero default falls back to the `2^grid_power` ladder.
    #[serde(default)]
    pub grid_increment: f32,
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
            grid_increment: 0.0,
        }
    }
}

impl SnapSettings {
    /// The grid size in world units.
    ///
    /// An explicit [`SnapSettings::grid_increment`] wins: the two
    /// controls are two ways of saying the same thing, so the one the
    /// user touched last is the one that holds. Zero, negative and
    /// non-finite increments fall back to the `2^grid_power` ladder
    /// rather than yielding a grid nothing can snap to.
    pub fn grid_size(&self) -> f32 {
        if self.grid_increment.is_finite() && self.grid_increment > 0.0 {
            return self.grid_increment;
        }
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

    /// Snap a translation vector on a plane.
    pub fn snap_translate_vec2(&self, v: Vec2) -> Vec2 {
        Vec2::new(self.snap_translate(v.x), self.snap_translate(v.y))
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

/// The rect a 2D edge snap is moving, in whatever units the candidates
/// are stated in.
///
/// Not an engine rect: this crate is arithmetic over [`glam`] and nothing
/// else, so the caller converts on the way in.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SnapRect {
    pub min: Vec2,
    pub max: Vec2,
}

impl SnapRect {
    pub fn from_min_size(min: Vec2, size: Vec2) -> Self {
        Self {
            min,
            max: min + size,
        }
    }

    /// The three coordinates that can land on a candidate: the two
    /// edges and the midpoint between them.
    fn snap_lines(&self) -> [Vec2; 3] {
        [self.min, (self.min + self.max) / 2.0, self.max]
    }
}

/// Which of a moving rect's three lines landed on a candidate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SnapLine {
    /// The near edge: left, or top.
    Min,
    /// The midpoint between the two edges.
    Mid,
    /// The far edge: right, or bottom.
    Max,
}

impl SnapLine {
    /// The three lines in the order [`SnapRect::snap_lines`] reports them.
    const ALL: [Self; 3] = [Self::Min, Self::Mid, Self::Max];
}

/// What one axis of a snap landed on.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct AxisSnap {
    /// How far the axis has to move for the landing.
    pub delta: f32,
    /// The line of the moving rect that landed.
    pub line: SnapLine,
    /// Index into the candidate slice the landing was on.
    pub candidate: usize,
}

/// Offset that puts one of `moving`'s edges (or its centre) onto the
/// nearest candidate line within `threshold`, per axis.
///
/// The two axes are decided independently, so a rect can take its x from
/// one neighbour and its y from another, and an axis with no candidate
/// in range contributes zero rather than pulling toward the origin. A
/// non-positive or non-finite threshold means no snapping at all.
pub fn snap_edges_2d(
    moving: SnapRect,
    candidates_x: &[f32],
    candidates_y: &[f32],
    threshold: f32,
) -> Vec2 {
    let (x, y) = snap_edges_2d_with_winners(moving, candidates_x, candidates_y, threshold);
    Vec2::new(
        x.map_or(0.0, |snap| snap.delta),
        y.map_or(0.0, |snap| snap.delta),
    )
}

/// [`snap_edges_2d`] with the landings themselves rather than only the
/// offset they imply, for a caller that has to say what was landed on.
pub fn snap_edges_2d_with_winners(
    moving: SnapRect,
    candidates_x: &[f32],
    candidates_y: &[f32],
    threshold: f32,
) -> (Option<AxisSnap>, Option<AxisSnap>) {
    let lines = moving.snap_lines();
    (
        nearest_candidate(
            [lines[0].x, lines[1].x, lines[2].x],
            candidates_x,
            threshold,
        ),
        nearest_candidate(
            [lines[0].y, lines[1].y, lines[2].y],
            candidates_y,
            threshold,
        ),
    )
}

/// The candidate one of `lines` comes nearest to inside `threshold`, or
/// `None` when none of them is in range.
///
/// Candidates are tried in the order given and only a strictly smaller
/// distance displaces the one already held, so two candidates the same
/// distance away resolve to the earlier one: the caller's order is its
/// precedence.
pub fn nearest_candidate(lines: [f32; 3], candidates: &[f32], threshold: f32) -> Option<AxisSnap> {
    if threshold <= 0.0 || !threshold.is_finite() {
        return None;
    }
    let mut best: Option<AxisSnap> = None;
    let mut best_abs = f32::INFINITY;
    for (index, candidate) in candidates.iter().enumerate() {
        for (ordinal, line) in SnapLine::ALL.into_iter().enumerate() {
            let delta = candidate - lines[ordinal];
            let abs = delta.abs();
            if abs <= threshold && abs < best_abs {
                best_abs = abs;
                best = Some(AxisSnap {
                    delta,
                    line,
                    candidate: index,
                });
            }
        }
    }
    best
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
    fn an_explicit_increment_beats_the_power() {
        let s = SnapSettings {
            grid_power: 3, // would be 8.0
            grid_increment: 1.5,
            ..SnapSettings::default()
        };
        assert!((s.grid_size() - 1.5).abs() < 1e-6);
    }

    #[test]
    fn an_increment_of_zero_falls_back_to_the_power() {
        let mut s = SnapSettings {
            grid_power: 3,
            grid_increment: 0.0,
            ..SnapSettings::default()
        };
        assert!((s.grid_size() - 8.0).abs() < 1e-6);

        // So does anything that is not a usable interval.
        for bad in [-1.5, f32::NAN, f32::INFINITY] {
            s.grid_increment = bad;
            assert!((s.grid_size() - 8.0).abs() < 1e-6, "increment {bad}");
        }
    }

    #[test]
    fn a_one_and_a_half_metre_grid_snaps_to_its_own_lattice() {
        let s = SnapSettings {
            grid_increment: 1.5,
            ..SnapSettings::default()
        };
        // 2.2 is nearer 1.5 than 3.0; 2.3 is nearer 3.0.
        assert!((s.snap_position_to_grid(Vec3::splat(2.2)) - Vec3::splat(1.5)).length() < 1e-5);
        assert!((s.snap_position_to_grid(Vec3::splat(2.3)) - Vec3::splat(3.0)).length() < 1e-5);
    }

    #[test]
    fn the_increment_ships_off_so_the_power_still_rules() {
        let s = SnapSettings::default();
        assert_eq!(s.grid_increment, 0.0);
        assert!((s.grid_size() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn settings_round_trip_through_serde() {
        let s = SnapSettings {
            grid_power: 1,
            grid_increment: 2.5,
            translate_snap: true,
            ..SnapSettings::default()
        };
        let json = serde_json::to_string(&s).expect("serialize");
        let back: SnapSettings = serde_json::from_str(&json).expect("deserialize");
        assert!(back == s);
        assert!((back.grid_size() - 2.5).abs() < 1e-6);
    }

    #[test]
    fn settings_written_before_the_increment_existed_still_load() {
        // Exactly the field set an earlier build wrote.
        let legacy = r#"{
            "translate_snap": false,
            "translate_increment": 0.25,
            "rotate_snap": false,
            "rotate_increment": 0.2617994,
            "scale_snap": false,
            "scale_increment": 0.1,
            "grid_power": -2
        }"#;
        let loaded: SnapSettings = serde_json::from_str(legacy).expect("deserialize legacy");
        assert_eq!(loaded.grid_increment, 0.0);
        assert!((loaded.grid_size() - 0.25).abs() < 1e-6);
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
    fn translate_vec2_snaps_each_axis() {
        let s = SnapSettings {
            translate_snap: true,
            translate_increment: 1.0,
            ..SnapSettings::default()
        };
        let out = s.snap_translate_vec2(Vec2::new(0.6, -0.6));
        assert!((out - Vec2::new(1.0, -1.0)).length() < 1e-6);

        // Off, it passes straight through, like every other snap here.
        let off = SnapSettings::default();
        let v = Vec2::new(0.37, 1.42);
        assert!((off.snap_translate_vec2(v) - v).length() < 1e-6);
    }

    #[test]
    fn an_edge_inside_the_threshold_pulls_the_rect_onto_it() {
        // Left edge at 98, a candidate at 100: three pixels of pull.
        let moving = SnapRect::from_min_size(Vec2::new(98.0, 40.0), Vec2::new(50.0, 20.0));
        let out = snap_edges_2d(moving, &[100.0], &[], 6.0);
        assert!((out - Vec2::new(2.0, 0.0)).length() < 1e-6);

        // The right edge counts too: 148 is two short of 150.
        let out = snap_edges_2d(moving, &[150.0], &[], 6.0);
        assert!((out - Vec2::new(2.0, 0.0)).length() < 1e-6);

        // And so does the centre: 123 against 120.
        let out = snap_edges_2d(moving, &[120.0], &[], 6.0);
        assert!((out - Vec2::new(-3.0, 0.0)).length() < 1e-6);
    }

    #[test]
    fn the_nearest_candidate_wins_on_each_axis() {
        let moving = SnapRect::from_min_size(Vec2::new(98.0, 41.0), Vec2::new(50.0, 20.0));
        // 100 is 2 away from the left edge; 96 is 2 away too but 152 is
        // only 4 from the right edge and 100 stays the smaller pull.
        let out = snap_edges_2d(moving, &[100.0, 152.0], &[40.0, 44.0], 6.0);
        assert!(
            (out - Vec2::new(2.0, -1.0)).length() < 1e-6,
            "{out:?} should take the smallest pull per axis",
        );
    }

    #[test]
    fn a_candidate_outside_the_threshold_moves_nothing() {
        let moving = SnapRect::from_min_size(Vec2::new(98.0, 40.0), Vec2::new(50.0, 20.0));
        assert_eq!(snap_edges_2d(moving, &[120.0], &[400.0], 1.0), Vec2::ZERO);
        // An axis with no candidates at all stays put rather than
        // collapsing toward zero.
        assert_eq!(snap_edges_2d(moving, &[], &[], 6.0), Vec2::ZERO);
        // A threshold of zero is "no snapping", not "snap to anything".
        assert_eq!(snap_edges_2d(moving, &[100.0], &[40.0], 0.0), Vec2::ZERO);
    }

    /// The offset alone says a rect moved; it does not say what it came
    /// to rest against. A caller drawing the line it landed on needs the
    /// candidate and the line that reached it.
    #[test]
    fn the_nearest_candidate_names_the_line_and_the_candidate_it_used() {
        // Lines at 98 (min), 123 (mid) and 148 (max).
        let lines = [98.0, 123.0, 148.0];

        let landed = nearest_candidate(lines, &[400.0, 100.0], 6.0).expect("100 is two away");
        assert_eq!(landed.line, SnapLine::Min);
        assert_eq!(landed.candidate, 1, "the second candidate is what it used");
        assert!((landed.delta - 2.0).abs() < 1e-6);

        // The centre and the far edge are landings of their own.
        let landed = nearest_candidate(lines, &[120.0], 6.0).expect("120 is three from the mid");
        assert_eq!(landed.line, SnapLine::Mid);
        assert!((landed.delta + 3.0).abs() < 1e-6);

        let landed = nearest_candidate(lines, &[150.0], 6.0).expect("150 is two from the max");
        assert_eq!(landed.line, SnapLine::Max);
        assert!((landed.delta - 2.0).abs() < 1e-6);

        // Nothing in range is no landing, not a landing of zero.
        assert_eq!(nearest_candidate(lines, &[400.0], 6.0), None);
        assert_eq!(nearest_candidate(lines, &[100.0], 0.0), None);
    }

    /// The candidate order is the caller's precedence: it lists the
    /// lines it wants a drag to prefer first, and a tie has to go that
    /// way rather than to whichever of the rect's own lines came first.
    #[test]
    fn the_first_of_two_equidistant_candidates_wins() {
        // The mid line is 10 from 110 and the min line is 10 from 88:
        // the same distance, so only the candidate order decides.
        let lines = [98.0, 120.0, 142.0];

        let first = nearest_candidate(lines, &[110.0, 88.0], 12.0).expect("both are in range");
        assert_eq!(first.candidate, 0);
        assert_eq!(first.line, SnapLine::Mid);

        let swapped = nearest_candidate(lines, &[88.0, 110.0], 12.0).expect("both are in range");
        assert_eq!(swapped.candidate, 0);
        assert_eq!(swapped.line, SnapLine::Min);
    }

    /// The plain offset call is the winner call with the landings
    /// dropped, so the two can never disagree about how far a drag
    /// moves.
    #[test]
    fn the_delta_wrapper_agrees_with_the_winner() {
        let moving = SnapRect::from_min_size(Vec2::new(98.0, 41.0), Vec2::new(50.0, 20.0));
        for candidates in [
            (&[100.0, 152.0][..], &[40.0, 44.0][..]),
            (&[400.0][..], &[44.0][..]),
            (&[][..], &[][..]),
        ] {
            let (x, y) = snap_edges_2d_with_winners(moving, candidates.0, candidates.1, 6.0);
            let delta = snap_edges_2d(moving, candidates.0, candidates.1, 6.0);
            assert_eq!(delta.x, x.map_or(0.0, |snap| snap.delta));
            assert_eq!(delta.y, y.map_or(0.0, |snap| snap.delta));
        }
    }

    #[test]
    fn each_axis_snaps_independently() {
        let moving = SnapRect::from_min_size(Vec2::new(98.0, 40.0), Vec2::new(50.0, 20.0));
        // x has a candidate in range, y does not.
        let out = snap_edges_2d(moving, &[100.0], &[400.0], 6.0);
        assert!((out - Vec2::new(2.0, 0.0)).length() < 1e-6);
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
