//! Gizmo space operator.
//!
//! `gizmo.space.toggle` flips world/local transform space.
//! Default keybind: L.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;

use crate::core_extension::CoreExtensionInputContext;
use crate::gizmos::GizmoSpace;
use crate::keybind_focus::KeybindFocus;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<GizmoSpaceToggleOp>();

    ctx.bind_operator::<CoreExtensionInputContext, GizmoSpaceToggleOp>([PresetInput::key("KeyL")]);
}

/// Space toggle is allowed in any edit mode. Modal drags block it via
/// `is_modal_running`; the toggle is a no-op when no gizmo is visible.
///
/// World and local are the 3D gizmo's two frames, so the chord belongs to
/// the world: over the canvas the letter is one a name is spelled with.
fn can_toggle_space(
    keybind_focus: KeybindFocus,
    active: ActiveModalQuery,
    viewport: crate::viewport_2d::FrontedViewport,
) -> bool {
    !keybind_focus.keyboard_is_spoken_for() && !active.is_modal_running() && viewport.is_three_d()
}

#[operator(
    id = "gizmo.space.toggle",
    label = "Toggle Gizmo Space",
    is_available = can_toggle_space
)]
pub(crate) fn gizmo_space_toggle(
    _: In<OperatorParameters>,
    mut space: ResMut<GizmoSpace>,
) -> OperatorResult {
    *space = match *space {
        GizmoSpace::World => GizmoSpace::Local,
        GizmoSpace::Local => GizmoSpace::World,
    };
    OperatorResult::Finished
}
