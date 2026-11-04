//! Input-action markers for modal interactions, navigation, and viewport
//! pointer gestures, plus the polling `SystemParam`s that expose their
//! just-fired signals.
//!
//! # `ModalInputs` `SystemParam`
//!
//! [`ModalInputs`] exposes `confirm()`, `cancel()`, etc. as just-fired
//! booleans for polling modal systems. The mechanism is pull-style polling of
//! the [`ActionEvents`] component on each action entity, which is updated
//! every `PreUpdate` frame in `ActionSystems`.
//!
//! Specifically, `confirm()` and `cancel()` return `true` when
//! `ActionEvents::START` is set, meaning the key transitioned from not-pressed
//! to pressed on this frame. `step_up()` and `step_down()` return `true` on
//! `ActionEvents::FIRE` because a scroll tick fires once per scrolling frame,
//! making `FIRE` the correct "just happened" signal for scroll. `axis_x/y/z`
//! return `true` on `ActionEvents::START` (first press of the axis key).
//!
//! Held keys cannot leak into the modal context because all modal actions set
//! `require_reset = true`, so keys held from before each modal session are
//! ignored until released.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use jackdaw_input::ActionEvents;

pub use jackdaw_api_internal::tools::{
    ModalAction, ModalAxisX, ModalAxisY, ModalAxisZ, ModalCancel, ModalConfirm, ModalInputs,
    ModalStepDown, ModalStepUp,
};

/// Hold to enter fly-navigation mode (right mouse button, `Down` condition).
///
/// Bound code-level with `Binding::MouseButton { Right }` + `Down` so fly is
/// active every frame RMB is held. This action is NOT registered in
/// `BuiltinActions` and carries no `DefaultKeymap` entry: the binding is
/// hard-wired here, not preset-bindable.
#[derive(Component, Default)]
pub struct NavFly;

/// Mouse motion delta while navigating.
///
/// Bound code-level with `Binding::mouse_motion()`. Not preset-bindable:
/// axis feeds drive a continuous value; only the activation chord (RMB) is
/// the user-rebindable part, and that is gated by `NavFly.fly_active` in the
/// populate system.
#[derive(Component, Default)]
pub struct NavLook;

/// One scroll-up tick while in the nav context.
///
/// Bound code-level with `Binding::mouse_wheel()` + `Condition::ScrollTick { up: true }`.
/// Registered as `"nav.brush_resize_up"` in `BuiltinActions` and
/// `DefaultKeymap` so keymap presets can rebind the resize chord. The
/// terrain sculpt system gates on Shift held in keyboard, matching the
/// earlier behavior where scroll-resize and camera zoom coexist via the
/// Shift guard in `jackdaw_camera` (the camera skips zoom when Shift is
/// down; the resize system only fires when Shift is down).
#[derive(Component, Default)]
pub struct NavBrushResizeUp;

/// One scroll-down tick while in the nav context.
///
/// Mirror of [`NavBrushResizeUp`] for the downward direction.
/// Registered as `"nav.brush_resize_down"`.
#[derive(Component, Default)]
pub struct NavBrushResizeDown;

/// LMB press/hold while in the pointer context.
///
/// Bound code-level with `Binding::MouseButton { Left }` + `Condition::Down`
/// so the action tracks the full press/hold cycle. The `Down` condition means the
/// action value is `true` every frame LMB is held; `ActionEvents::START` fires
/// only on the first held frame (same-frame as `ButtonInput::just_pressed`).
/// [`PointerInputs`] exposes the start edge via `pointer_primary_just_pressed()`.
///
/// LMB release checks and drag/threshold logic in invoke-trigger systems
/// keep reading raw `ButtonInput<MouseButton>` directly; only the initial
/// press edge is routed here. Preset-bindability of LMB itself is not
/// meaningful until click-vs-drag semantics live in the binding layer.
#[derive(Component, Default)]
pub struct PointerPrimary;

// ---- ModalInputs SystemParam ----

// ---- NavScrollInputs SystemParam ----

/// System parameter exposing per-tick brush-resize scroll signals from the
/// nav context.
///
/// Uses the same pull-style `ActionEvents::FIRE` mechanism as `ModalInputs`
/// step signals: a scroll tick fires once per scrolling frame, so `FIRE` is
/// the correct per-tick signal. Systems run in `Update` after the `PreUpdate`
/// evaluation; `ActionEvents` is stable for the whole `Update` phase.
#[derive(SystemParam)]
pub struct NavScrollInputs<'w, 's> {
    resize_up_q: Query<'w, 's, &'static ActionEvents, With<NavBrushResizeUp>>,
    resize_down_q: Query<'w, 's, &'static ActionEvents, With<NavBrushResizeDown>>,
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
/// The edge is detected via `ActionEvents::START`, set on the frame the
/// `Down` condition transitions from not-met to met, which is the same
/// frame `ButtonInput` reports `just_pressed`. Both are updated before `Update`
/// runs (`ButtonInput` in `PreUpdate`'s `InputSystems`, actions after it in `ActionSystems`);
/// systems reading either signal in `Update` observe the same press on the same frame.
///
/// `ActionEvents::START` is already a single-frame flag (set only when the action
/// state transitions to active this frame); no previous-frame mirror is needed.
#[derive(SystemParam)]
pub struct PointerInputs<'w, 's> {
    primary_q: Query<'w, 's, &'static ActionEvents, With<PointerPrimary>>,
}

impl<'w, 's> PointerInputs<'w, 's> {
    /// Returns `true` on the first frame LMB is pressed (rising edge).
    pub fn pointer_primary_just_pressed(&self) -> bool {
        self.primary_q
            .iter()
            .any(|e| e.contains(ActionEvents::START))
    }
}
