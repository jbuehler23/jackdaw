//! Undoes the edits made during Play once the last game stops, when the
//! project's Play settings ask for it.
//!
//! Play leaving Stopped holds a mark on each open document's history. Edits
//! made during Play author the documents and reach the game as any edit does.
//! When Play returns to Stopped after a game ran, with [`PlayEdits::Revert`]
//! set, every document that moved off its mark is undone back to it, and the
//! states made since are forgotten so no redo brings them back. A document
//! opened during Play goes back to how it opened. A save made during Play
//! stays on disk, so the reverted document then reads modified against its
//! file.

use std::collections::HashMap;

use bevy::prelude::*;
use jackdaw_commands::CommandHistory;

use crate::pie::{PlayPhase, PlayPhaseChanged};
use crate::play_settings::{PlayEdits, PlaySettings};
use crate::scenes::Scenes;

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(follow_play_phase);
}

/// Present from Play leaving Stopped until it returns there. The marks
/// themselves are held by the tabs' histories.
#[derive(Resource, Default)]
pub struct PlayEditMarks {
    /// Whether each tab marked read modified then, by tab id.
    dirty: HashMap<u64, bool>,
    /// Whether a game played, so a launch that never ran reverts nothing.
    played: bool,
}

fn follow_play_phase(changed: On<PlayPhaseChanged>, mut commands: Commands) {
    let PlayPhaseChanged { from, to } = *changed.event();
    commands.queue(move |world: &mut World| {
        if from == PlayPhase::Stopped {
            mark_play_start(world);
        }
        if matches!(to, PlayPhase::Playing | PlayPhase::Paused)
            && let Some(mut marks) = world.get_resource_mut::<PlayEditMarks>()
        {
            marks.played = true;
        }
        if to == PlayPhase::Stopped {
            finish_play(world);
        }
    });
}

/// Hold a mark on every open tab's history.
fn mark_play_start(world: &mut World) {
    if world.contains_resource::<PlayEditMarks>() {
        return;
    }
    let mut dirty = HashMap::new();
    if let Some(mut scenes) = world.get_resource_mut::<Scenes>() {
        let active = scenes.active;
        for (index, tab) in scenes.tabs.iter_mut().enumerate() {
            dirty.insert(tab.id, tab.dirty);
            if index != active {
                tab.history.hold_mark();
            }
        }
    }
    if let Some(mut history) = world.get_resource_mut::<CommandHistory>() {
        history.hold_mark();
    }
    world.insert_resource(PlayEditMarks {
        dirty,
        played: false,
    });
}

/// Undo every tab back to its mark when a game played and the project asks
/// for it, then let go of the marks.
fn finish_play(world: &mut World) {
    let Some(marks) = world.remove_resource::<PlayEditMarks>() else {
        return;
    };
    let revert = world
        .get_resource::<PlaySettings>()
        .is_some_and(|settings| settings.edits == PlayEdits::Revert);
    if marks.played && revert {
        revert_to_marks(world, &marks.dirty);
    }
    release_marks(world);
}

fn release_marks(world: &mut World) {
    if let Some(mut history) = world.get_resource_mut::<CommandHistory>() {
        history.release_mark();
    }
    if let Some(mut scenes) = world.get_resource_mut::<Scenes>() {
        for tab in &mut scenes.tabs {
            tab.history.release_mark();
        }
    }
}

fn revert_to_marks(world: &mut World, dirty_at_mark: &HashMap<u64, bool>) {
    let _span = info_span!("play.revert").entered();
    let Some(front) = world
        .get_resource::<Scenes>()
        .map(|scenes| scenes.tabs.get(scenes.active).map(|tab| tab.id))
    else {
        return;
    };
    let ids: Vec<u64> = world
        .resource::<Scenes>()
        .tabs
        .iter()
        .map(|tab| tab.id)
        .collect();
    let mut short = false;
    for id in ids {
        let Some(index) = tab_index(world, id) else {
            continue;
        };
        if !left_mark(world, index) || !crate::scenes::swap::swap_active_tab(world, index) {
            continue;
        }
        let was_dirty = dirty_at_mark.get(&id).copied().unwrap_or(false)
            && world.resource::<CommandHistory>().holds_mark();
        let reached = crate::history_ops::return_to_mark(world);
        short |= !reached;
        let (state, saved) = {
            let history = world.resource::<CommandHistory>();
            (history.state(), history.is_saved())
        };
        if let Some(tab) = world.resource_mut::<Scenes>().tabs.get_mut(index) {
            tab.dirty = !saved || (was_dirty && reached);
            tab.history_state_at_last_check = Some(state);
        }
    }
    if let Some(index) = front.and_then(|id| tab_index(world, id)) {
        crate::scenes::swap::swap_active_tab(world, index);
    }
    if short {
        crate::status_bar::notify_warn(
            world,
            "Edits made during Play were only partly reverted: the undo history no longer reaches back to when Play started",
        );
    }
}

fn tab_index(world: &World, id: u64) -> Option<usize> {
    world
        .resource::<Scenes>()
        .tabs
        .iter()
        .position(|tab| tab.id == id)
}

/// Whether tab `index`'s history has left its mark, or left how it opened when
/// it holds none.
fn left_mark(world: &World, index: usize) -> bool {
    let scenes = world.resource::<Scenes>();
    if scenes.active == index {
        world.resource::<CommandHistory>().left_mark()
    } else {
        scenes
            .tabs
            .get(index)
            .is_some_and(|tab| tab.history.left_mark())
    }
}
