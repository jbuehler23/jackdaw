//! Filesystem watcher over the files the open tabs point at. A scene
//! changed outside the editor raises a prompt offering to reload that tab.
//!
//! Separate from the prefab watcher (`crate::prefab::watcher`), which shares
//! the same notify machinery but handles a change silently by re-resolving the
//! live scene. An open scene is a document the user is editing, so a change
//! underneath it raises a prompt instead.
//!
//! Distinguishing the editor's own writes from outside ones rests on one rule:
//! the editor records the bytes it read or wrote at the boundary where it read
//! or wrote them, and a file still hashing to that record is unchanged.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use jackdaw_feathers::dialog::{DialogActionEvent, DialogVariant, EditorDialog, OpenDialogEvent};
use jackdaw_feathers::icons::EditorFont;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::scenes::{Scenes, TabContent, TabKind};

pub struct ExternalSceneWatchPlugin;

impl Plugin for ExternalSceneWatchPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExternalWatchState>()
            .init_resource::<ExternalSceneChanges>()
            .add_systems(
                Update,
                (
                    refresh_watch_list,
                    drain_changes,
                    sync_prompts_with_tabs,
                    // Ordered ahead of the presenters, so the frame that opens
                    // a dialog is never also the frame that reads its absence
                    // as a dismissal.
                    resolve_dismissed_prompt,
                    dismiss_refusal_notice,
                    present_front_prompt,
                    present_refusal_notice,
                )
                    .chain(),
            )
            .add_observer(on_dialog_reload);
    }
}

const DEBOUNCE: Duration = Duration::from_millis(150);

/// Label on the button that leaves the editor's copy standing.
pub const KEEP_LABEL: &str = "Keep Mine";

/// What the user chose when told a file changed under them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalReloadChoice {
    /// Take what is on disk, discarding the editor's copy.
    Reload,
    /// Leave the editor's copy standing. The next save overwrites disk.
    Keep,
}

/// One open document whose file changed on disk, waiting on an answer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalSceneChange {
    /// Canonical path of the changed file.
    pub path: PathBuf,
    /// Tab holding it, as of the last frame.
    pub tab_index: usize,
    pub tab_display_name: String,
    /// Whether that tab has edits a reload would throw away.
    pub tab_dirty: bool,
    /// Whether the tab holding it is a prefab tab. Affects only the wording of
    /// the prompt.
    pub tab_is_prefab: bool,
    /// Hash of the bytes this prompt was raised for. Answering records these
    /// rather than whatever is on disk by then, so a write that lands while
    /// the prompt is up is still a change the next drain asks about.
    hash: u64,
}

impl ExternalSceneChange {
    /// File name, or the whole path when there isn't one.
    pub fn file_name(&self) -> String {
        file_name_of(&self.path)
    }

    /// Dialog title, naming the kind of document the tab holds.
    pub fn title(&self) -> &'static str {
        if self.tab_is_prefab {
            "Prefab Changed on Disk"
        } else {
            "Scene Changed on Disk"
        }
    }

    /// Label on the button that takes what is on disk. Spells out the cost when
    /// the tab is dirty.
    pub fn action_label(&self) -> &'static str {
        if self.tab_dirty {
            "Reload (discards your unsaved changes)"
        } else {
            "Reload"
        }
    }

    pub fn description(&self) -> String {
        if self.tab_dirty {
            format!(
                "{} changed on disk, and you have unsaved changes to it here. \
                 Reload from disk, or keep yours and overwrite the file on the \
                 next save?",
                self.file_name()
            )
        } else {
            format!(
                "{} changed on disk. Reload it, or keep the copy the editor \
                 has open?",
                self.file_name()
            )
        }
    }
}

/// A reload the editor refused, waiting to be reported to the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefusedReload {
    pub path: PathBuf,
    pub category: crate::scene_io::RefusalCategory,
    pub reason: String,
}

impl RefusedReload {
    pub fn description(&self) -> String {
        format!(
            "{} was not reloaded: {}. The copy the editor has open is \
             untouched.\n\n{}",
            file_name_of(&self.path),
            self.category.label(),
            self.reason
        )
    }
}

/// The outside edits waiting on an answer, and a refusal waiting to be shown.
#[derive(Resource, Default)]
pub struct ExternalSceneChanges {
    /// Front entry is the one being prompted for.
    pub prompts: Vec<ExternalSceneChange>,
    pub refused: Option<RefusedReload>,
}

/// Set while a reload prompt is on screen, naming the file it was opened for.
/// Answering goes to that path rather than to whatever is at the front of the
/// queue when the click lands. Absent headless, where the prompt is the
/// resource and there is no dialog to dismiss.
#[derive(Resource)]
struct ReloadPromptOpen(PathBuf);

/// Set while the refusal notice is on screen.
#[derive(Resource)]
struct RefusalNoticeOpen;

#[derive(Resource, Default)]
struct ExternalWatchState {
    watcher: Option<RecommendedWatcher>,
    watched: Vec<PathBuf>,
    pending: Arc<Mutex<Vec<PathBuf>>>,
    debounced: Vec<(PathBuf, Instant)>,
    /// Hash of the bytes the editor believes are at each watched path: what
    /// it read when it opened the file, what it last wrote there, or what it
    /// has already asked about.
    known: HashMap<PathBuf, u64>,
}

/// Record `contents` as what the editor believes is at `path`.
///
/// Called at the boundary that read or wrote those bytes rather than afterwards
/// from disk, so an edit landing in between still registers as a change.
pub fn note_known_content(world: &mut World, path: &Path, contents: &[u8]) {
    note_known_hash(world, path, hash_bytes(contents));
}

/// [`note_known_content`] for a caller that already has the hash.
pub fn note_known_hash(world: &mut World, path: &Path, hash: u64) {
    let key = canonical(path);
    if let Some(mut state) = world.get_resource_mut::<ExternalWatchState>() {
        state.known.insert(key, hash);
    }
}

/// Answer the prompt raised for `path`.
///
/// An answer for a path with no open prompt is dropped rather than treated as
/// an error. The queue can move under a dialog that is already
/// up (a tab closes, a file is briefly unlinked), and a click on a dialog
/// naming one file must not act on another.
pub fn answer_external_change(world: &mut World, path: &Path, choice: ExternalReloadChoice) {
    let path = canonical(path);
    let Some(change) = world
        .resource::<ExternalSceneChanges>()
        .prompts
        .iter()
        .find(|prompt| prompt.path == path)
        .cloned()
    else {
        debug!(
            "external reload answered for {}, which is no longer being asked about",
            path.display()
        );
        return;
    };

    {
        let mut changes = world.resource_mut::<ExternalSceneChanges>();
        changes.prompts.retain(|other| other.path != change.path);
        changes.refused = None;
    }

    let mut installed = false;
    if choice == ExternalReloadChoice::Reload {
        match reload_tab_from_disk(world, &change.path) {
            Ok(()) => installed = true,
            Err(refusal) => {
                world.resource_mut::<ExternalSceneChanges>().refused = Some(refusal);
            }
        }
    }

    // A reload records the bytes it read, which is the file as of the load.
    // Every other answer records the bytes the prompt was raised for, the ones
    // the user was shown, so a write that landed while the dialog was up is
    // still a change and the re-check below finds it.
    if !installed {
        note_known_hash(world, &change.path, change.hash);
    }
    queue_recheck(world, &change.path);
}

/// Ask the drain to look at `path` once more, without waiting for the
/// filesystem to say anything.
fn queue_recheck(world: &mut World, path: &Path) {
    if let Some(state) = world.get_resource::<ExternalWatchState>()
        && let Ok(mut lock) = state.pending.lock()
    {
        lock.push(path.to_path_buf());
    }
}

/// Re-run the open path for the tab holding `path`.
///
/// A refusal leaves the open scene standing, since the load resolves and gates
/// before it clears anything, so this reports rather than repairs.
fn reload_tab_from_disk(world: &mut World, path: &Path) -> Result<(), RefusedReload> {
    let Some(index) = tab_index_for_path(world, path) else {
        return Err(RefusedReload {
            path: path.to_path_buf(),
            category: crate::scene_io::RefusalCategory::Unreadable,
            reason: format!("{} is no longer open", path.display()),
        });
    };

    // The load installs into the live world, which belongs to the active tab,
    // so the swap has to come first. A refusal therefore has to swap back,
    // rather than leaving the user on a tab they never asked to visit.
    let was_active = world.resource::<Scenes>().active;
    if was_active != index {
        crate::scenes::swap::swap_active_tab(world, index);
    }

    match crate::scene_io::load_scene_from_file_with_outcome(world, path) {
        crate::scene_io::LoadOutcome::Refused(refusal) => {
            if was_active != index && was_active < world.resource::<Scenes>().tabs.len() {
                crate::scenes::swap::swap_active_tab(world, was_active);
            }
            Err(RefusedReload {
                path: path.to_path_buf(),
                category: refusal.category,
                reason: refusal.message,
            })
        }
        crate::scene_io::LoadOutcome::Loaded => {
            adopt_reloaded_document(world, index);
            Ok(())
        }
    }
}

/// Point the tab's own bookkeeping at the document read from disk.
fn adopt_reloaded_document(world: &mut World, index: usize) {
    let doc = world
        .get_resource::<jackdaw_bsn::SceneBsnAst>()
        .map(jackdaw_bsn::SceneBsnAst::deep_clone);
    // The document that arrived decides the tab's kind. An outside edit can
    // turn a scene file into a prefab, and a prefab document left in a Scene
    // tab would be captured and saved back as a plain scene, losing the marker.
    let becomes_prefab = doc
        .as_ref()
        .is_some_and(crate::scenes::operators::document_is_prefab);
    let path = world.resource::<Scenes>().tabs.get(index).and_then(|tab| {
        becomes_prefab
            .then(|| tab.path.clone())
            .flatten()
            .map(|path| crate::prefab::canonical_prefab_path(&path))
    });
    if let (Some(path), Some(doc)) = (path.as_ref(), doc.as_ref())
        && world.contains_resource::<crate::prefab::PrefabAstCache>()
    {
        world
            .resource_mut::<crate::prefab::PrefabAstCache>()
            .insert(path.as_path(), doc.deep_clone());
    }

    let mut scenes = world.resource_mut::<Scenes>();
    let Some(tab) = scenes.tabs.get_mut(index) else {
        return;
    };
    match (path, doc) {
        (Some(path), _) => {
            tab.kind = TabKind::Prefab;
            tab.content = TabContent::Prefab(path);
        }
        (None, Some(doc)) => {
            tab.kind = TabKind::Scene;
            tab.content = TabContent::Scene(Some(Box::new(doc)));
        }
        (None, None) => {}
    }
    tab.dirty = false;
    // The load cleared the undo stacks, so the tab's baseline is zero.
    tab.history_depth_at_last_check = 0;
    // The entities the stored selection named were despawned with the document.
    tab.view_state.selection.clear();
    tab.view_state.brush_sub_selection = crate::brush::BrushSelection::default();
}

fn tab_index_for_path(world: &World, path: &Path) -> Option<usize> {
    world.resource::<Scenes>().tabs.iter().position(|tab| {
        tab.path
            .as_ref()
            .is_some_and(|tab_path| canonical(tab_path) == path)
    })
}

/// One open tab this watcher covers.
struct OpenSceneTab {
    path: PathBuf,
    index: usize,
    display_name: String,
    dirty: bool,
    is_prefab: bool,
}

/// Every open tab this watcher covers.
///
/// Prefab tabs are included: an open prefab file is an edited document like any
/// scene, and the prefab watcher's silent re-resolve would either overwrite the
/// editor's copy or hide the outside edit. To keep the change from being
/// handled twice, `crate::prefab::watcher` skips any path an open prefab tab
/// holds, leaving it to this prompt.
fn open_scene_tabs(scenes: &Scenes) -> Vec<OpenSceneTab> {
    scenes
        .tabs
        .iter()
        .enumerate()
        .filter(|(_, tab)| matches!(tab.kind, TabKind::Scene | TabKind::Prefab))
        .filter_map(|(index, tab)| {
            let path = canonical(tab.path.as_ref()?);
            let is_prefab = matches!(tab.kind, TabKind::Prefab);
            path.is_file().then(|| OpenSceneTab {
                path,
                index,
                display_name: tab.display_name.clone(),
                dirty: tab.dirty,
                is_prefab,
            })
        })
        .collect()
}

fn refresh_watch_list(mut state: ResMut<ExternalWatchState>, scenes: Res<Scenes>) {
    // Reading the tab list costs a canonicalize and a stat per tab, and the
    // list can only differ when the tabs changed.
    if !scenes.is_changed() {
        return;
    }
    let current: Vec<PathBuf> = open_scene_tabs(&scenes)
        .into_iter()
        .map(|tab| tab.path)
        .collect();
    if current == state.watched {
        return;
    }
    let pending = state.pending.clone();
    let mut watcher: RecommendedWatcher =
        match notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res
                && matches!(
                    event.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                )
                && let Ok(mut lock) = pending.lock()
            {
                lock.extend(event.paths);
            }
        }) {
            Ok(watcher) => watcher,
            Err(err) => {
                warn!("open-scene watcher init failed: {err}");
                return;
            }
        };

    let mut newly_watched: Vec<PathBuf> = Vec::new();
    for path in &current {
        if let Err(err) = watcher.watch(path, RecursiveMode::NonRecursive) {
            warn!("watch failed for {}: {}", path.display(), err);
        }
        if !state.watched.contains(path) {
            newly_watched.push(path.clone());
        }
    }
    for path in newly_watched {
        // notify reports nothing that happened before `watch()` returned, so a
        // file edited between the open reading it and the watch starting would
        // never be asked about. This synthetic check closes that gap against
        // the baseline the open recorded.
        if let Ok(mut lock) = state.pending.lock() {
            lock.push(path.clone());
        }
        // Only a path with no recorded hash falls back to disk. A recorded one
        // came from the editor's own read or write, which takes precedence.
        if !state.known.contains_key(&path)
            && let Ok(bytes) = std::fs::read(&path)
        {
            state.known.insert(path, hash_bytes(&bytes));
        }
    }
    state.known.retain(|path, _| current.contains(path));
    state.watcher = Some(watcher);
    state.watched = current;
}

fn drain_changes(world: &mut World) {
    let (fired, mut debounced) = {
        let state = world.resource::<ExternalWatchState>();
        let fired = match state.pending.lock() {
            Ok(mut lock) => lock.drain(..).collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        (fired, state.debounced.clone())
    };
    if fired.is_empty() && debounced.is_empty() {
        return;
    }

    let now = Instant::now();
    for path in fired {
        let path = canonical(&path);
        if !debounced.iter().any(|(seen, _)| *seen == path) {
            debounced.push((path, now));
        }
    }
    let mut settled: Vec<PathBuf> = Vec::new();
    debounced.retain(|(path, at)| {
        if now.duration_since(*at) >= DEBOUNCE {
            settled.push(path.clone());
            false
        } else {
            true
        }
    });
    world.resource_mut::<ExternalWatchState>().debounced = debounced;

    for path in settled {
        if !world
            .resource::<ExternalWatchState>()
            .watched
            .contains(&path)
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            // An unreadable file yields no hash to compare; the next event on
            // it checks again.
            continue;
        };
        let hash = hash_bytes(&bytes);
        if world.resource::<ExternalWatchState>().known.get(&path) == Some(&hash) {
            continue;
        }

        let already_asking = world
            .resource::<ExternalSceneChanges>()
            .prompts
            .iter()
            .any(|prompt| prompt.path == path);
        if already_asking {
            continue;
        }
        let Some(tab) = open_scene_tabs(world.resource::<Scenes>())
            .into_iter()
            .find(|tab| tab.path == path)
        else {
            continue;
        };
        info!("{} changed on disk; offering a reload", path.display());
        world
            .resource_mut::<ExternalSceneChanges>()
            .prompts
            .push(ExternalSceneChange {
                path,
                tab_index: tab.index,
                tab_display_name: tab.display_name,
                tab_dirty: tab.dirty,
                tab_is_prefab: tab.is_prefab,
                hash,
            });
    }
}

/// Drop prompts about files no tab still holds, and refresh the rest against
/// the current tab state.
fn sync_prompts_with_tabs(scenes: Res<Scenes>, mut changes: ResMut<ExternalSceneChanges>) {
    if changes.prompts.is_empty() {
        return;
    }
    let open = open_scene_tabs(&scenes);
    let mut refreshed: Vec<ExternalSceneChange> = Vec::with_capacity(changes.prompts.len());
    for prompt in &changes.prompts {
        let Some(tab) = open.iter().find(|tab| tab.path == prompt.path) else {
            continue;
        };
        refreshed.push(ExternalSceneChange {
            tab_index: tab.index,
            tab_display_name: tab.display_name.clone(),
            tab_dirty: tab.dirty,
            tab_is_prefab: tab.is_prefab,
            ..prompt.clone()
        });
    }
    if refreshed != changes.prompts {
        changes.prompts = refreshed;
    }
}

fn present_front_prompt(world: &mut World) {
    if world.contains_resource::<ReloadPromptOpen>() {
        return;
    }
    let Some(change) = world
        .resource::<ExternalSceneChanges>()
        .prompts
        .first()
        .cloned()
    else {
        return;
    };
    if !screen_is_free(world) {
        return;
    }

    world.insert_resource(ReloadPromptOpen(change.path.clone()));
    let mut dialog = OpenDialogEvent::new(change.title(), change.action_label())
        .with_description(change.description())
        .with_close_button(false)
        .with_close_on_click_outside(false);
    dialog.cancel = Some(KEEP_LABEL.to_string());
    if change.tab_dirty {
        dialog = dialog.with_variant(DialogVariant::Destructive);
    }
    world.commands().trigger(dialog);
    world.flush();
}

/// Show a refused reload in its own notice, since the prompt that triggered it
/// has already closed.
fn present_refusal_notice(world: &mut World) {
    if world.contains_resource::<RefusalNoticeOpen>() {
        return;
    }
    let Some(refused) = world.resource::<ExternalSceneChanges>().refused.clone() else {
        return;
    };
    if !screen_is_free(world) {
        return;
    }

    world.insert_resource(RefusalNoticeOpen);
    world.commands().trigger(
        OpenDialogEvent::new("Scene Not Reloaded", "OK")
            .with_description(refused.description())
            .with_close_button(false)
            .without_cancel(),
    );
    world.flush();
}

/// Whether a dialog surface exists and no dialog is currently up.
///
/// A missing dialog font means headless, where the prompt and the refusal live
/// only in the resource and callers read them directly. A dialog already up
/// blocks presentation; the queued entry waits.
fn screen_is_free(world: &mut World) -> bool {
    if world.get_resource::<EditorFont>().is_none() {
        return false;
    }
    let mut dialogs = world.query_filtered::<Entity, With<EditorDialog>>();
    dialogs.iter(world).next().is_none()
}

fn on_dialog_reload(
    _event: On<DialogActionEvent>,
    open: Option<Res<ReloadPromptOpen>>,
    mut commands: Commands,
) {
    let Some(open) = open else {
        return;
    };
    let path = open.0.clone();
    commands.remove_resource::<ReloadPromptOpen>();
    commands.queue(move |world: &mut World| {
        answer_external_change(world, &path, ExternalReloadChoice::Reload);
    });
}

/// A prompt whose dialog closed without its action firing counts as Keep,
/// covering the cancel button and Esc.
fn resolve_dismissed_prompt(
    open: Option<Res<ReloadPromptOpen>>,
    dialogs: Query<(), With<EditorDialog>>,
    mut commands: Commands,
) {
    let Some(open) = open else {
        return;
    };
    if !dialogs.is_empty() {
        return;
    }
    let path = open.0.clone();
    commands.remove_resource::<ReloadPromptOpen>();
    commands.queue(move |world: &mut World| {
        answer_external_change(world, &path, ExternalReloadChoice::Keep);
    });
}

/// Clear the refusal once its notice closes; either button dismisses it.
fn dismiss_refusal_notice(
    open: Option<Res<RefusalNoticeOpen>>,
    dialogs: Query<(), With<EditorDialog>>,
    mut changes: ResMut<ExternalSceneChanges>,
    mut commands: Commands,
) {
    if open.is_none() || !dialogs.is_empty() {
        return;
    }
    commands.remove_resource::<RefusalNoticeOpen>();
    changes.refused = None;
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}
