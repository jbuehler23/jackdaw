//! Gizmo space operator.
//!
//! `gizmo.space.toggle` flips world/local transform space.
//! Default keybind: X.

use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;

use crate::active_tool::ActiveTool;
use crate::brush::{BrushSelection, EditMode};
use crate::core_extension::CoreExtensionInputContext;
use crate::gizmos::GizmoSpace;
use crate::keybind_focus::KeybindFocus;
use crate::numeric_transform::{NumericTransformState, numeric_entry_eligible};
use crate::selection::Selection;

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
///
/// The bare X key is shared with numeric transform entry. When a transform
/// tool is active with a valid selection, X starts a numeric entry instead of
/// toggling space, so the toggle yields in that context (and while an entry is
/// already open). X still toggles space with the Select tool or an empty
/// selection.
fn can_toggle_space(
    keybind_focus: KeybindFocus,
    active: ActiveModalQuery,
    numeric: Res<NumericTransformState>,
    mode: Res<ActiveTool>,
    edit_mode: Res<EditMode>,
    selection: Res<Selection>,
    brush_selection: Res<BrushSelection>,
) -> bool {
    if keybind_focus.is_typing() || active.is_modal_running() {
        return false;
    }
    if numeric.axis.is_some() {
        return false;
    }
    !numeric_entry_eligible(&mode, &edit_mode, &selection, &brush_selection)
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
