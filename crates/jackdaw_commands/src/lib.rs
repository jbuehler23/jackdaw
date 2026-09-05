pub mod keybinds;

use bevy::prelude::*;

/// Set while the keybind settings dialog is waiting for the user to press
/// the chord it is about to record.
///
/// A chord recorded in the dialog is a chord the editor is also bound to,
/// so without this the press that names a binding also runs whatever it
/// currently means. Every path that turns a key into an action reads this
/// and stands down: the operator dispatch observer, and the handful of
/// panels that read the keyboard directly because their keys are gestures
/// rather than commands. The bindings themselves stay in place, so
/// nothing has to be torn down and rebuilt around a recording.
///
/// It lives here, in the crate every one of those readers already
/// depends on, rather than beside the keymap: a second flag mirrored from
/// this one would be a second thing to get wrong.
#[derive(Resource, Default)]
pub struct KeymapCapture {
    pub recording: bool,
}

impl KeymapCapture {
    /// Whether a key press belongs to the recorder rather than to the
    /// editor. Takes the resource as an option because the panels that
    /// call it also run in worlds that have no keymap at all.
    pub fn is_recording(capture: Option<&Self>) -> bool {
        capture.is_some_and(|capture| capture.recording)
    }
}

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
    /// Most entries hold entity ids and single field values and answer
    /// zero. The ones worth counting are the terrain edits, which keep
    /// before-and-after copies of per-cell arrays.
    fn heap_bytes(&self) -> usize {
        0
    }
}

/// Bytes of entry payload the history may hold before its oldest entries
/// are dropped.
///
/// Nothing else bounds the stack: it grows for as long as a tab stays
/// open, and terrain entries are large enough that a long sculpting run
/// can exhaust memory.
pub const HISTORY_BUDGET_BYTES: usize = 256 * 1024 * 1024;

#[derive(Resource)]
pub struct CommandHistory {
    pub undo_stack: Vec<Box<dyn EditorCommand>>,
    pub redo_stack: Vec<Box<dyn EditorCommand>>,
    /// Ceiling on what both stacks hold, in bytes. Zero disables the cap.
    pub budget_bytes: usize,
    /// How many entries an open span has to account for.
    ///
    /// A caller that groups a run of edits has to know how many entries
    /// the run produced. The stack's length cannot say: `trim_to_budget`
    /// removes from the *front* while the run is going, so a length
    /// recorded before it would name a position that has since slid
    /// under someone else's older edit. Closing a span rewinds this to
    /// what the span actually left on the stack -- one entry, or none --
    /// so an enclosing span counts an inner span once rather than once
    /// per entry the inner span collapsed.
    pushes: u64,
}

/// Where the undo stack stood when a span was opened. Hand it back to
/// [`CommandHistory::end_span`] to collapse everything pushed since.
#[derive(Debug, Clone, Copy)]
pub struct HistorySpan(u64);

impl Default for CommandHistory {
    fn default() -> Self {
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            budget_bytes: HISTORY_BUDGET_BYTES,
            pushes: 0,
        }
    }
}

impl CommandHistory {
    pub fn execute(&mut self, mut command: Box<dyn EditorCommand>, world: &mut World) {
        command.execute(world);
        self.undo_stack.push(command);
        self.pushes += 1;
        self.redo_stack.clear();
        self.trim_to_budget();
    }

    /// Start collecting entries for one undo entry. Spans nest: an inner
    /// span counts against its enclosing one as the single entry it
    /// leaves behind.
    pub fn begin_span(&mut self) -> HistorySpan {
        HistorySpan(self.pushes)
    }

    /// Collapse everything pushed since `span` into one entry labelled
    /// `label`, and count the span as that one entry for any enclosing
    /// span.
    pub fn end_span(&mut self, span: HistorySpan, label: impl Into<String>) {
        let produced = self.pushes.saturating_sub(span.0) as usize;
        let take = produced.min(self.undo_stack.len());
        let at = self.undo_stack.len() - take;
        let mut pushed = self.undo_stack.split_off(at);
        match pushed.len() {
            0 => {
                self.pushes = span.0;
                return;
            }
            // One entry is already one entry; wrapping it would only bury
            // the description the user reads in the undo menu.
            1 => self.undo_stack.push(pushed.remove(0)),
            _ => self.push_executed(Box::new(CommandGroup {
                label: label.into(),
                commands: pushed,
            })),
        }
        self.pushes = span.0 + 1;
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
    /// The total is summed rather than tracked as a running counter,
    /// because `undo_stack` is public and callers clear and pop it
    /// directly. The newest entry is never dropped, whatever it cost.
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
        self.pushes += 1;
        self.redo_stack.clear();
        self.trim_to_budget();
    }
}

/// Warn once per process that history is being dropped to stay inside its
/// budget. Each open tab owns a [`CommandHistory`], so a per-history
/// warning would repeat per tab.
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

    /// An undone entry still holds its arrays, so it counts against the
    /// same total the undo stack does.
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

    /// An inner span costs its enclosing span one entry, not the several
    /// it collapsed, so a batch of calls groups the batch and leaves the
    /// edits made before it alone.
    #[test]
    fn a_span_around_inner_spans_groups_only_their_entries() {
        let mut world = World::new();
        let mut history = CommandHistory {
            budget_bytes: 0,
            ..default()
        };
        for _ in 0..20 {
            history.push_executed(Box::new(Weighty(0)));
        }

        let batch = history.begin_span();
        for _ in 0..5 {
            let call = history.begin_span();
            history.push_executed(Box::new(Weighty(0)));
            history.push_executed(Box::new(Weighty(0)));
            history.end_span(call, "call");
        }
        history.end_span(batch, "batch");

        assert_eq!(history.undo_stack.len(), 21);
        assert_eq!(history.undo_stack[20].description(), "batch");
        history.undo(&mut world);
        assert_eq!(history.undo_stack.len(), 20);
    }

    /// A span that pushed nothing leaves the stack and the enclosing
    /// span's count untouched.
    #[test]
    fn an_empty_span_costs_its_enclosing_span_nothing() {
        let mut history = CommandHistory {
            budget_bytes: 0,
            ..default()
        };
        history.push_executed(Box::new(Weighty(0)));

        let outer = history.begin_span();
        let inner = history.begin_span();
        history.end_span(inner, "inner");
        history.push_executed(Box::new(Weighty(0)));
        history.end_span(outer, "outer");

        assert_eq!(history.undo_stack.len(), 2);
        assert_eq!(history.undo_stack[0].description(), "weighty");
    }
}
