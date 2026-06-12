//! Input conditions the preset format needs beyond the built-ins.
//!
//! - [`DoubleClick`]: fires on the second press edge within a configurable
//!   time window; resets after firing so a third press starts a new pair.
//! - [`ScrollTick`]: fires once per evaluation tick whose vertical scroll
//!   sign matches the configured direction; ignores zero and opposite-sign.

use core::time::Duration;

use bevy::prelude::*;
use bevy_enhanced_input::prelude::*;

/// Default double-click window in seconds.
pub const DEFAULT_DOUBLE_CLICK_WINDOW: f32 = 0.3;

/// Fires [`TriggerState::Fired`] exactly on the second press edge within
/// [`window`](DoubleClick::window) seconds of the first.
///
/// The first press alone returns [`TriggerState::Ongoing`] while the window
/// is still open, [`TriggerState::None`] once it expires.  After firing the
/// internal state resets: the very next press starts a new pair rather than
/// immediately re-firing.
///
/// Window timing uses the real clock as an absolute timestamp, so the pair
/// survives evaluation gaps (inactive contexts) without drifting.
///
/// If another action consumes this binding's input mid-press, the zero read
/// registers as a release edge; acceptable for double-click semantics.
#[derive(Component, Debug, Clone)]
pub struct DoubleClick {
    /// Maximum gap between the two press edges, in seconds.
    pub window: f32,

    /// Actuation threshold forwarded to [`ActionValue::is_actuated`].
    pub actuation: f32,

    actuated: bool,
    /// Absolute real-clock timestamp of the first press edge, stored as
    /// [`Duration`] from [`Time<Real>::elapsed`].  `None` means no first
    /// press is pending.
    first_press_at: Option<Duration>,
}

impl DoubleClick {
    /// Creates an instance with the given window (seconds) and the default
    /// actuation threshold.
    #[must_use]
    pub fn new(window: f32) -> Self {
        Self {
            window,
            actuation: 0.5,
            actuated: false,
            first_press_at: None,
        }
    }

    /// Override the actuation threshold.
    #[must_use]
    pub fn with_actuation(mut self, actuation: f32) -> Self {
        self.actuation = actuation;
        self
    }
}

impl Default for DoubleClick {
    fn default() -> Self {
        Self::new(DEFAULT_DOUBLE_CLICK_WINDOW)
    }
}

impl InputCondition for DoubleClick {
    fn evaluate(
        &mut self,
        _actions: &ActionsQuery,
        time: &ContextTime,
        value: ActionValue,
    ) -> TriggerState {
        let previously_actuated = self.actuated;
        self.actuated = value.is_actuated(self.actuation);

        let press_edge = self.actuated && !previously_actuated;
        let now = time.real.elapsed();
        let window = Duration::from_secs_f32(self.window);

        match self.first_press_at {
            None => {
                if press_edge {
                    // Record the first press timestamp and start waiting.
                    self.first_press_at = Some(now);
                    TriggerState::Ongoing
                } else {
                    TriggerState::None
                }
            }
            Some(first_at) => {
                let gap = now.saturating_sub(first_at);

                if press_edge {
                    if gap <= window {
                        // Second press arrived within the window: fire and reset
                        // so the next press starts a fresh pair.
                        self.first_press_at = None;
                        TriggerState::Fired
                    } else {
                        // Too late; treat this press as the new first press.
                        self.first_press_at = Some(now);
                        TriggerState::Ongoing
                    }
                } else if gap > window {
                    // Window expired without a second press.
                    self.first_press_at = None;
                    TriggerState::None
                } else {
                    // Still inside the window, waiting.
                    TriggerState::Ongoing
                }
            }
        }
    }
}

/// Fires [`TriggerState::Fired`] once per evaluation tick when the vertical
/// scroll value matches the configured direction.
///
/// `positive = true` fires on upward scroll (Y > 0); `positive = false` fires
/// on downward scroll (Y < 0).  Zero values and ticks in the opposite direction
/// return [`TriggerState::None`].  Multi-unit scroll deltas (e.g. value 2.0)
/// fire exactly once per tick, not once per unit.
///
/// This condition is always paired with a [`Binding::MouseWheel`] binding,
/// which produces [`ActionValue::Axis2D`] where Y is the vertical axis.
/// The `phase` field in a `PresetBinding` is ignored for Scroll entries; this
/// condition replaces any phase condition entirely.
///
/// Smooth-scroll devices emit small deltas across many frames; each evaluation
/// with a matching sign fires once, so flicks produce bursts.  Acceptable for
/// continuous targets (brush resize); discrete targets should accumulate.
#[derive(Component, Debug, Clone, Copy)]
pub struct ScrollTick {
    /// `true` fires on upward ticks (Y > 0); `false` fires on downward ticks
    /// (Y < 0).
    pub positive: bool,
}

impl ScrollTick {
    #[must_use]
    pub const fn new(positive: bool) -> Self {
        Self { positive }
    }
}

impl InputCondition for ScrollTick {
    fn evaluate(
        &mut self,
        _actions: &ActionsQuery,
        _time: &ContextTime,
        value: ActionValue,
    ) -> TriggerState {
        let y = value.as_axis2d().y;
        let fires = if self.positive { y > 0.0 } else { y < 0.0 };
        if fires {
            TriggerState::Fired
        } else {
            TriggerState::None
        }
    }
}

#[cfg(test)]
mod tests {
    use core::time::Duration;

    use bevy::ecs::system::SystemState;
    use bevy::prelude::*;
    use bevy_enhanced_input::prelude::*;

    use super::*;

    /// Mirror of `bevy_enhanced_input::context::init_world`, which is
    /// `pub(crate)` in BEI and therefore not accessible from here.
    fn init_world<'w, 's>() -> (World, SystemState<(ContextTime<'w>, ActionsQuery<'w, 's>)>) {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<Time<Real>>();
        let state = SystemState::<(ContextTime, ActionsQuery)>::new(&mut world);
        (world, state)
    }

    // DoubleClick tests

    #[test]
    fn double_click_first_press_returns_ongoing() {
        let (world, mut state) = init_world();
        let (time, actions) = state.get(&world);
        let mut cond = DoubleClick::new(0.3);

        // First press edge: should be Ongoing, not Fired.
        assert_eq!(
            cond.evaluate(&actions, &time, ActionValue::Bool(true)),
            TriggerState::Ongoing,
        );
    }

    #[test]
    fn double_click_fires_on_second_press_within_window() {
        let (mut world, mut state) = init_world();
        let (time, actions) = state.get(&world);
        let mut cond = DoubleClick::new(0.3);

        // First press.
        let _ = cond.evaluate(&actions, &time, ActionValue::Bool(true));
        // Release.
        let _ = cond.evaluate(&actions, &time, ActionValue::Bool(false));

        // A tiny tick so the window timer advances a little (still inside).
        world
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(100));
        let (time, actions) = state.get(&world);

        // Second press within the window: must fire.
        assert_eq!(
            cond.evaluate(&actions, &time, ActionValue::Bool(true)),
            TriggerState::Fired,
        );
    }

    #[test]
    fn double_click_does_not_fire_when_spaced_beyond_window() {
        let (mut world, mut state) = init_world();
        let (time, actions) = state.get(&world);
        let mut cond = DoubleClick::new(0.3);

        // First press.
        let _ = cond.evaluate(&actions, &time, ActionValue::Bool(true));

        // Advance well past the window.
        world
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(400));
        let (time, actions) = state.get(&world);

        // Second press should NOT fire because the window already expired.
        let result = cond.evaluate(&actions, &time, ActionValue::Bool(true));
        assert_ne!(
            result,
            TriggerState::Fired,
            "should not fire when second press is beyond the window"
        );
    }

    #[test]
    fn double_click_resets_after_fire() {
        let (mut world, mut state) = init_world();
        let (time, actions) = state.get(&world);
        let mut cond = DoubleClick::new(0.3);

        // First press.
        let _ = cond.evaluate(&actions, &time, ActionValue::Bool(true));
        // Release.
        let _ = cond.evaluate(&actions, &time, ActionValue::Bool(false));

        world
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(50));
        let (time, actions) = state.get(&world);

        // Second press fires.
        let fired = cond.evaluate(&actions, &time, ActionValue::Bool(true));
        assert_eq!(fired, TriggerState::Fired);

        // Release after the double-click.
        let _ = cond.evaluate(&actions, &time, ActionValue::Bool(false));

        world
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_millis(10));
        let (time, actions) = state.get(&world);

        // Third press: state was reset, so this is a NEW first press (Ongoing),
        // not an immediate re-fire.
        let after_reset = cond.evaluate(&actions, &time, ActionValue::Bool(true));
        assert_eq!(
            after_reset,
            TriggerState::Ongoing,
            "after firing, the next press should start a new pair (Ongoing)"
        );
    }

    #[test]
    fn double_click_no_fire_on_zero_value() {
        let (world, mut state) = init_world();
        let (time, actions) = state.get(&world);
        let mut cond = DoubleClick::new(0.3);

        assert_eq!(
            cond.evaluate(&actions, &time, ActionValue::Bool(false)),
            TriggerState::None,
        );
    }

    // ScrollTick tests

    #[test]
    fn scroll_tick_fires_on_positive_y() {
        let (world, mut state) = init_world();
        let (time, actions) = state.get(&world);
        let mut cond = ScrollTick::new(true);

        assert_eq!(
            cond.evaluate(&actions, &time, ActionValue::Axis2D(Vec2::new(0.0, 1.0))),
            TriggerState::Fired,
        );
    }

    #[test]
    fn scroll_tick_does_not_fire_on_negative_y_when_positive() {
        let (world, mut state) = init_world();
        let (time, actions) = state.get(&world);
        let mut cond = ScrollTick::new(true);

        assert_eq!(
            cond.evaluate(&actions, &time, ActionValue::Axis2D(Vec2::new(0.0, -1.0))),
            TriggerState::None,
        );
    }

    #[test]
    fn scroll_tick_fires_on_negative_y_when_negative() {
        let (world, mut state) = init_world();
        let (time, actions) = state.get(&world);
        let mut cond = ScrollTick::new(false);

        assert_eq!(
            cond.evaluate(&actions, &time, ActionValue::Axis2D(Vec2::new(0.0, -2.0))),
            TriggerState::Fired,
        );
    }

    #[test]
    fn scroll_tick_does_not_fire_on_zero() {
        let (world, mut state) = init_world();
        let (time, actions) = state.get(&world);
        let mut cond = ScrollTick::new(true);

        assert_eq!(
            cond.evaluate(&actions, &time, ActionValue::Axis2D(Vec2::ZERO)),
            TriggerState::None,
        );
    }

    #[test]
    fn scroll_tick_fires_once_per_evaluation_not_per_unit() {
        let (world, mut state) = init_world();
        let (time, actions) = state.get(&world);
        let mut cond = ScrollTick::new(true);

        // A large delta of 5.0 should still fire exactly once.
        assert_eq!(
            cond.evaluate(&actions, &time, ActionValue::Axis2D(Vec2::new(0.0, 5.0))),
            TriggerState::Fired,
        );
    }
}
