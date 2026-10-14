//! Viewport tools an extension adds: a toolbar button that starts the
//! tool's modal operator, and the status line shown while it runs.
//!
//! A tool is a modal operator. While it runs the editor's own gizmo,
//! numeric entry and viewport selection stand down, its toolbar button is
//! highlighted, and it reads the cursor through
//! [`ViewportCursor`](crate::viewport::ViewportCursor) and the confirm and
//! cancel keys through [`ModalInputs`] each frame.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use jackdaw_input::ActionEvents;

/// A button on the viewport toolbar that calls an operator.
///
/// Spawned by [`crate::ExtensionContext::register_toolbar_button`] as a child
/// of the extension, so the button leaves the toolbar when the extension
/// unloads. The button is highlighted while its operator runs as a modal.
#[derive(Component, Clone, Debug)]
pub struct ToolbarButton {
    /// The operator the button calls.
    pub operator_id: &'static str,
    /// The glyph the button shows.
    pub icon: lucide_icons::Icon,
}

/// The line the status bar shows while a modal tool runs, such as the keys
/// that finish or cancel it.
///
/// A tool sets it when it starts. The editor clears it once no modal
/// operator is running.
#[derive(Resource, Default, Clone, Debug)]
pub struct ToolStatus(pub Option<String>);

/// Marker on each of the seven modal action entities (confirm, cancel,
/// `axis_x`/`y`/`z`, `step_up`, `step_down`).
#[derive(Component)]
pub struct ModalAction;

// ---- Action types ----

/// Confirm the active modal interaction (default: Enter).
#[derive(Component, Default)]
pub struct ModalConfirm;

/// Cancel the active modal interaction (default: Escape).
#[derive(Component, Default)]
pub struct ModalCancel;

/// Constrain modal to the X axis (default: X key).
#[derive(Component, Default)]
pub struct ModalAxisX;

/// Constrain modal to the Y axis (default: Y key).
#[derive(Component, Default)]
pub struct ModalAxisY;

/// Constrain modal to the Z axis (default: Z key).
#[derive(Component, Default)]
pub struct ModalAxisZ;

/// Step the active modal value upward (default: scroll up).
#[derive(Component, Default)]
pub struct ModalStepUp;

/// Step the active modal value downward (default: scroll down).
#[derive(Component, Default)]
pub struct ModalStepDown;

/// System parameter exposing just-fired signals from the modal actions.
///
/// The mechanism is pull-style polling of the [`ActionEvents`] component on
/// each action entity, updated every `PreUpdate` in `ActionSystems`. Systems that run in `Update` (after
/// `PreUpdate`) see a stable value for the whole frame.
///
/// - `confirm`, `cancel`, `axis_x/y/z`: return `true` on `ActionEvents::START`
///   (key transition from not-pressed to pressed; correct for one-shot actions).
/// - `step_up`, `step_down`: return `true` on `ActionEvents::FIRE` (each
///   `ScrollTick` fires once, so FIRE is the right per-tick signal).
#[derive(SystemParam)]
pub struct ModalInputs<'w, 's> {
    confirm_q: Query<'w, 's, &'static ActionEvents, With<ModalConfirm>>,
    cancel_q: Query<'w, 's, &'static ActionEvents, With<ModalCancel>>,
    axis_x_q: Query<'w, 's, &'static ActionEvents, With<ModalAxisX>>,
    axis_y_q: Query<'w, 's, &'static ActionEvents, With<ModalAxisY>>,
    axis_z_q: Query<'w, 's, &'static ActionEvents, With<ModalAxisZ>>,
    step_up_q: Query<'w, 's, &'static ActionEvents, With<ModalStepUp>>,
    step_down_q: Query<'w, 's, &'static ActionEvents, With<ModalStepDown>>,
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
