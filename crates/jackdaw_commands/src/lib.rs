pub mod keybinds;

use bevy_ecs::prelude::*;

pub trait EditorCommand: Send + Sync + 'static {
    fn execute(&mut self, world: &mut World);
    fn undo(&mut self, world: &mut World);
    fn description(&self) -> &str;
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
