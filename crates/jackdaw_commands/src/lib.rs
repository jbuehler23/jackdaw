pub mod keybinds;
mod shortcut_platform;

pub use shortcut_platform::ShortcutPlatform;

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

    /// Why the entry can never apply again, if what it edits is gone. Undo
    /// and redo drop such an entry and go on to the next one.
    fn gone(&self, _world: &World) -> Option<String> {
        None
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
    /// Which state of the document the history last left, drawn from one
    /// process-wide counter so no two histories ever share a value.
    revision: u64,
    /// The states of the document along this history, oldest first, each named
    /// by a value no other state has. Undo and redo move along it, and an edit
    /// after an undo cuts off what lay ahead.
    states: Vec<u64>,
    /// Where in `states` the document is.
    at: usize,
    /// The state the document's file holds, if it is still on this history.
    saved: Option<u64>,
    /// How many entries the budget has dropped from the bottom of the undo
    /// stack, so an entry's depth stays the same as entries below it go.
    trimmed: usize,
    /// The place [`return_to_mark`](Self::return_to_mark) comes back to, while
    /// one is held.
    held: Option<HeldMark>,
}

/// The source of every [`CommandHistory::revision`].
static NEXT_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn next_revision() -> u64 {
    NEXT_REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// A place on a history and the way back to it.
///
/// The way back is the line of undo entries down to `depth`, then `path`
/// redone in order. An edit made after undoing below the mark would discard
/// the redo entries leading back up to it; while a mark is held they move
/// into `path` instead.
struct HeldMark {
    /// The state the history was at.
    state: u64,
    /// Every state made after the mark has a value at least this.
    since: u64,
    /// The states up to and including the mark's.
    states: Vec<u64>,
    /// How many undo entries, counting trimmed ones, lie below where the way
    /// back leaves the line the history is on.
    depth: usize,
    /// The entries to redo from `depth` to reach the mark, oldest first.
    path: Vec<Box<dyn EditorCommand>>,
}

/// What [`CommandHistory::return_to_mark`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Returned {
    /// The first reason an entry whose target is gone was dropped on the way.
    pub dropped: Option<String>,
    /// Whether the history reached the mark. It falls short when the budget
    /// has dropped entries it needed.
    pub reached: bool,
}

/// Where the undo stack stood when a span was opened. Hand it back to
/// [`CommandHistory::end_span`] to collapse everything pushed since.
#[derive(Debug, Clone, Copy)]
pub struct HistorySpan(u64);

impl Default for CommandHistory {
    fn default() -> Self {
        let state = next_revision();
        Self {
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            budget_bytes: HISTORY_BUDGET_BYTES,
            pushes: 0,
            revision: next_revision(),
            states: vec![state],
            at: 0,
            saved: Some(state),
            trimmed: 0,
            held: None,
        }
    }
}

impl CommandHistory {
    pub fn execute(&mut self, mut command: Box<dyn EditorCommand>, world: &mut World) {
        command.execute(world);
        self.keep_way_back();
        self.undo_stack.push(command);
        self.pushes += 1;
        self.revision = next_revision();
        self.enter_new_state();
        self.redo_stack.clear();
        self.trim_to_budget();
    }

    fn enter_new_state(&mut self) {
        self.states.truncate(self.at + 1);
        self.states.push(next_revision());
        self.at = self.states.len() - 1;
    }

    /// Which state of the document the history is at. Unlike
    /// [`revision`](Self::revision), an undo back to a state gives the value that
    /// state had.
    pub fn state(&self) -> u64 {
        self.states[self.at]
    }

    /// Record `state`, from [`state`](Self::state), as what the document's file
    /// holds.
    pub fn mark_saved(&mut self, state: u64) {
        self.saved = Some(state);
    }

    /// Record the state the history is at as what the document's file holds.
    pub fn mark_current_saved(&mut self) {
        self.saved = Some(self.state());
    }

    /// Record that no state on this history is what the document's file holds.
    pub fn forget_saved(&mut self) {
        self.saved = None;
    }

    /// Whether the document is in the state its file holds.
    pub fn is_saved(&self) -> bool {
        self.saved == Some(self.state())
    }

    /// Which state of the document this history last left. It rises on every
    /// do, undo and redo and on [`clear`](Self::clear), and never repeats.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// How many undo entries lie below the document, counting the ones the
    /// budget has dropped.
    fn depth(&self) -> usize {
        self.trimmed + self.undo_stack.len()
    }

    /// Hold the place the history is at now, unless one is already held.
    pub fn hold_mark(&mut self) {
        if self.held.is_some() {
            return;
        }
        self.held = Some(HeldMark {
            state: self.state(),
            since: NEXT_REVISION.load(std::sync::atomic::Ordering::Relaxed),
            states: self.states[..=self.at].to_vec(),
            depth: self.depth(),
            path: Vec::new(),
        });
    }

    /// Let go of the held mark.
    pub fn release_mark(&mut self) {
        self.held = None;
    }

    /// Whether a mark is held.
    pub fn holds_mark(&self) -> bool {
        self.held.is_some()
    }

    /// The state the held mark is at, if one is held.
    pub fn mark_state(&self) -> Option<u64> {
        self.held.as_ref().map(|held| held.state)
    }

    /// A mark at the oldest state on the history, with nothing below it.
    fn origin(&self) -> HeldMark {
        let state = self.states[0];
        HeldMark {
            state,
            since: state + 1,
            states: vec![state],
            depth: 0,
            path: Vec::new(),
        }
    }

    /// Whether the history has moved off the held mark, or off its oldest state
    /// when none is held.
    pub fn left_mark(&self) -> bool {
        let origin;
        let held = match &self.held {
            Some(held) => held,
            None => {
                origin = self.origin();
                &origin
            }
        };
        self.state() != held.state
            || !held.path.is_empty()
            || self.states.iter().any(|state| *state >= held.since)
    }

    /// Before an edit discards the redo stack, move the entries that lead back
    /// up to the held mark onto its way back.
    fn keep_way_back(&mut self) {
        let depth = self.depth();
        let Some(held) = self.held.as_mut() else {
            return;
        };
        if held.depth <= depth {
            return;
        }
        let take = (held.depth - depth).min(self.redo_stack.len());
        let at = self.redo_stack.len() - take;
        let mut leading = self.redo_stack.split_off(at);
        leading.reverse();
        leading.append(&mut held.path);
        held.path = leading;
        held.depth = depth;
    }

    /// Undo and redo back to the held mark, or to the oldest state on the
    /// history when none is held, and let go of it. When anything was done
    /// since the mark, every state made since is forgotten, so no redo reaches
    /// one.
    pub fn return_to_mark(&mut self, world: &mut World) -> Returned {
        let held = self.held.take().unwrap_or_else(|| self.origin());
        let edited = !held.path.is_empty() || self.states.iter().any(|state| *state >= held.since);
        let mut returned = Returned {
            dropped: None,
            reached: true,
        };
        while self.depth() > held.depth {
            let Some(mut command) = self.undo_stack.pop() else {
                break;
            };
            if let Some(why) = command.gone(world) {
                returned.dropped.get_or_insert(why);
                continue;
            }
            command.undo(world);
            if !edited {
                self.redo_stack.push(command);
            }
        }
        if self.depth() > held.depth {
            returned.reached = false;
        }
        let below = held.depth.saturating_sub(self.depth());
        for _ in 0..below {
            let Some(mut command) = self.redo_stack.pop() else {
                returned.reached = false;
                break;
            };
            if let Some(why) = command.gone(world) {
                returned.dropped.get_or_insert(why);
                continue;
            }
            command.execute(world);
            self.undo_stack.push(command);
        }
        if returned.reached {
            for mut command in held.path {
                if let Some(why) = command.gone(world) {
                    returned.dropped.get_or_insert(why);
                    continue;
                }
                command.execute(world);
                self.undo_stack.push(command);
            }
        }
        self.revision = next_revision();
        if edited {
            self.redo_stack.clear();
            if returned.reached {
                self.states = held.states;
                self.at = self.states.len() - 1;
            } else {
                self.enter_new_state();
            }
        } else if let Some(at) = self.states.iter().position(|state| *state == held.state) {
            self.at = at;
        }
        returned
    }

    /// Forget both stacks, as opening another scene does. The document stays as
    /// saved, or as unsaved, as it was.
    pub fn clear(&mut self) {
        let was_saved = self.is_saved();
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.revision = next_revision();
        let state = next_revision();
        self.states = vec![state];
        self.at = 0;
        self.saved = was_saved.then_some(state);
        self.trimmed = 0;
        self.held = None;
    }

    /// Start collecting entries for one undo entry. Spans nest: an inner
    /// span counts against its enclosing one as the single entry it
    /// leaves behind.
    pub fn begin_span(&mut self) -> HistorySpan {
        HistorySpan(self.pushes)
    }

    /// Collapse everything pushed since `span` into one entry labelled
    /// `label`, and count the span as that one entry for any enclosing
    /// span. Returns whether the span left an entry.
    pub fn end_span(&mut self, span: HistorySpan, label: impl Into<String>) -> bool {
        let mut pushed = self.split_span(span);
        match pushed.len() {
            0 => {
                self.pushes = span.0;
                return false;
            }
            // One entry is already one entry; wrapping it would only bury
            // the description the user reads in the undo menu.
            1 => {
                self.undo_stack.push(pushed.remove(0));
                self.at = (self.at + 1).min(self.states.len() - 1);
            }
            _ => self.push_executed(Box::new(CommandGroup {
                label: label.into(),
                commands: pushed,
            })),
        }
        self.pushes = span.0 + 1;
        true
    }

    /// Take everything pushed since `span` off the stack without undoing it,
    /// for a caller that records the span its own way or has already put the
    /// scene back.
    pub fn take_span(&mut self, span: HistorySpan) -> Vec<Box<dyn EditorCommand>> {
        let pushed = self.split_span(span);
        self.pushes = span.0;
        pushed
    }

    fn split_span(&mut self, span: HistorySpan) -> Vec<Box<dyn EditorCommand>> {
        let produced = self.pushes.saturating_sub(span.0) as usize;
        let take = produced.min(self.undo_stack.len());
        let at = self.undo_stack.len() - take;
        self.at = self.at.saturating_sub(take);
        self.undo_stack.split_off(at)
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
            self.trimmed += 1;
            report_budget_reached(self.budget_bytes);
        }
    }

    /// Undoes the newest entry. Entries whose target is gone are dropped on
    /// the way, and the first reason is returned for the caller to report.
    pub fn undo(&mut self, world: &mut World) -> Option<String> {
        let dropped = drop_gone(&mut self.undo_stack, world);
        if let Some(mut command) = self.undo_stack.pop() {
            command.undo(world);
            self.redo_stack.push(command);
            self.revision = next_revision();
            self.at = self.at.saturating_sub(1);
        }
        if dropped.is_some() {
            self.saved = None;
        }
        dropped
    }

    /// Drops the entry the next redo would apply, and the state it leads
    /// to, as if it had never been made.
    pub fn discard_next_redo(&mut self) -> Option<Box<dyn EditorCommand>> {
        let command = self.redo_stack.pop()?;
        if self.at + 1 < self.states.len() {
            self.states.remove(self.at + 1);
        }
        Some(command)
    }

    /// Redoes the next entry, dropping the ones whose target is gone as
    /// [`undo`](Self::undo) does.
    pub fn redo(&mut self, world: &mut World) -> Option<String> {
        let dropped = drop_gone(&mut self.redo_stack, world);
        if let Some(mut command) = self.redo_stack.pop() {
            command.execute(world);
            self.undo_stack.push(command);
            self.revision = next_revision();
            // A rolled-back batch can leave more to redo than states ahead.
            if self.at + 1 == self.states.len() {
                self.enter_new_state();
            } else {
                self.at += 1;
            }
        }
        if dropped.is_some() {
            self.saved = None;
        }
        dropped
    }

    pub fn push_executed(&mut self, command: Box<dyn EditorCommand>) {
        self.keep_way_back();
        self.undo_stack.push(command);
        self.pushes += 1;
        self.revision = next_revision();
        self.enter_new_state();
        self.redo_stack.clear();
        self.trim_to_budget();
    }
}

/// Pops the entries on top of `stack` whose target is gone, returning the
/// first reason.
fn drop_gone(stack: &mut Vec<Box<dyn EditorCommand>>, world: &World) -> Option<String> {
    let mut dropped = None;
    while let Some(why) = stack.last().and_then(|command| command.gone(world)) {
        stack.pop();
        dropped.get_or_insert(why);
    }
    dropped
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

    fn gone(&self, world: &World) -> Option<String> {
        let mut reasons = self.commands.iter().map(|command| command.gone(world));
        let first = reasons.next()??;
        reasons.all(|reason| reason.is_some()).then_some(first)
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

    struct Gone;

    impl EditorCommand for Gone {
        fn execute(&mut self, _world: &mut World) {}
        fn undo(&mut self, _world: &mut World) {}
        fn description(&self) -> &str {
            "gone"
        }
        fn gone(&self, _world: &World) -> Option<String> {
            Some("its target closed".into())
        }
    }

    #[test]
    fn undo_drops_entries_whose_target_is_gone_and_undoes_the_next() {
        let mut world = World::new();
        let mut history = CommandHistory::default();
        history.push_executed(Box::new(Weighty(1)));
        history.push_executed(Box::new(Gone));
        history.push_executed(Box::new(Gone));
        let dropped = history.undo(&mut world);
        assert_eq!(dropped.as_deref(), Some("its target closed"));
        assert!(
            history.undo_stack.is_empty(),
            "the earlier entry was undone"
        );
        assert_eq!(history.redo_stack.len(), 1);
        assert_eq!(history.redo(&mut world), None);
    }

    fn rolled_back_edit(history: &mut CommandHistory) {
        let redo = std::mem::take(&mut history.redo_stack);
        let span = history.begin_span();
        history.push_executed(Box::new(Weighty(1)));
        let _ = history.take_span(span);
        history.redo_stack = redo;
    }

    #[test]
    fn redoing_past_a_rolled_back_batch_gives_each_entry_its_own_state() {
        let mut world = World::new();
        let mut history = CommandHistory::default();
        history.push_executed(Box::new(Weighty(1)));
        history.push_executed(Box::new(Weighty(1)));
        history.undo(&mut world);
        history.undo(&mut world);
        rolled_back_edit(&mut history);

        history.redo(&mut world);
        let first = history.state();
        history.mark_current_saved();
        history.redo(&mut world);
        assert_ne!(history.state(), first);
        assert!(
            !history.is_saved(),
            "the second entry is not what was saved"
        );
        history.undo(&mut world);
        assert!(history.is_saved());
    }

    #[test]
    fn a_rolled_back_batch_leaves_the_saved_state_saved() {
        let mut history = CommandHistory::default();
        rolled_back_edit(&mut history);
        assert!(history.is_saved());
    }

    #[test]
    fn a_rolled_back_batch_that_cut_off_the_saved_state_does_not_read_saved_on_redo() {
        let mut world = World::new();
        let mut history = CommandHistory::default();
        history.push_executed(Box::new(Weighty(1)));
        history.mark_current_saved();
        history.undo(&mut world);
        rolled_back_edit(&mut history);
        history.redo(&mut world);
        assert!(!history.is_saved());
    }

    #[test]
    fn trimming_keeps_the_saved_state_where_it_was() {
        let mut world = World::new();
        let mut history = CommandHistory {
            budget_bytes: 25,
            ..default()
        };
        let opened = history.state();
        history.push_executed(Box::new(Weighty(10)));
        history.push_executed(Box::new(Weighty(10)));
        history.mark_current_saved();
        history.push_executed(Box::new(Weighty(10)));
        history.push_executed(Box::new(Weighty(10)));
        assert_eq!(history.undo_stack.len(), 2, "two entries were trimmed");

        history.undo(&mut world);
        history.undo(&mut world);
        assert!(history.is_saved(), "the oldest state left is the saved one");
        assert_ne!(history.state(), opened);
        history.redo(&mut world);
        assert!(!history.is_saved());
    }

    #[test]
    fn a_saved_state_trimmed_away_cannot_read_saved() {
        let mut world = World::new();
        let mut history = CommandHistory {
            budget_bytes: 15,
            ..default()
        };
        history.push_executed(Box::new(Weighty(10)));
        history.push_executed(Box::new(Weighty(10)));
        history.undo(&mut world);
        assert!(
            !history.is_saved(),
            "the opened state was trimmed from reach"
        );
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

    #[test]
    fn a_saved_state_is_found_again_by_undo_and_redo_and_lost_to_a_new_edit() {
        struct Noop;
        impl EditorCommand for Noop {
            fn execute(&mut self, _world: &mut World) {}
            fn undo(&mut self, _world: &mut World) {}
            fn description(&self) -> &str {
                "noop"
            }
        }
        let mut world = World::new();
        let mut history = CommandHistory::default();
        assert!(history.is_saved(), "a new document is as its file");

        history.execute(Box::new(Noop), &mut world);
        assert!(!history.is_saved());
        history.undo(&mut world);
        assert!(history.is_saved());

        history.execute(Box::new(Noop), &mut world);
        history.mark_current_saved();
        history.undo(&mut world);
        assert!(!history.is_saved());
        history.redo(&mut world);
        assert!(history.is_saved());

        history.undo(&mut world);
        history.execute(Box::new(Noop), &mut world);
        assert!(!history.is_saved(), "the same depth, another state");
        history.undo(&mut world);
        assert!(!history.is_saved(), "the saved state is out of reach");
    }

    #[test]
    fn a_span_of_one_entry_keeps_the_state_and_of_several_makes_one() {
        struct Noop;
        impl EditorCommand for Noop {
            fn execute(&mut self, _world: &mut World) {}
            fn undo(&mut self, _world: &mut World) {}
            fn description(&self) -> &str {
                "noop"
            }
        }
        let mut world = World::new();
        let mut history = CommandHistory::default();
        let span = history.begin_span();
        history.push_executed(Box::new(Noop));
        let one = history.state();
        history.end_span(span, "one");
        assert_eq!(history.state(), one);
        history.mark_current_saved();

        let span = history.begin_span();
        history.push_executed(Box::new(Noop));
        history.push_executed(Box::new(Noop));
        history.end_span(span, "two");
        assert!(!history.is_saved());
        history.undo(&mut world);
        assert!(history.is_saved(), "one undo goes back over the group");
    }

    #[derive(Resource, Default)]
    struct Doc(Vec<&'static str>);

    struct Step(&'static str);

    impl EditorCommand for Step {
        fn execute(&mut self, world: &mut World) {
            world.resource_mut::<Doc>().0.push(self.0);
        }
        fn undo(&mut self, world: &mut World) {
            world.resource_mut::<Doc>().0.pop();
        }
        fn description(&self) -> &str {
            self.0
        }
    }

    fn doc_world() -> World {
        let mut world = World::new();
        world.init_resource::<Doc>();
        world
    }

    fn doc(world: &World) -> Vec<&'static str> {
        world.resource::<Doc>().0.clone()
    }

    #[test]
    fn returning_to_a_mark_undoes_what_followed_and_leaves_nothing_to_redo() {
        let mut world = doc_world();
        let mut history = CommandHistory::default();
        history.execute(Box::new(Step("a")), &mut world);
        history.mark_current_saved();
        history.hold_mark();
        history.execute(Box::new(Step("b")), &mut world);
        history.execute(Box::new(Step("c")), &mut world);
        history.undo(&mut world);
        assert!(history.left_mark());

        let returned = history.return_to_mark(&mut world);

        assert!(returned.reached);
        assert_eq!(doc(&world), ["a"]);
        assert!(history.redo_stack.is_empty());
        assert!(history.is_saved());
        assert!(!history.holds_mark());
        history.redo(&mut world);
        assert_eq!(doc(&world), ["a"]);
    }

    #[test]
    fn returning_to_a_mark_redoes_an_undo_made_since_and_keeps_the_older_redo() {
        let mut world = doc_world();
        let mut history = CommandHistory::default();
        history.execute(Box::new(Step("a")), &mut world);
        history.execute(Box::new(Step("b")), &mut world);
        history.undo(&mut world);
        history.hold_mark();
        history.undo(&mut world);

        history.return_to_mark(&mut world);

        assert_eq!(doc(&world), ["a"]);
        assert_eq!(history.redo_stack.len(), 1, "the redo older than the mark");
    }

    #[test]
    fn undoing_below_the_mark_then_editing_still_returns_to_the_mark() {
        let mut world = doc_world();
        let mut history = CommandHistory::default();
        history.execute(Box::new(Step("a")), &mut world);
        history.execute(Box::new(Step("b")), &mut world);
        history.execute(Box::new(Step("c")), &mut world);
        history.mark_current_saved();
        history.hold_mark();
        history.undo(&mut world);
        history.undo(&mut world);
        history.execute(Box::new(Step("d")), &mut world);
        history.undo(&mut world);
        history.undo(&mut world);
        history.execute(Box::new(Step("e")), &mut world);
        assert_eq!(doc(&world), ["e"]);

        let returned = history.return_to_mark(&mut world);

        assert!(returned.reached);
        assert_eq!(doc(&world), ["a", "b", "c"]);
        assert!(history.is_saved());
        assert!(history.redo_stack.is_empty());
        history.undo(&mut world);
        assert_eq!(
            doc(&world),
            ["a", "b"],
            "the restored entries undo as before"
        );
    }

    #[test]
    fn an_entry_gone_among_the_edits_since_the_mark_leaves_the_older_entries_alone() {
        let mut world = doc_world();
        let mut history = CommandHistory::default();
        history.execute(Box::new(Step("a")), &mut world);
        history.hold_mark();
        history.execute(Box::new(Step("b")), &mut world);
        history.push_executed(Box::new(Gone));
        history.execute(Box::new(Step("c")), &mut world);

        let returned = history.return_to_mark(&mut world);

        assert!(returned.reached);
        assert_eq!(returned.dropped.as_deref(), Some("its target closed"));
        assert_eq!(doc(&world), ["a"]);
        assert_eq!(history.undo_stack.len(), 1);
    }

    #[test]
    fn a_mark_whose_entries_the_budget_dropped_is_out_of_reach() {
        let mut world = doc_world();
        let mut history = CommandHistory {
            budget_bytes: 1,
            ..Default::default()
        };
        history.push_executed(Box::new(Weighty(1)));
        history.hold_mark();
        history.push_executed(Box::new(Weighty(1)));
        history.push_executed(Box::new(Weighty(1)));

        let returned = history.return_to_mark(&mut world);

        assert!(!returned.reached);
        assert!(!history.is_saved());
    }

    #[test]
    fn the_revision_rises_on_do_undo_and_redo_and_never_repeats() {
        let mut world = World::new();
        let mut history = CommandHistory::default();
        let mut seen = vec![history.revision()];
        history.push_executed(Box::new(Weighty(1)));
        seen.push(history.revision());
        history.execute(Box::new(Weighty(1)), &mut world);
        seen.push(history.revision());
        history.undo(&mut world);
        seen.push(history.revision());
        history.redo(&mut world);
        seen.push(history.revision());
        history.clear();
        seen.push(history.revision());
        assert!(
            seen.windows(2).all(|pair| pair[0] < pair[1]),
            "every change raises the revision: {seen:?}"
        );

        let still = history.revision();
        history.undo(&mut world);
        history.redo(&mut world);
        assert_eq!(history.revision(), still, "nothing to undo changes nothing");
        assert!(
            CommandHistory::default().revision() > still,
            "a fresh history never reuses another's revision"
        );
    }
}
