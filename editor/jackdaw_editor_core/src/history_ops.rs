//! Undo/Redo operators.
//!
//! These *are* the undo stack, so `allows_undo = false`: they can be
//! invoked uniformly (menu, Ctrl+Z/Ctrl+Shift+Z, F3 palette, extension
//! code) but don't themselves push a new history entry.
//!
//! If a modal operator is in flight when undo/redo fires, cancel it
//! first. The snapshot restore would otherwise rip the scene out from
//! under the modal, leaving its `ActiveModalOperator` marker + per-op
//! state stale. A running 2D canvas gesture goes the same way, through
//! [`crate::ui_stage::cancel_canvas_gestures`].

use bevy::input::ButtonInput;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;

use crate::core_extension::CoreExtensionInputContext;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<HistoryUndoOp>()
        .register_operator::<HistoryRedoOp>();

    ctx.bind_operator::<CoreExtensionInputContext, HistoryUndoOp>([
        PresetInput::key("KeyZ").ctrl_or_super()
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, HistoryRedoOp>([PresetInput::key("KeyZ")
        .ctrl_or_super()
        .shift()]);
}

/// Undo names the shortcut modifier. Shift on top of it is Redo's chord, and
/// Alt on top of it is neither.
const UNDO_CHORD: crate::keybinds::ChordModifiers = crate::keybinds::ChordModifiers {
    primary: true,
    alt: false,
    shift: false,
};

#[operator(
    id = "history.undo",
    leaves_history = true,
    label = "Undo",
    allows_undo = false
)]
pub(crate) fn history_undo(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        // Ctrl+Shift+Z fires both the Ctrl-only and Ctrl+Shift bindings
        // because the modifier matcher is "must include these"; the chord
        // names Ctrl and nothing else, so Shift held is Redo's chord and
        // not this one.
        if world
            .get_resource::<ButtonInput<KeyCode>>()
            .is_some_and(|keyboard| crate::keybinds::unwanted_modifier(keyboard, UNDO_CHORD))
        {
            return;
        }
        // In-modal undo for knife mode: pop the last placed path point
        // instead of cancelling the modal. Path points aren't committed
        // history entries (commit happens on Enter), so the pop is a pure
        // resource mutation; the popped point goes onto `undone_path` for
        // symmetric redo.
        if let Some(mut knife) = world.get_resource_mut::<crate::brush::KnifeMode>()
            && knife.undo_point()
        {
            return;
        }
        cancel_active_modal_if_any(world);
        crate::ui_stage::cancel_canvas_gestures(world);
        let dropped = info_span!("history.undo").in_scope(|| {
            world.resource_scope(|world, mut history: Mut<crate::commands::CommandHistory>| {
                history.undo(world)
            })
        });
        if let Some(why) = dropped {
            jackdaw_api_internal::operator::warn_caller(world, why);
        }
        refresh_views_after_history(world);
    });
    OperatorResult::Finished
}

#[operator(
    id = "history.redo",
    leaves_history = true,
    label = "Redo",
    allows_undo = false
)]
pub(crate) fn history_redo(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        // Symmetric to `history_undo`: re-add the last knife point if any.
        if let Some(mut knife) = world.get_resource_mut::<crate::brush::KnifeMode>()
            && knife.redo_point()
        {
            return;
        }
        cancel_active_modal_if_any(world);
        crate::ui_stage::cancel_canvas_gestures(world);
        let dropped = info_span!("history.redo").in_scope(|| {
            world.resource_scope(|world, mut history: Mut<crate::commands::CommandHistory>| {
                history.redo(world)
            })
        });
        if let Some(why) = dropped {
            jackdaw_api_internal::operator::warn_caller(world, why);
        }
        refresh_views_after_history(world);
    });
    OperatorResult::Finished
}

/// Undo or redo the document in front back to its history's mark, as
/// [`jackdaw_commands::CommandHistory::return_to_mark`] does. Returns whether
/// it got there.
pub(crate) fn return_to_mark(world: &mut World) -> bool {
    cancel_active_modal_if_any(world);
    crate::ui_stage::cancel_canvas_gestures(world);
    let returned =
        world.resource_scope(|world, mut history: Mut<crate::commands::CommandHistory>| {
            history.return_to_mark(world)
        });
    if let Some(why) = returned.dropped {
        warn!("{why}");
    }
    refresh_views_after_history(world);
    returned.reached
}

/// Refresh derived view state after an undo or redo: give the outliner a row for
/// any root the history brought back. Row icons follow the components that decide
/// them, and meshes and edit overlays follow `Changed<Brush>` on the brushes the
/// undo touched.
fn refresh_views_after_history(world: &mut World) {
    let _span = info_span!("history.refresh").entered();
    let _ = crate::hierarchy::rebuild_hierarchy(world);
}

fn cancel_active_modal_if_any(world: &mut World) {
    if let Err(err) = world.cancel_active_modal() {
        warn!("Failed to cancel active modal before undo/redo: {err:?}");
    }
}
