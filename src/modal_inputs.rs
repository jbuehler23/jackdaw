//! BEI input-action types for modal interactions, navigation, and viewport
//! pointer gestures, plus the polling `SystemParam`s that expose their
//! just-fired signals.
//!
//! # `ModalInputs` `SystemParam`
//!
//! [`ModalInputs`] exposes `confirm()`, `cancel()`, etc. as just-fired
//! booleans for polling modal systems. The mechanism is pull-style polling of
//! the [`ActionEvents`] component on each action entity, which BEI updates
//! every `PreUpdate` frame (see `EnhancedInputSystems::Update`).
//!
//! Specifically, `confirm()` and `cancel()` return `true` when
//! `ActionEvents::START` is set, meaning the key transitioned from not-pressed
//! to pressed on this frame. `step_up()` and `step_down()` return `true` on
//! `ActionEvents::FIRE` because `ScrollTick` fires once per wheel event,
//! making `FIRE` the correct "just happened" signal for scroll. `axis_x/y/z`
//! return `true` on `ActionEvents::START` (first press of the axis key).
//!
//! Held keys cannot leak into the modal context because all modal actions set
//! `require_reset = true`. Re-inserting `Binding` on each INACTIVE -> ACTIVE
//! transition re-arms BEI's `FirstActivation` flag, so keys held from before
//! each modal session are ignored until released.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Action, ActionEvents, InputAction};

/// Marker on each of the seven modal action entities (confirm, cancel,
/// `axis_x`/`y`/`z`, `step_up`, `step_down`).
///
/// `update_modal_context_activity` queries for binding entities whose
/// [`BindingOf`](bevy_enhanced_input::prelude::BindingOf) target carries this
/// marker, and re-inserts their
/// [`Binding`](bevy_enhanced_input::prelude::Binding) component on every
/// INACTIVE -> ACTIVE transition so that BEI's `FirstActivation` flag is reset
/// for the new modal session.
#[derive(Component)]
pub struct ModalAction;

// ---- Action types ----

/// Confirm the active modal interaction (default: Enter).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalConfirm;

/// Cancel the active modal interaction (default: Escape).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalCancel;

/// Constrain modal to the X axis (default: X key).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalAxisX;

/// Constrain modal to the Y axis (default: Y key).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalAxisY;

/// Constrain modal to the Z axis (default: Z key).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalAxisZ;

/// Step the active modal value upward (default: scroll up).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalStepUp;

/// Step the active modal value downward (default: scroll down).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct ModalStepDown;

/// Hold to enter fly-navigation mode (right mouse button, `Down` condition).
///
/// Bound code-level with `Binding::MouseButton { Right }` + `Down` so fly is
/// active every frame RMB is held. This action is NOT registered in
/// `BuiltinActions` and carries no `DefaultKeymap` entry: the binding is
/// hard-wired here, not preset-bindable.
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct NavFly;

/// Mouse motion delta while navigating.
///
/// Bound code-level with `Binding::mouse_motion()`. Not preset-bindable:
/// axis feeds drive a continuous value; only the activation chord (RMB) is
/// the user-rebindable part, and that is gated by `NavFly.fly_active` in the
/// populate system.
#[derive(InputAction, Default)]
#[action_output(Vec2)]
pub struct NavLook;

/// One scroll-up tick while in the nav context.
///
/// Bound code-level with `Binding::mouse_wheel()` + `ScrollTick::new(true)`.
/// Registered as `"nav.brush_resize_up"` in `BuiltinActions` and
/// `DefaultKeymap` so keymap presets can rebind the resize chord. The
/// terrain sculpt system gates on Shift held in keyboard, matching the
/// pre-BEI behavior where scroll-resize and camera zoom coexist via the
/// Shift guard in `jackdaw_camera` (the camera skips zoom when Shift is
/// down; the resize system only fires when Shift is down).
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct NavBrushResizeUp;

/// One scroll-down tick while in the nav context.
///
/// Mirror of [`NavBrushResizeUp`] for the downward direction.
/// Registered as `"nav.brush_resize_down"`.
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct NavBrushResizeDown;

/// LMB press/hold while in the pointer context.
///
/// Bound code-level with `Binding::MouseButton { Left }` + `Down::default()`
/// so BEI tracks the full press/hold cycle. The `Down` condition means the
/// action value is `true` every frame LMB is held; `ActionEvents::START` fires
/// only on the first held frame (same-frame as `ButtonInput::just_pressed`).
/// [`PointerInputs`] exposes the start edge via `pointer_primary_just_pressed()`.
///
/// LMB release checks and drag/threshold logic in invoke-trigger systems
/// keep reading raw `ButtonInput<MouseButton>` directly; only the initial
/// press edge is routed here. Preset-bindability of LMB itself is not
/// meaningful until click-vs-drag semantics live in the binding layer.
#[derive(InputAction, Default)]
#[action_output(bool)]
pub struct PointerPrimary;

// ---- ModalInputs SystemParam ----

/// System parameter exposing just-fired signals from the modal actions.
///
/// The mechanism is pull-style polling of the [`ActionEvents`] component on
/// each action entity. BEI updates `ActionEvents` every `PreUpdate` in
/// `EnhancedInputSystems::Update`. Systems that run in `Update` (after
/// `PreUpdate`) see a stable value for the whole frame.
///
/// - `confirm`, `cancel`, `axis_x/y/z`: return `true` on `ActionEvents::START`
///   (key transition from not-pressed to pressed; correct for one-shot actions).
/// - `step_up`, `step_down`: return `true` on `ActionEvents::FIRE` (each
///   `ScrollTick` fires once, so FIRE is the right per-tick signal).
#[derive(SystemParam)]
pub struct ModalInputs<'w, 's> {
    confirm_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalConfirm>>>,
    cancel_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalCancel>>>,
    axis_x_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalAxisX>>>,
    axis_y_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalAxisY>>>,
    axis_z_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalAxisZ>>>,
    step_up_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalStepUp>>>,
    step_down_q: Query<'w, 's, &'static ActionEvents, With<Action<ModalStepDown>>>,
}

impl<'w, 's> ModalInputs<'w, 's> {
    fn any_start(q: &Query<'_, '_, &ActionEvents, impl bevy::ecs::query::QueryFilter>) -> bool {
        q.iter().any(|e| e.contains(ActionEvents::START))
    }

    fn any_fire(q: &Query<'_, '_, &ActionEvents, impl bevy::ecs::query::QueryFilter>) -> bool {
        q.iter().any(|e| e.contains(ActionEvents::FIRE))
    }

    /// Returns `true` on the first frame the confirm key (Enter) is pressed.
    pub fn confirm(&self) -> bool {
        Self::any_start(&self.confirm_q)
    }

    /// Returns `true` on the first frame the cancel key (Escape) is pressed.
    pub fn cancel(&self) -> bool {
        Self::any_start(&self.cancel_q)
    }

    /// Returns `true` on the first frame the X-axis key is pressed.
    pub fn axis_x(&self) -> bool {
        Self::any_start(&self.axis_x_q)
    }

    /// Returns `true` on the first frame the Y-axis key is pressed.
    pub fn axis_y(&self) -> bool {
        Self::any_start(&self.axis_y_q)
    }

    /// Returns `true` on the first frame the Z-axis key is pressed.
    pub fn axis_z(&self) -> bool {
        Self::any_start(&self.axis_z_q)
    }

    /// Returns `true` each frame a scroll-up tick fires.
    pub fn step_up(&self) -> bool {
        Self::any_fire(&self.step_up_q)
    }

    /// Returns `true` each frame a scroll-down tick fires.
    pub fn step_down(&self) -> bool {
        Self::any_fire(&self.step_down_q)
    }
}

// ---- NavScrollInputs SystemParam ----

/// System parameter exposing per-tick brush-resize scroll signals from the
/// nav context.
///
/// Uses the same pull-style `ActionEvents::FIRE` mechanism as `ModalInputs`
/// step signals: `ScrollTick` fires once per wheel event, so `FIRE` is the
/// correct per-tick signal. Systems run in `Update` after BEI's `PreUpdate`
/// evaluation; `ActionEvents` is stable for the whole `Update` phase.
#[derive(SystemParam)]
pub struct NavScrollInputs<'w, 's> {
    resize_up_q: Query<'w, 's, &'static ActionEvents, With<Action<NavBrushResizeUp>>>,
    resize_down_q: Query<'w, 's, &'static ActionEvents, With<Action<NavBrushResizeDown>>>,
}

impl<'w, 's> NavScrollInputs<'w, 's> {
    /// Returns `true` each frame a scroll-up tick fires.
    pub fn resize_up(&self) -> bool {
        self.resize_up_q
            .iter()
            .any(|e| e.contains(ActionEvents::FIRE))
    }

    /// Returns `true` each frame a scroll-down tick fires.
    pub fn resize_down(&self) -> bool {
        self.resize_down_q
            .iter()
            .any(|e| e.contains(ActionEvents::FIRE))
    }
}

// ---- PointerInputs SystemParam ----

/// System parameter exposing the LMB press edge from the pointer context.
///
/// `pointer_primary_just_pressed()` returns `true` on the first frame LMB is
/// pressed, matching the semantics of `ButtonInput::just_pressed(MouseButton::Left)`.
///
/// The edge is detected via `ActionEvents::START`: BEI sets START on the frame
/// the `Down` condition transitions from not-met to met, which is the same
/// frame `ButtonInput` reports `just_pressed`. Both are updated before `Update`
/// runs (`ButtonInput` in `First`, BEI in `PreUpdate` via `EnhancedInputSystems::Update`);
/// systems reading either signal in `Update` observe the same press on the same frame.
///
/// `ActionEvents::START` is already a single-frame flag (set only when the action
/// state transitions to active this frame); no previous-frame mirror is needed.
#[derive(SystemParam)]
pub struct PointerInputs<'w, 's> {
    primary_q: Query<'w, 's, &'static ActionEvents, With<Action<PointerPrimary>>>,
}

impl<'w, 's> PointerInputs<'w, 's> {
    /// Returns `true` on the first frame LMB is pressed (rising edge).
    pub fn pointer_primary_just_pressed(&self) -> bool {
        self.primary_q
            .iter()
            .any(|e| e.contains(ActionEvents::START))
    }
}
