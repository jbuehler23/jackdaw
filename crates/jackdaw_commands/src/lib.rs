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

    /// Heap bytes this entry holds, for [`CommandHistory`]'s budget.
    ///
    /// Zero unless overridden. The entries worth counting hold bulk per-cell
    /// arrays: a terrain's heights, control words or channel values, kept
    /// before and after.
    ///
    /// A despawn reports zero. It holds a reflected snapshot of the entity's
    /// subtree whose components are `Box<dyn PartialReflect>` with no size to
    /// ask for, and pricing one would mean walking every field of every
    /// component on each push, since the budget re-totals the stack.
    fn heap_bytes(&self) -> usize {
        0
    }
}

/// Bytes of entry payload the history may hold before its oldest entries are
/// dropped.
///
/// Nothing else bounds the stack: it grows for as long as a tab stays open,
/// and terrain entries are large enough that a long sculpting run can exhaust
/// memory.
pub const HISTORY_BUDGET_BYTES: usize = 256 * 1024 * 1024;

#[derive(Resource)]
pub struct CommandHistory {
    pub undo_stack: Vec<Box<dyn EditorCommand>>,
    pub redo_stack: Vec<Box<dyn EditorCommand>>,
    /// Ceiling on what both stacks hold, in bytes. Zero disables the cap.
    pub budget_bytes: usize,
}

impl Default for CommandHistory {
    fn default() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            budget_bytes: HISTORY_BUDGET_BYTES,
        }
    }
}

impl CommandHistory {
    pub fn execute(&mut self, mut command: Box<dyn EditorCommand>, world: &mut World) {
        command.execute(world);
        self.undo_stack.push(command);
        self.redo_stack.clear();
        self.trim_to_budget();
    }

    /// Bytes both stacks currently hold.
    pub fn heap_bytes(&self) -> usize {
        self.undo_stack
            .iter()
            .chain(&self.redo_stack)
            .map(|command| command.heap_bytes())
            .sum()
    }

    /// Drop the oldest undoable entries until both stacks fit the budget.
    ///
    /// The total is summed rather than tracked as a running counter, because
    /// `undo_stack` is public and callers clear and pop it directly. The
    /// newest entry is never dropped, whatever it cost.
    fn trim_to_budget(&mut self) {
        if self.budget_bytes == 0 {
            return;
        }
        let mut bytes = self.heap_bytes();
        while bytes > self.budget_bytes && self.undo_stack.len() > 1 {
            bytes -= self.undo_stack.remove(0).heap_bytes();
            report_budget_reached(self.budget_bytes);
        }
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
        self.trim_to_budget();
    }
}

/// Warn once per process that history is being dropped to stay inside its
/// budget. Each open tab owns a [`CommandHistory`], so a per-history warning
/// would repeat per tab.
fn report_budget_reached(budget: usize) {
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| {
        warn!(
            "undo history reached its {} MiB budget; the oldest entries are being dropped",
            budget / (1024 * 1024),
        );
    });
}

/// Push a command whose ECS work was already done by the caller, AND
/// run its AST-sync hook. Use this from "live drag" code paths where
/// the ECS state was mutated frame-by-frame during the drag and the
/// AST still holds the pre-drag value; the sync hook brings the AST
/// up to date so a later reload doesn't restore the original state.
pub fn push_executed_synced(command: Box<dyn EditorCommand>, commands: &mut Commands) {
    commands.queue(move |world: &mut World| {
        command.sync_after_external_execute(world);
        world
            .resource_mut::<CommandHistory>()
            .push_executed(command);
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

    fn heap_bytes(&self) -> usize {
        self.commands
            .iter()
            .map(|command| command.heap_bytes())
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Weighty(usize);

    impl EditorCommand for Weighty {
        fn execute(&mut self, _world: &mut World) {}
        fn undo(&mut self, _world: &mut World) {}
        fn description(&self) -> &str {
            "weighty"
        }
        fn heap_bytes(&self) -> usize {
            self.0
        }
    }

    #[test]
    fn a_history_inside_its_budget_keeps_every_entry() {
        let mut history = CommandHistory {
            budget_bytes: 100,
            ..default()
        };
        for _ in 0..5 {
            history.push_executed(Box::new(Weighty(10)));
        }
        assert_eq!(history.undo_stack.len(), 5);
        assert_eq!(history.heap_bytes(), 50);
    }

    /// The oldest entries go, not the newest, so what was just done stays
    /// undoable.
    #[test]
    fn passing_the_budget_drops_the_oldest_entries() {
        let mut history = CommandHistory {
            budget_bytes: 100,
            ..default()
        };
        for _ in 0..8 {
            history.push_executed(Box::new(Weighty(30)));
        }
        assert_eq!(history.undo_stack.len(), 3);
        assert_eq!(history.heap_bytes(), 90);
    }

    /// One entry bigger than the whole budget stays undoable.
    #[test]
    fn the_newest_entry_survives_even_when_it_alone_exceeds_the_budget() {
        let mut history = CommandHistory {
            budget_bytes: 100,
            ..default()
        };
        history.push_executed(Box::new(Weighty(10)));
        history.push_executed(Box::new(Weighty(500)));
        assert_eq!(history.undo_stack.len(), 1);
        assert_eq!(history.heap_bytes(), 500);
    }

    /// An undone entry still holds its arrays, so it counts against the same
    /// total the undo stack does.
    #[test]
    fn undone_entries_still_count_toward_the_total() {
        let mut world = World::new();
        let mut history = CommandHistory {
            budget_bytes: 0,
            ..default()
        };
        for _ in 0..3 {
            history.push_executed(Box::new(Weighty(40)));
        }
        history.undo(&mut world);
        assert_eq!(history.undo_stack.len(), 2);
        assert_eq!(history.redo_stack.len(), 1);
        assert_eq!(history.heap_bytes(), 120);
    }

    #[test]
    fn a_zero_budget_caps_nothing() {
        let mut history = CommandHistory {
            budget_bytes: 0,
            ..default()
        };
        for _ in 0..20 {
            history.push_executed(Box::new(Weighty(1_000)));
        }
        assert_eq!(history.undo_stack.len(), 20);
    }
}
