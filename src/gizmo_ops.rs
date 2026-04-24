//! Gizmo mode + space operators.
//!
//! Mode ops (`gizmo.mode.translate/rotate/scale`) flip the active
//! transform gizmo. Space op (`gizmo.space.toggle`) flips world/local.
//! All gated to Object mode so they don't fire while the user is
//! editing brush sub-elements or mid-modal.
//!
//! Default keybinds: R=rotate, T=scale, Escape=translate, X=space
//! toggle.

use bevy::{input_focus::InputFocus, prelude::*};
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;

use crate::core_extension::CoreExtensionInputContext;
use crate::gizmos::{GizmoMode, GizmoSpace};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<GizmoModeTranslateOp>()
        .register_operator::<GizmoModeRotateOp>()
        .register_operator::<GizmoModeScaleOp>()
        .register_operator::<GizmoSpaceToggleOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<GizmoModeRotateOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::KeyR, Press::default())],
        ));
        world.spawn((
            Action::<GizmoModeScaleOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::KeyT, Press::default())],
        ));
        world.spawn((
            Action::<GizmoModeTranslateOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::Escape, Press::default())],
        ));
        world.spawn((
            Action::<GizmoSpaceToggleOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(KeyCode::KeyX, Press::default())],
        ));
    });
}

/// Gizmo mode changes are ignored while any overlay is typing into a
/// text field or a modal operator is in flight — matches the guards
/// the legacy handler used to apply.
fn can_change_gizmo(
    input_focus: Res<InputFocus>,
    edit_mode: Res<crate::brush::EditMode>,
    active: ActiveModalQuery,
) -> bool {
    input_focus.0.is_none()
        && !active.is_modal_running()
        && *edit_mode == crate::brush::EditMode::Object
}

#[operator(
    id = "gizmo.mode.translate",
    label = "Gizmo Translate",
    allows_undo = false,
    is_available = can_change_gizmo
)]
pub(crate) fn gizmo_mode_translate(
    _: In<OperatorParameters>,
    mut mode: ResMut<GizmoMode>,
) -> OperatorResult {
    *mode = GizmoMode::Translate;
    OperatorResult::Finished
}

#[operator(
    id = "gizmo.mode.rotate",
    label = "Gizmo Rotate",
    allows_undo = false,
    is_available = can_change_gizmo
)]
pub(crate) fn gizmo_mode_rotate(
    _: In<OperatorParameters>,
    mut mode: ResMut<GizmoMode>,
) -> OperatorResult {
    *mode = GizmoMode::Rotate;
    OperatorResult::Finished
}

#[operator(
    id = "gizmo.mode.scale",
    label = "Gizmo Scale",
    allows_undo = false,
    is_available = can_change_gizmo
)]
pub(crate) fn gizmo_mode_scale(
    _: In<OperatorParameters>,
    mut mode: ResMut<GizmoMode>,
) -> OperatorResult {
    *mode = GizmoMode::Scale;
    OperatorResult::Finished
}

#[operator(
    id = "gizmo.space.toggle",
    label = "Toggle Gizmo Space",
    allows_undo = false,
    is_available = can_change_gizmo
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
