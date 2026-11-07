//! Saving a scene with the main thread free.
//!
//! The main thread takes the snapshot and encodes the sidecars
//! (`prepare_scene_save`). A thread of its own, since a game build
//! can hold the task pool for minutes and this is waited on, emits the document,
//! reads the text back, writes the sidecars, and writes and renames the file
//! (`SceneWrite`), and the main thread then says what became of it
//! (`finish_scene_save`). So a document that does not read back
//! touches nothing on disk.
//!
//! Only the tab in front has a write running. Anything that takes its document or
//! its file out from under the save first waits for the save (`land_saves`), so
//! the last part runs with the saved tab in front, as the hooks and `SceneSaved`
//! observers expect. Whatever else writes to a file a save may be
//! writing waits too.
//!
//! A second save during a write takes its snapshot at once and waits behind it, or
//! does nothing if the document has not moved on. A third replaces the second, whose
//! sidecars are never written, so one write runs and one waits, and the last state
//! asked for is the last on disk.
//!
//! Saving All writes the tabs behind in one blocking pass, each brought forward
//! with no frame in between, and only the tab in front in the background. A save
//! that moves a document to a new file also runs with no frame in between, see
//! [`super::hooks::DocumentIoHook::holds_the_frame`]. Leaving and closing a tab
//! wait on `AfterSaves` and say so in the footer, and run once no modal operator
//! or held history needs the tabs to stay as they are. A caller that needs the file on
//! return uses the blocking [`super::save::save_scene`]; the `scene.save` operator
//! does when not invoked from the user interface.
//!
//! A write here is forced to the disk. Any scene write, blocking or not, finds out if
//! the file changed under it since the editor last knew its bytes: it then leaves
//! that file in place, keeps a copy beside it for a later Keep Mine to spare, and
//! fails, so the outside edit is offered as any other is.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use bevy::app::AppExit;
use bevy::ecs::message::Messages;
use bevy::platform::cell::SyncCell;
use bevy::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalOperator;
use jackdaw_bsn::SceneBsnAst;

use super::hooks::{PendingSave, SidecarWrite};
use crate::scenes::external_watch::{canonical, hash_bytes, queue_recheck};
use crate::scenes::operators::TabSaveFailure;
use crate::scenes::{Scenes, TabKind};
use crate::worker_task::{WorkerOutcome, WorkerTask};

/// Room for the recursion of a deeply nested document, as on a main thread.
const WRITE_THREAD_STACK: usize = 8 * 1024 * 1024;

const PHASE: &str = "scene_save";

/// How old a temporary file has to be for no live write to be behind it.
const STALE_TEMPORARY_AGE: Duration = Duration::from_secs(60 * 60);

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<SceneSaves>()
        .add_systems(Update, poll_scene_saves)
        .add_systems(Last, land_saves_on_exit)
        .add_systems(
            OnExit(crate::AppState::Editor),
            land_saves.before(crate::cleanup_editor),
        )
        .add_systems(
            OnEnter(crate::AppState::Editor),
            sweep_temporaries_of_project,
        );
}

/// The part of a save that needs no world.
pub(crate) struct SceneWrite {
    pub(crate) ast: Option<SceneBsnAst>,
    pub(crate) path: String,
    /// The legacy file to keep as a backup, moved aside just before the write.
    pub(crate) legacy_backup: Option<String>,
    sidecars: Vec<SidecarWrite>,
    /// The hash of the bytes the file should hold until this write replaces them.
    expected: Option<u64>,
    /// Whether to force the files to the disk, which is for off the main thread.
    durable: bool,
    /// The hash of the bytes in the file, once they are there.
    landed: Arc<OnceLock<u64>>,
    /// Set before the first sidecar is written. A save that fails after this leaves
    /// sidecars the document's file does not go with.
    pub(crate) sidecars_started: Arc<AtomicBool>,
    before_writing: Option<SaveGate>,
    before_renaming: Option<SaveGate>,
    after_writing: Option<SaveGate>,
}

/// What a [`SceneWrite`] did.
pub(crate) struct WrittenScene {
    /// The hash of the bytes now in the file, or why the old file still stands.
    pub(crate) outcome: Result<u64, String>,
    /// How much of the text the document's brushes take up.
    pub(crate) brush_bytes: usize,
}

impl SceneWrite {
    pub(crate) fn new(
        ast: Option<SceneBsnAst>,
        path: String,
        legacy_backup: Option<String>,
        sidecars: Vec<SidecarWrite>,
    ) -> Self {
        Self {
            ast,
            path,
            legacy_backup,
            sidecars,
            expected: None,
            durable: false,
            landed: Arc::default(),
            sidecars_started: Arc::default(),
            before_writing: None,
            before_renaming: None,
            after_writing: None,
        }
    }

    /// Have this write fail rather than replace bytes the editor does not know of.
    pub(crate) fn expect_known_file(&mut self, world: &World) {
        self.expected = crate::scenes::external_watch::known_hash(world, Path::new(&self.path));
    }

    pub(crate) fn write(mut self) -> WrittenScene {
        let _span = info_span!("save.write_thread").entered();
        if let Some(gate) = self.before_writing.take() {
            gate.wait();
        }
        let ast = self.ast.take();
        let (body, brush_bytes) = ast
            .as_ref()
            .map_or((String::new(), 0), jackdaw_bsn::emit_scene_sizing_brushes);
        let outcome = self.write_text(&body);
        if let Some(ast) = ast {
            jackdaw_bsn::drop_document(ast);
        }
        if let Some(gate) = self.after_writing.take() {
            gate.wait();
        }
        WrittenScene {
            outcome,
            brush_bytes,
        }
    }

    /// Read `body` back, and only then touch the disk.
    fn write_text(&self, body: &str) -> Result<u64, String> {
        let path = &self.path;
        let parsed =
            jackdaw_bsn::parse_emitted(body).map_err(|err| format!("cannot save {path}: {err}"))?;
        jackdaw_bsn::drop_document(parsed);
        let stamped = super::stamp::with_stamp(body);
        let bytes = jackdaw_bsn::document_bytes(Path::new(path), &stamped)
            .map_err(|err| format!("failed to write scene file {path}: {err}"))?;
        self.refuse_outside_edit()?;
        if !self.sidecars.is_empty() {
            self.sidecars_started.store(true, Ordering::Relaxed);
        }
        info_span!("save.sidecar_files")
            .in_scope(|| super::hooks::write_sidecars(&self.sidecars, self.durable))
            .map_err(|err| format!("cannot save {path}: {err}"))?;
        // A redirected save moves the legacy source aside first so a stale `.jsn`
        // cannot shadow the fresh `.bsn` on the next open.
        if let Some(old_path) = &self.legacy_backup {
            let backup = format!("{old_path}.bak");
            std::fs::rename(old_path, &backup).map_err(|err| {
                format!("could not back up legacy scene {old_path} to {backup}: {err}")
            })?;
        }
        let hash = info_span!("save.hash").in_scope(|| hash_bytes(&bytes));
        info_span!("save.write")
            .in_scope(|| {
                if self.durable {
                    jackdaw_api_internal::document::write_durable(Path::new(path), &bytes, || {
                        if let Some(gate) = &self.before_renaming {
                            gate.wait();
                        }
                        self.refuse_outside_edit().map_err(std::io::Error::other)
                    })
                } else {
                    super::save::write_atomic(Path::new(path), &bytes)
                }
            })
            .map_err(|err| format!("failed to write scene file {path}: {err}"))?;
        self.landed.set(hash).ok();
        Ok(hash)
    }

    /// Fail if the file no longer holds the bytes this write replaces, leaving them
    /// where they are with a copy beside them.
    fn refuse_outside_edit(&self) -> Result<(), String> {
        let Some(expected) = self.expected else {
            return Ok(());
        };
        let path = &self.path;
        let held = match std::fs::read(path) {
            Ok(held) => held,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(err) => {
                return Err(format!(
                    "cannot save {path}: could not read what it holds now: {err}"
                ));
            }
        };
        if hash_bytes(&held) == expected {
            return Ok(());
        }
        let copy = outside_copy_of(path);
        super::save::write_atomic(Path::new(&copy), &held)
            .map_err(|err| format!("{path} changed outside the editor; cannot keep it: {err}"))?;
        Err(format!(
            "{path} changed outside the editor during the save, so it was left as it is, \
             with a copy at {copy}"
        ))
    }
}

fn outside_copy_of(path: &str) -> String {
    format!("{path}.outside.bak")
}

/// Delete the copy a refused write kept of `path`, if the file now says the same.
pub(crate) fn drop_outside_copy_matching_file(path: &Path) {
    let copy = outside_copy_of(&path.to_string_lossy());
    let same = match (std::fs::read(&copy), std::fs::read(path)) {
        (Ok(kept), Ok(held)) => kept == held,
        _ => false,
    };
    if same && let Err(err) = std::fs::remove_file(&copy) {
        warn!("could not remove {copy}: {err}");
    }
}

/// What the last part of a save needs from the first.
pub(crate) struct WaitingSave {
    pub(crate) tab: u64,
    pub(crate) tab_name: String,
    pub(crate) tab_kind: TabKind,
    /// The tab's history state when the snapshot was taken.
    pub(crate) state: u64,
    /// The history's revision when the snapshot was taken.
    pub(crate) history_revision: u64,
    /// The live document's revision when the snapshot was taken, which also moves on
    /// edits that leave no history entry.
    pub(crate) document: Option<u32>,
    pub(crate) sidecars_started: Arc<AtomicBool>,
    pub(crate) path: String,
    pub(crate) redirected: bool,
    /// What the hooks returned for this save.
    pub(crate) hooks: SyncCell<PendingSave>,
}

pub(crate) struct PreparedSave {
    pub(crate) write: SceneWrite,
    pub(crate) waiting: WaitingSave,
}

struct RunningSave {
    task: WorkerTask<WrittenScene>,
    waiting: WaitingSave,
    /// The canonical path being written, which the watcher leaves alone.
    file: PathBuf,
    /// Set once the file is in place, so a thread that dies after that is not taken
    /// for a save that failed.
    landed: Arc<OnceLock<u64>>,
}

/// What waits for every save to be over.
pub(crate) enum AfterSaves {
    /// Ask again whether the editor can quit.
    CloseWindow,
    /// Ask again whether the project can be left.
    LeaveProject,
    /// Ask again whether this tab can close.
    CloseTab(u64),
    /// Leave, if all went well, for the quit dialog's Save All.
    LeaveIfSaved { leaving_project: bool },
    /// Close this tab, if it saved, for the tab dialog's Save.
    CloseTabIfSaved(u64),
}

impl AfterSaves {
    fn same_as(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::CloseWindow, Self::CloseWindow) | (Self::LeaveProject, Self::LeaveProject) => {
                true
            }
            (Self::CloseTab(a), Self::CloseTab(b)) => a == b,
            _ => false,
        }
    }

    /// How the footer says what the wait is for.
    fn reason(&self) -> Option<&'static str> {
        match self {
            Self::CloseWindow | Self::LeaveIfSaved { .. } => Some("closing"),
            Self::LeaveProject => Some("leaving the project"),
            Self::CloseTab(_) | Self::CloseTabIfSaved(_) => Some("closing the tab"),
        }
    }
}

/// The saves running and what waits on them.
#[derive(Resource, Default)]
pub struct SceneSaves {
    running: Option<RunningSave>,
    queued: Option<PreparedSave>,
    then: Vec<AfterSaves>,
    failures: Vec<TabSaveFailure>,
    before_writing: Option<SaveGate>,
    before_renaming: Option<SaveGate>,
    after_writing: Option<SaveGate>,
}

impl SceneSaves {
    fn writing(&self) -> bool {
        self.running.is_some() || self.queued.is_some()
    }

    pub(crate) fn idle(&self) -> bool {
        !self.writing() && self.then.is_empty()
    }
}

/// Whether a save is running or something is waiting on one.
pub fn saves_in_flight(world: &World) -> bool {
    world
        .get_resource::<SceneSaves>()
        .is_some_and(|saves| !saves.idle())
}

/// Whether `file`, a canonical path, is being written.
pub(crate) fn is_being_saved(world: &World, file: &Path) -> bool {
    world
        .get_resource::<SceneSaves>()
        .and_then(|saves| saves.running.as_ref())
        .is_some_and(|running| running.file == file)
}

/// Whether the tab `id` has a write running or waiting.
pub(crate) fn tab_is_being_saved(world: &World, id: u64) -> bool {
    world.get_resource::<SceneSaves>().is_some_and(|saves| {
        saves.running.as_ref().is_some_and(|r| r.waiting.tab == id)
            || saves.queued.as_ref().is_some_and(|q| q.waiting.tab == id)
    })
}

/// Save the active tab with the main thread free, or open Save As if it has no
/// file. A failure is reported when it is known.
pub fn save_in_background(world: &mut World) {
    if super::save::ready_to_save(world).is_some() {
        return;
    }
    save_active_tab_in_background(world);
}

/// [`save_in_background`] for a tab already known to have a file.
pub(crate) fn save_active_tab_in_background(world: &mut World) {
    if let Err(err) = start_save(world) {
        report_tab_failure(world, err);
    }
    advance(world);
}

/// Take the snapshot of the active tab and hand it to a thread. A prefab tab, a
/// legacy scene being redirected to a new file, and what a hook will not have frames
/// run through, are written before this returns.
fn start_save(world: &mut World) -> Result<(), BevyError> {
    if save_holds_the_frame(world) {
        return super::save::save_scene_inner(world);
    }
    if latest_save_holds_this_state(world) {
        return Ok(());
    }
    let prepared = super::save::prepare_scene_save(world)?;
    if world.get_resource_or_init::<SceneSaves>().writing() {
        queue(world, prepared);
    } else {
        spawn_write(world, prepared);
    }
    update_phase(world);
    Ok(())
}

fn save_holds_the_frame(world: &mut World) -> bool {
    if super::save::active_tab_is_prefab(world) || super::save::active_tab_refusal(world).is_some()
    {
        return true;
    }
    let Some(path) = world.resource::<super::SceneFilePath>().path.clone() else {
        return false;
    };
    path.ends_with(".jsn") || super::hooks::holds_the_frame(world, &path)
}

/// Whether the last save asked for is of this tab, to this file, in this state.
fn latest_save_holds_this_state(world: &World) -> bool {
    let Some(saves) = world.get_resource::<SceneSaves>() else {
        return false;
    };
    let latest = saves
        .queued
        .as_ref()
        .map(|queued| &queued.waiting)
        .or(saves.running.as_ref().map(|running| &running.waiting));
    let Some(latest) = latest else {
        return false;
    };
    let scenes = world.resource::<Scenes>();
    let front = scenes.tabs.get(scenes.active).map(|tab| tab.id);
    front == Some(latest.tab)
        && world.resource::<jackdaw_commands::CommandHistory>().state() == latest.state
        && super::save::document_revision(world) == latest.document
        && world.resource::<super::SceneFilePath>().path.as_deref() == Some(&latest.path)
}

/// Wait behind the running write, in the place of any save already waiting.
fn queue(world: &mut World, prepared: PreparedSave) {
    let mut saves = world.resource_mut::<SceneSaves>();
    let replaced = saves.queued.replace(prepared);
    if let Some(replaced) = replaced {
        if let Some(ast) = replaced.write.ast {
            jackdaw_bsn::drop_document(ast);
        }
        super::hooks::finish_save(world, SyncCell::to_inner(replaced.waiting.hooks), false);
    }
}

fn spawn_write(world: &mut World, prepared: PreparedSave) {
    let PreparedSave { mut write, waiting } = prepared;
    write.expect_known_file(world);
    write.durable = true;
    let landed = write.landed.clone();
    let mut saves = world.get_resource_or_init::<SceneSaves>();
    write.before_writing.clone_from(&saves.before_writing);
    write.before_renaming.clone_from(&saves.before_renaming);
    write.after_writing.clone_from(&saves.after_writing);
    let file = canonical(Path::new(&waiting.path));
    let task = WorkerTask::spawn_thread("scene-save", WRITE_THREAD_STACK, move || write.write());
    match task {
        Ok(task) => {
            saves.running = Some(RunningSave {
                task,
                waiting,
                file,
                landed,
            });
        }
        Err(err) => {
            let detail = format!("could not start the write of {}: {err}", waiting.path);
            land(world, waiting, stopped_write(detail), &file);
        }
    }
}

fn stopped_write(detail: String) -> WrittenScene {
    WrittenScene {
        outcome: Err(detail),
        brush_bytes: 0,
    }
}

fn take_running(world: &mut World) -> Option<RunningSave> {
    world.get_resource_mut::<SceneSaves>()?.running.take()
}

/// Wait here until no write is running or waiting. What waits on the saves goes on
/// the next frame, not inside the caller.
pub(crate) fn land_saves(world: &mut World) {
    let mut landed = false;
    loop {
        start_queued(world);
        let Some(mut running) = take_running(world) else {
            break;
        };
        let _span = info_span!("save.wait").entered();
        let outcome = running.task.wait();
        land_outcome(world, running, outcome);
        landed = true;
    }
    if landed {
        update_phase(world);
    }
}

/// Say what became of a write, given how its thread ended.
fn land_outcome(world: &mut World, running: RunningSave, outcome: WorkerOutcome<WrittenScene>) {
    let written = match outcome {
        WorkerOutcome::Done(written) => written,
        WorkerOutcome::Running | WorkerOutcome::Panicked => {
            let path = &running.waiting.path;
            match running.landed.get() {
                Some(hash) => {
                    warn!("the write of {path} stopped unexpectedly once the file was in place");
                    WrittenScene {
                        outcome: Ok(*hash),
                        brush_bytes: 0,
                    }
                }
                None => stopped_write(format!(
                    "the write of {path} stopped unexpectedly, before the file was replaced"
                )),
            }
        }
    };
    land(world, running.waiting, written, &running.file);
}

fn land(world: &mut World, waiting: WaitingSave, written: WrittenScene, file: &Path) {
    let (name, kind) = (waiting.tab_name.clone(), waiting.tab_kind.clone());
    if let Err(err) = super::save::finish_scene_save(world, waiting, written) {
        record_failure(world, name, kind, err);
    }
    queue_recheck(world, file);
}

fn start_queued(world: &mut World) {
    let queued = world
        .get_resource_mut::<SceneSaves>()
        .filter(|saves| saves.running.is_none())
        .and_then(|mut saves| saves.queued.take());
    if let Some(prepared) = queued {
        spawn_write(world, prepared);
    }
}

/// Run every save to its end and everything that waits on one, short of what a
/// running modal or held history keeps waiting. For a caller that needs the files on
/// disk and the tabs settled now.
pub fn wait_for_saves(world: &mut World) {
    while saves_in_flight(world) {
        land_saves(world);
        advance(world);
        if tabs_must_stay(world) {
            break;
        }
    }
}

/// Whether what waits on the saves has to wait longer: it closes tabs, leaves the
/// project or starts a game, which must not happen under a running modal or a history
/// held open.
fn tabs_must_stay(world: &mut World) -> bool {
    jackdaw_api_internal::operator::HistoryHold::reason(world).is_some()
        || world
            .query_filtered::<(), With<ActiveModalOperator>>()
            .iter(world)
            .next()
            .is_some()
}

/// Finish a write whose thread is done, and move on.
fn poll_scene_saves(world: &mut World) {
    let Some(mut saves) = world.get_resource_mut::<SceneSaves>() else {
        return;
    };
    if saves.idle() {
        return;
    }
    let outcome = saves.running.as_mut().map(|running| running.task.outcome());
    match outcome {
        Some(WorkerOutcome::Running) | None => {}
        Some(outcome) => {
            if let Some(running) = take_running(world) {
                land_outcome(world, running, outcome);
                start_queued(world);
            }
        }
    }
    advance(world);
    update_phase(world);
}

/// What waits on the saves, if no write is running.
fn advance(world: &mut World) {
    if !world.get_resource_or_init::<SceneSaves>().writing() {
        run_after_saves(world);
    }
    update_phase(world);
}

fn tab_index(world: &World, id: u64) -> Option<usize> {
    world
        .get_resource::<Scenes>()?
        .tabs
        .iter()
        .position(|tab| tab.id == id)
}

/// Save the tabs at `indices`: those behind in one blocking pass, the one in front
/// with the main thread free. Anything [`after_saves`] holds runs when they are done.
pub(crate) fn save_tabs_front_last(world: &mut World, indices: &[usize]) {
    let scenes = world.resource::<Scenes>();
    let active = scenes.active;
    let front = scenes
        .tabs
        .get(active)
        .filter(|_| indices.contains(&active))
        .and_then(|tab| tab.path.clone());
    let behind: Vec<usize> = indices
        .iter()
        .copied()
        .filter(|index| *index != active)
        .collect();
    let failures = crate::scenes::operators::save_tabs(world, &behind);
    if !failures.is_empty() {
        let names: Vec<&str> = failures.iter().map(|f| f.message.as_str()).collect();
        let message = format!("Not saved: {}", names.join("; "));
        crate::scenes::confirm_dialog::error_toast(world, &message);
    }
    for failure in failures {
        keep_failure(world, failure);
    }
    if let Some(path) = front {
        if let Some(mut file) = world.get_resource_mut::<super::SceneFilePath>() {
            file.path = Some(path.to_string_lossy().into_owned());
        }
        save_active_tab_in_background(world);
    } else {
        advance(world);
    }
}

/// Hold `action` until every save is over.
pub(crate) fn after_saves(world: &mut World, action: AfterSaves) {
    let mut saves = world.get_resource_or_init::<SceneSaves>();
    if !saves.then.iter().any(|held| held.same_as(&action)) {
        saves.then.push(action);
    }
    update_phase(world);
}

fn run_after_saves(world: &mut World) {
    if tabs_must_stay(world) {
        return;
    }
    let Some(mut saves) = world.get_resource_mut::<SceneSaves>() else {
        return;
    };
    let then = std::mem::take(&mut saves.then);
    let failures = std::mem::take(&mut saves.failures);
    for action in then {
        match action {
            AfterSaves::CloseWindow => crate::scenes::ask_to_quit(world),
            AfterSaves::LeaveProject => {
                if crate::scenes::confirm_dialog::leave_project_or_confirm(world) {
                    world
                        .resource_mut::<NextState<crate::AppState>>()
                        .set(crate::AppState::ProjectSelect);
                }
            }
            AfterSaves::CloseTab(id) => {
                if let Some(index) = tab_index(world, id) {
                    crate::scenes::operators::scene_close_system(world, index);
                }
            }
            AfterSaves::LeaveIfSaved { leaving_project } => {
                let saved = failures.is_empty()
                    && !world.resource::<Scenes>().tabs.iter().any(|tab| tab.dirty);
                crate::scenes::confirm_dialog::leave_if_saved(world, leaving_project, saved);
            }
            AfterSaves::CloseTabIfSaved(id) => {
                let Some(index) = tab_index(world, id) else {
                    continue;
                };
                if failures.is_empty() && !world.resource::<Scenes>().tabs[index].dirty {
                    crate::scenes::operators::scene_close_system_unprompted(world, index);
                } else {
                    warn!("confirm_dialog: save failed; keeping tab {index} open");
                }
            }
        }
    }
}

fn record_failure(world: &mut World, name: String, kind: TabKind, err: BevyError) {
    error!("scene save failed: {err}");
    report_loudly(world, format!("Not saved: {err}"));
    keep_failure(
        world,
        TabSaveFailure {
            kind,
            message: format!("{name} ({err})"),
        },
    );
}

/// Keep a failure for what is waiting on the saves. With nothing waiting it has been
/// told, and it must not count against what asks to wait later.
fn keep_failure(world: &mut World, failure: TabSaveFailure) {
    let mut saves = world.get_resource_or_init::<SceneSaves>();
    if !saves.then.is_empty() {
        saves.failures.push(failure);
    }
}

fn report_tab_failure(world: &mut World, err: BevyError) {
    let scenes = world.resource::<Scenes>();
    let (name, kind) = scenes.tabs.get(scenes.active).map_or_else(
        || (String::new(), TabKind::Scene),
        |tab| (tab.display_name.clone(), tab.kind.clone()),
    );
    record_failure(world, name, kind, err);
}

/// In the footer and a toast: the tab's title alone is easy to miss, and the user
/// has moved on since asking.
fn report_loudly(world: &mut World, message: String) {
    crate::status_bar::notify_error(world, message.clone());
    crate::scenes::confirm_dialog::error_toast(world, &message);
}

fn update_phase(world: &mut World) {
    let Some(saves) = world.get_resource::<SceneSaves>() else {
        return;
    };
    let name = saves.running.as_ref().map(|running| {
        Path::new(&running.waiting.path)
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
    });
    let reason = saves.then.iter().find_map(AfterSaves::reason);
    let text = match (name, reason) {
        (Some(name), Some(reason)) => Some(format!("Waiting for {name} to save before {reason}")),
        (Some(name), None) => Some(format!("Saving {name}")),
        (None, _) => None,
    };
    match text {
        Some(text) => crate::status_bar::begin_phase(world, PHASE, text),
        None => crate::status_bar::finish_phase(world, PHASE),
    }
}

/// A file is about to stop being written by this process, so no write may be left
/// behind.
fn land_saves_on_exit(world: &mut World) {
    let exiting = world
        .get_resource::<Messages<AppExit>>()
        .is_some_and(|exits| !exits.is_empty());
    if exiting {
        land_saves(world);
    }
}

/// Delete the temporary files of writes whose process died, anywhere in the project
/// but its hidden folders and build output. A file younger than `older_than` may have
/// a live write behind it, in another editor. Returns how many went.
pub fn remove_stale_temporaries(root: &Path, older_than: Duration) -> usize {
    let mut removed = 0;
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if !name.starts_with('.') && name != "target" {
                    folders.push(entry.path());
                }
                continue;
            }
            let stale = jackdaw_api_internal::document::is_temporary_name(&name)
                && entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age >= older_than);
            if stale && std::fs::remove_file(entry.path()).is_ok() {
                info!("removed {}, left by a write that never ended", name);
                removed += 1;
            }
        }
    }
    removed
}

/// Away from the main thread, since a project can be any size.
fn sweep_temporaries_of_project(project: Option<Res<crate::project::ProjectRoot>>) {
    let Some(project) = project else {
        return;
    };
    let root = project.root.clone();
    let swept = std::thread::Builder::new()
        .name("temporary-sweep".to_string())
        .spawn(move || remove_stale_temporaries(&root, STALE_TEMPORARY_AGE));
    if let Err(err) = swept {
        warn!("could not look for temporary files left behind: {err}");
    }
}

/// A point a write waits at, so a test can act while the write is in progress.
#[derive(Clone)]
struct SaveGate(Arc<(Mutex<bool>, Condvar, AtomicBool)>);

impl SaveGate {
    fn new() -> Self {
        Self(Arc::new((
            Mutex::new(false),
            Condvar::new(),
            AtomicBool::new(false),
        )))
    }

    fn wait(&self) {
        let (open, changed, stop) = &*self.0;
        let mut open = open.lock().unwrap_or_else(PoisonError::into_inner);
        while !*open {
            open = changed.wait(open).unwrap_or_else(PoisonError::into_inner);
        }
        assert!(!stop.load(Ordering::Relaxed), "a test stopped this write");
    }

    fn open(&self) {
        let (open, changed, _) = &*self.0;
        *open.lock().unwrap_or_else(PoisonError::into_inner) = true;
        changed.notify_all();
    }
}

/// Keeps writes at the point they were held at until dropped.
#[doc(hidden)]
pub struct SaveHold(SaveGate);

impl SaveHold {
    /// Let go, with the writes held panicking on their threads instead of going on.
    pub fn stop_the_writes(self) {
        self.0.0.2.store(true, Ordering::Relaxed);
    }
}

impl Drop for SaveHold {
    fn drop(&mut self) {
        self.0.open();
    }
}

/// Make the next writes wait, for a test, before they emit anything.
#[doc(hidden)]
pub fn hold_saves_before_writing(world: &mut World) -> SaveHold {
    let gate = SaveGate::new();
    world.get_resource_or_init::<SceneSaves>().before_writing = Some(gate.clone());
    SaveHold(gate)
}

/// Make the next writes wait, for a test, once the sidecars are written and the
/// document's bytes are ready to be renamed into place.
#[doc(hidden)]
pub fn hold_saves_before_renaming(world: &mut World) -> SaveHold {
    let gate = SaveGate::new();
    world.get_resource_or_init::<SceneSaves>().before_renaming = Some(gate.clone());
    SaveHold(gate)
}

/// Make the next writes wait, for a test, once the file is in place.
#[doc(hidden)]
pub fn hold_saves_after_writing(world: &mut World) -> SaveHold {
    let gate = SaveGate::new();
    world.get_resource_or_init::<SceneSaves>().after_writing = Some(gate.clone());
    SaveHold(gate)
}
