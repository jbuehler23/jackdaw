pub mod keybinds;

use bevy::prelude::*;

pub trait EditorCommand: Send + Sync + 'static {
    fn execute(&mut self, world: &mut World);
    fn undo(&mut self, world: &mut World);
    fn description(&self) -> &str;

    /// Run the post-execute AST sync without redoing the ECS work.
    ///
    /// Called by [`CommandHistory::push_executed`] for commands whose
    /// ECS state was already mutated by the caller (gizmo drag, modal
    /// transform, brush element drag). Those callers can't call
    /// [`Self::execute`] because it would re-apply the ECS mutation;
    /// they need a sync-only path so the AST learns about the new
    /// state. The default impl is a no-op; commands that touch the
    /// AST during execute (`SetTransform`, `SetBrush`, etc.) override
    /// this to sync the final value.
    fn sync_after_external_execute(&self, _world: &mut World) {}
}

#[derive(Resource, Default)]
pub struct CommandHistory {
    pub undo_stack: Vec<Box<dyn EditorCommand>>,
    pub redo_stack: Vec<Box<dyn EditorCommand>>,
}

impl CommandHistory {
    pub fn execute(&mut self, mut command: Box<dyn EditorCommand>, world: &mut World) {
        command.execute(world);
        self.undo_stack.push(command);
        self.redo_stack.clear();
    }

    pub fn undo(&mut self, world: &mut World) {
        if let Some(mut command) = self.undo_stack.pop() {
            command.undo(world);
            self.redo_stack.push(command);
        }
    }

    pub fn redo(&mut self, world: &mut World) {
        if let Some(mut command) = self.redo_stack.pop() {
            command.execute(world);
            self.undo_stack.push(command);
        }
    }

    pub fn push_executed(&mut self, command: Box<dyn EditorCommand>) {
        self.undo_stack.push(command);
        self.redo_stack.clear();
    }
}

/// Push a command whose ECS work was already done by the caller, AND
/// run its AST-sync hook. Use this from "live drag" code paths where
/// the ECS state was mutated frame-by-frame during the drag and the
/// AST still holds the pre-drag value; the sync hook brings the AST
/// up to date so a later reload doesn't restore the original state.
pub fn push_executed_synced(command: Box<dyn EditorCommand>, commands: &mut Commands) {
    commands.queue(move |world: &mut World| {
        command.sync_after_external_execute(world);
        let mut history = world.resource_mut::<CommandHistory>();
        history.undo_stack.push(command);
        history.redo_stack.clear();
    });
}

pub struct CommandGroup {
    pub commands: Vec<Box<dyn EditorCommand>>,
    pub label: String,
}

impl EditorCommand for CommandGroup {
    fn execute(&mut self, world: &mut World) {
        for cmd in &mut self.commands {
            cmd.execute(world);
        }
    }

    fn undo(&mut self, world: &mut World) {
        for cmd in self.commands.iter_mut().rev() {
            cmd.undo(world);
        }
    }

    fn description(&self) -> &str {
        &self.label
    }
}
