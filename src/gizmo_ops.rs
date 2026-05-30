//! Gizmo space operator.
//!
//! `gizmo.space.toggle` flips world/local transform space.
//! Default keybind: X.

use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;

use crate::core_extension::CoreExtensionInputContext;
use crate::gizmos::GizmoSpace;
use crate::keybind_focus::KeybindFocus;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<GizmoSpaceToggleOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<GizmoSpaceToggleOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::KeyX, Press::default())],
        ));
    });
}

/// Space toggle is allowed in any edit mode. Modal drags block it via
/// `is_modal_running`; the toggle is a no-op when no gizmo is visible.
fn can_toggle_space(keybind_focus: KeybindFocus, active: ActiveModalQuery) -> bool {
    !keybind_focus.is_typing() && !active.is_modal_running()
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
