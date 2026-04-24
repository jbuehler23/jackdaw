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

use bevy::prelude::*;
use bevy_enhanced_input::prelude::*;
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
            bindings![KeyCode::KeyZ.with_mod_keys(ModKeys::CONTROL)],
        ));
        world.spawn((
            Action::<HistoryRedoOp>::new(),
            ActionOf::<CoreExtensionInputContext>::new(ext),
            bindings![KeyCode::KeyZ.with_mod_keys(ModKeys::CONTROL | ModKeys::SHIFT)],
        ));
    });
}

#[operator(id = "history.undo", label = "Undo", allows_undo = false)]
pub(crate) fn history_undo(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
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
