//! Undo/Redo operators.
//!
//! These *are* the undo stack, so `allows_undo = false`: they can be
//! invoked uniformly (menu, Ctrl+Z/Ctrl+Shift+Z, F3 palette, extension
//! code) but don't themselves push a new history entry.
//!
//! If a modal operator is in flight when undo/redo fires, cancel it
//! first. The snapshot restore would otherwise rip the scene out from
//! under the modal, leaving its `ActiveModalOperator` marker + per-op
//! state stale.

use bevy_input::ButtonInput;
use bevy_ecs::prelude::*;
use bevy_app::prelude::*;
use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;

use crate::core_extension::CoreExtensionInputContext;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<HistoryUndoOp>()
        .register_operator::<HistoryRedoOp>();

    let ext = ctx.id();
    ctx.entity_mut().world_scope(|world| {
        world.spawn((
            Action::<HistoryUndoOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyZ.with_mod_keys(ModKeys::CONTROL),
                Press::default(),
            )],
        ));
        world.spawn((
            Action::<HistoryRedoOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![(
                KeyCode::KeyZ.with_mod_keys(ModKeys::CONTROL | ModKeys::SHIFT),
                Press::default(),
            )],
        ));
    });
}

#[operator(id = "history.undo", label = "Undo", allows_undo = false)]
pub(crate) fn history_undo(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        // Ctrl+Shift+Z fires both the Ctrl-only and Ctrl+Shift bindings
        // because the modifier matcher is "must include these"; bail when
        // Shift is held so redo can run alone.
        let shift_held = world
            .get_resource::<ButtonInput<KeyCode>>()
            .is_some_and(|kb| kb.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]));
        if shift_held {
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
        world.resource_scope(|world, mut history: Mut<crate::commands::CommandHistory>| {
            history.undo(world);
        });
    });
    OperatorResult::Finished
}

#[operator(id = "history.redo", label = "Redo", allows_undo = false)]
pub(crate) fn history_redo(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        // Symmetric to `history_undo`: re-add the last knife point if any.
        if let Some(mut knife) = world.get_resource_mut::<crate::brush::KnifeMode>()
            && knife.redo_point()
        {
            return;
        }
        cancel_active_modal_if_any(world);
        world.resource_scope(|world, mut history: Mut<crate::commands::CommandHistory>| {
            history.redo(world);
        });
    });
    OperatorResult::Finished
}

fn cancel_active_modal_if_any(world: &mut World) {
    if let Err(err) = world.cancel_active_modal() {
        warn!("Failed to cancel active modal before undo/redo: {err:?}");
    }
}
