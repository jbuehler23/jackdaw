use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use bevy::asset::{ReflectAsset, ReflectHandle, UntypedAssetId};
use bevy::ecs::component::ComponentId;
use bevy::platform::cell::SyncCell;
use bevy::reflect::{TypeInfo, TypeRegistry};
use bevy::{ecs::reflect::AppTypeRegistry, prelude::*, tasks::AsyncComputeTaskPool};

use super::asset_fields::AbsoluteAssetField;
use super::asset_names::AssetReferences;
use super::load::SceneDialogTask;
use super::save_worker::{PreparedSave, SceneWrite, WaitingSave, WrittenScene};
use super::{SceneFilePath, doc_skip_type_ids, should_skip_component};
use jackdaw_bsn::AssetNames as _;

pub use jackdaw_api_internal::document::write_atomic;

/// Write the model settings and prefabs edited along with the scene, and say
/// which files the save wrote.
fn write_alongside(world: &mut World, scene: &str) -> Result<(), BevyError> {
    let metas = crate::model_lod::write_unsaved_model_settings(world)
        .map_err(|err| BevyError::from(format!("failed to write model settings: {err}")))?;
    let prefabs = crate::lod_upgrade::write_upgraded_prefabs(world)
        .map_err(|err| BevyError::from(format!("failed to write upgraded prefabs: {err}")))?;
    if metas.is_empty() && prefabs.is_empty() {
        return Ok(());
    }
    let name = |path: &Path| {
        path.file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
    };
    let mut written = vec![name(Path::new(scene))];
    written.extend(prefabs.iter().map(|path| name(path)));
    written.extend(metas.iter().map(|path| name(path)));
    for file in &written {
        info!("saved {file}");
    }
    crate::status_bar::notify_info(
        world,
        format!("Saved {} files: {}", written.len(), summarize(&written)),
    );
    Ok(())
}

/// The first few names of a list, and how many more there are.
fn summarize(names: &[String]) -> String {
    const SHOWN: usize = 4;
    let mut text = names
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if names.len() > SHOWN {
        text.push_str(&format!(" and {} more", names.len() - SHOWN));
    }
    text
}

fn spawn_save_dialog(world: &mut World) {
    let dialog = crate::native_dialog::save_dialog(
        world,
        crate::native_dialog::DialogPurpose::Scene,
        "scene.bsn",
    )
    .add_filter("BSN Scene", &["bsn", "bsb"]);

    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.save_file().await });
    world.insert_resource(SceneDialogTask::Save(task));
}

/// What happened when [`save_scene`] or [`save_scene_with_outcome`] ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SaveOutcome {
    /// The authoritative files reached disk.
    Saved,
    /// The active tab had no path, so a Save As dialog was opened
    /// instead of writing anything. Whether the scene ends up saved
    /// depends on what the user picks and is only known once
    /// `poll_scene_dialog` observes the dialog resolve -- a caller that
    /// needs to act on an eventual success (not just "a save was
    /// attempted") has to defer that action to there, not treat this
    /// outcome as failure and give up.
    DialogOpened,
    /// The save was attempted and failed.
    Failed,
}

/// Save the active scene, returning whether its authoritative files
/// reached disk. A thin `bool` view over [`save_scene_with_outcome`] for
/// callers that only care about "did the save happen right now" and have
/// no work to defer past an opened Save As dialog.
pub fn save_scene(world: &mut World) -> bool {
    save_scene_with_outcome(world) == SaveOutcome::Saved
}

/// Save the active scene, distinguishing an opened Save As dialog from
/// an outright failure. See [`SaveOutcome`].
pub fn save_scene_with_outcome(world: &mut World) -> SaveOutcome {
    if let Some(outcome) = ready_to_save(world) {
        return outcome;
    }
    match save_scene_inner(world) {
        Ok(()) => SaveOutcome::Saved,
        Err(err) => {
            error!("scene save failed: {err}");
            crate::status_bar::notify_error(world, format!("Not saved: {err}"));
            SaveOutcome::Failed
        }
    }
}

/// Point the global scene path at the active tab, or say why there is nothing to
/// write: a refused tab, or an untitled one, for which Save As has been opened.
pub(crate) fn ready_to_save(world: &mut World) -> Option<SaveOutcome> {
    // The active scene tab is the source of truth for which file to
    // save to. Re-sync the global `SceneFilePath` from it so a stale
    // path from a previous tab can never cause us to overwrite the
    // wrong file. Untitled tabs (no path) fall through to Save As.
    // A refused tab keeps its path in front of an empty world, so without
    // this check the save would write that empty world over the file.
    if let Some(reason) = active_tab_refusal(world) {
        error!(
            "Cannot save this tab: its scene was not loaded ({reason}). \
             Fix the file and reopen it; nothing has been written."
        );
        crate::status_bar::notify_error(
            world,
            "Not saved: this tab's scene was not loaded".to_string(),
        );
        return Some(SaveOutcome::Failed);
    }

    let active_tab_path: Option<String> = world
        .get_resource::<crate::scenes::Scenes>()
        .and_then(|s| s.tabs.get(s.active).and_then(|t| t.path.clone()))
        .map(|p| p.to_string_lossy().into_owned());
    if let Some(mut spath) = world.get_resource_mut::<SceneFilePath>() {
        spath.path = active_tab_path;
    }

    let has_path = world.resource::<SceneFilePath>().path.is_some();
    if !has_path {
        save_scene_as(world);
        return Some(SaveOutcome::DialogOpened);
    }
    None
}

/// Point the active tab, and the global scene path, at `path`.
///
/// The tab keeps its contents and its history and takes the new file. The next
/// save writes there, and everything that lives beside a scene follows it. A
/// relative path names a file in the project and is refused when it leaves it.
pub fn retarget_active_scene(world: &mut World, path: &str) -> Result<(), String> {
    super::save_worker::land_saves(world);
    let path = crate::scenes::operators::project_scene_path(world, Path::new(path))?;
    let display_name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".to_string());
    if let Some(mut scene_path) = world.get_resource_mut::<SceneFilePath>() {
        scene_path.path = Some(path.to_string_lossy().into_owned());
    }
    if let Some(mut scenes) = world.get_resource_mut::<crate::scenes::Scenes>() {
        let active = scenes.active;
        if let Some(tab) = scenes.tabs.get_mut(active) {
            tab.path = Some(path.clone());
            tab.display_name = display_name;
        }
    }
    world.trigger(jackdaw_api_internal::document::SceneRenamed { path });
    Ok(())
}

pub fn save_scene_as(world: &mut World) {
    // Save As performs the same write, so a refused tab is refused here too.
    if let Some(reason) = active_tab_refusal(world) {
        error!(
            "Cannot save this tab: its scene was not loaded ({reason}). \
             Fix the file and reopen it; nothing has been written."
        );
        crate::status_bar::notify_error(
            world,
            "Not saved: this tab's scene was not loaded".to_string(),
        );
        return;
    }
    if world.contains_resource::<SceneDialogTask>() {
        return; // Dialog already open
    }
    spawn_save_dialog(world);
}

/// Why the active tab must not be written, if it must not be. A refused
/// activation is the one state where the live world is not the tab's
/// contents.
pub(crate) fn active_tab_refusal(world: &World) -> Option<String> {
    let scenes = world.get_resource::<crate::scenes::Scenes>()?;
    let tab = scenes.tabs.get(scenes.active)?;
    tab.refusal
        .as_ref()
        .map(|refusal| refusal.reason().to_string())
}

/// Save the active tab's scene text and terrain sidecars, the file on disk and the
/// tab settled when this returns. Waits for any save still being written first.
///
/// The steps are those of a background save, see
/// [`super::save_worker`], with the write run here and not forced to the disk.
pub(crate) fn save_scene_inner(world: &mut World) -> Result<(), BevyError> {
    let _span = info_span!("save").entered();
    super::save_worker::land_saves(world);
    // Every write of the world to the active tab's file ends here, so the
    // refusal is enforced here too; callers check only for a better message.
    if let Some(reason) = active_tab_refusal(world) {
        crate::status_bar::notify_error(
            world,
            "Not saved: this tab's scene was not loaded".to_string(),
        );
        return Err(BevyError::from(format!(
            "cannot save this tab: its scene was not loaded ({reason})"
        )));
    }

    // If the active tab is a prefab, flush the live AST into the cache
    // and persist via the prefab-aware writer. Reflect-serializing the
    // live world would drop the `Prefab` marker (its deserializer fails,
    // so the resource never carries it) and turn the file into a regular
    // scene on the next save.
    let prefab_path: Option<PathBuf> = {
        let scenes = world.resource::<crate::scenes::Scenes>();
        scenes
            .tabs
            .get(scenes.active)
            .and_then(|t| match &t.content {
                crate::scenes::TabContent::Prefab(p) => Some(p.as_path().to_path_buf()),
                crate::scenes::TabContent::Scene(_) => None,
            })
    };
    if let Some(path) = prefab_path {
        // Snapshot the live BSN document as the prefab's cached form, then
        // persist it. `save_prefab_to_disk` emits from the cache entry.
        let parent = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let mut pending = super::hooks::begin_save(world, &path.to_string_lossy());
        let written = write_prefab_files(world, &path, &parent, &mut pending);
        super::hooks::finish_save(world, pending, written.is_ok());
        written?;
        write_alongside(world, &path.to_string_lossy())?;
        let (tab, state) = active_tab_and_state(world)?;
        settle_tab_after_save(world, tab, state, None, false);
        return Ok(());
    }

    let mut prepared = prepare_scene_save(world)?;
    prepared.write.expect_known_file(world);
    let written = prepared.write.write();
    finish_scene_save(world, prepared.waiting, written)
}

fn active_tab_and_state(world: &World) -> Result<(u64, u64), BevyError> {
    let scenes = world.resource::<crate::scenes::Scenes>();
    let tab = scenes.tabs.get(scenes.active).ok_or("no tab to save")?;
    let history = world.resource::<jackdaw_commands::CommandHistory>();
    Ok((tab.id, history.state()))
}

/// Whether the tab in front holds a prefab, which saves through its cache.
pub(crate) fn active_tab_is_prefab(world: &World) -> bool {
    let scenes = world.resource::<crate::scenes::Scenes>();
    scenes
        .tabs
        .get(scenes.active)
        .is_some_and(|tab| matches!(tab.content, crate::scenes::TabContent::Prefab(_)))
}

/// The part of a scene save that needs the world: the moment the file will hold,
/// and the sidecars of that moment, encoded. It writes nothing.
///
/// Sidecars land once the document has read back, ahead of its rename. There is no
/// rollback across files: when the rename then fails, the sidecars stay updated, no
/// state on the tab's history counts as saved, so the tab reads dirty until the next
/// save writes both.
pub(crate) fn prepare_scene_save(world: &mut World) -> Result<PreparedSave, BevyError> {
    let _span = info_span!("save.prepare").entered();
    let path = {
        let scene_path = world.resource::<SceneFilePath>();
        scene_path.path.clone().ok_or("no file to save to")?
    };

    // Scenes persist as BSN text. A legacy `.jsn` path redirects to its
    // `.bsn` sibling, keeping the original as a `.jsn.bak` backup (the same
    // convention project conversion uses), and the tab tracks the new path.
    let (path, legacy_backup) = if path.ends_with(".jsn") {
        let bsn_path = Path::new(&path)
            .with_extension("bsn")
            .to_string_lossy()
            .into_owned();
        (bsn_path, Some(path))
    } else {
        (path, None)
    };

    // Refresh the save timestamps carried on the scene metadata.
    let now = crate::timestamps::utc_rfc3339_now();
    let mut scene_path = world.resource_mut::<SceneFilePath>();
    scene_path.metadata.modified = now.clone();
    if scene_path.metadata.created.is_empty() {
        scene_path.metadata.created = now;
    }
    if scene_path.metadata.name.is_empty() {
        scene_path.metadata.name = "Untitled".to_string();
    }

    let scenes = world.resource::<crate::scenes::Scenes>();
    let tab = scenes.tabs.get(scenes.active).ok_or("no tab to save")?;
    let (tab_id, tab_name, tab_kind) = (tab.id, tab.display_name.clone(), tab.kind.clone());
    let state = world.resource::<jackdaw_commands::CommandHistory>().state();

    let mut pending = super::hooks::begin_save(world, &path);
    let (ast, sidecars) = match snapshot_with_sidecars(world, &path, &mut pending) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            super::hooks::finish_save(world, pending, false);
            return Err(err);
        }
    };
    let redirected = legacy_backup.is_some();
    let write = SceneWrite::new(ast, path.clone(), legacy_backup, sidecars);
    let sidecars_started = write.sidecars_started.clone();
    Ok(PreparedSave {
        write,
        waiting: WaitingSave {
            tab: tab_id,
            tab_name,
            tab_kind,
            state,
            history_revision: world
                .resource::<jackdaw_commands::CommandHistory>()
                .revision(),
            document: document_revision(world),
            sidecars_started,
            path,
            redirected,
            hooks: SyncCell::new(pending),
        },
    })
}

/// Stands for the live document's contents: it moves when anything writes to the
/// document, history or not.
pub(crate) fn document_revision(world: &World) -> Option<u32> {
    world
        .get_resource_ref::<jackdaw_bsn::SceneBsnAst>()
        .map(|ast| ast.last_changed().get())
}

/// The live document as the file at `path` will hold it, and its sidecars.
fn snapshot_with_sidecars(
    world: &mut World,
    path: &str,
    pending: &mut super::hooks::PendingSave,
) -> Result<
    (
        Option<jackdaw_bsn::SceneBsnAst>,
        Vec<super::hooks::SidecarWrite>,
    ),
    BevyError,
> {
    let parent_path = Path::new(path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let held = crate::preview_context::suspend_preview_writes(world);
    let authored = authored_document(world, &parent_path, SourceSpelling::RelativeToFile);
    crate::preview_context::resume_preview_writes(world, held);
    if let Some(ast) = &authored {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let named = absolute_fields(world, ast, &registry);
        if !named.is_empty() {
            let refusal = crate::scene_io::asset_fields::refusal(&named);
            return Err(BevyError::from(format!("cannot save {path}: {refusal}")));
        }
    }

    // A folder that cannot be made fails the writes below, which say which.
    // Only a project's own folders are made.
    if let Some(folder) = Path::new(path).parent()
        && world.contains_resource::<crate::project::ProjectRoot>()
        && crate::scenes::operators::resolve_scene_path(world, folder).is_ok()
        && let Err(err) = std::fs::create_dir_all(folder)
    {
        warn!("cannot make the folder for {path}: {err}");
    }
    let sidecars = info_span!("save.sidecars")
        .in_scope(|| super::hooks::encode_sidecars(world, path, pending))?;
    Ok((authored, sidecars))
}

/// The last part of a scene save, on the main thread with the saved tab in front:
/// note what is on disk, say so, and clear the tab's dirty state if nothing changed
/// since the snapshot.
pub(crate) fn finish_scene_save(
    world: &mut World,
    waiting: WaitingSave,
    written: WrittenScene,
) -> Result<(), BevyError> {
    let _span = info_span!("save.finish").entered();
    let WaitingSave {
        tab,
        state,
        history_revision,
        document,
        sidecars_started,
        path,
        redirected,
        hooks,
        ..
    } = waiting;
    let landed = written.outcome.as_ref().ok().copied();
    if let Some(hash) = landed {
        // So the open-tab watcher does not read this write back as an outside edit.
        crate::scenes::external_watch::note_known_hash(world, Path::new(&path), hash);
    }
    if let Err(err) = written.outcome {
        if sidecars_started.load(std::sync::atomic::Ordering::Relaxed) {
            let scenes = world.resource::<crate::scenes::Scenes>();
            if let Some(index) = scenes.tabs.iter().position(|held| held.id == tab) {
                crate::scenes::mark_tab_unsaved(world, index);
            }
        }
        super::hooks::finish_save(world, SyncCell::to_inner(hooks), false);
        return Err(BevyError::from(err));
    }
    info!("Scene saved to {path}");

    if !jackdaw_bsn::is_binary_path(Path::new(&path)) {
        note_brush_text_size(world, &path, written.brush_bytes);
    }

    // A step along the history writes to the document too, and the states say what
    // that means. The document moving with the history still is an edit it never saw.
    let document_moved = world
        .resource::<jackdaw_commands::CommandHistory>()
        .revision()
        == history_revision
        && document_revision(world) != document;
    let alongside = info_span!("save.alongside").in_scope(|| write_alongside(world, &path));
    if alongside.is_ok() {
        settle_tab_after_save(
            world,
            tab,
            state,
            redirected.then_some(&path),
            document_moved,
        );
    }

    // Last of what says the save is over, the tab settled: either may save again.
    super::hooks::finish_save(world, SyncCell::to_inner(hooks), true);
    info_span!("save.saved_observers").in_scope(|| {
        world.trigger(jackdaw_api_internal::document::SceneSaved {
            path: PathBuf::from(&path),
        });
    });
    alongside?;

    // Persist current editor layout to project.jsn
    info_span!("save.layout").in_scope(|| save_layout_to_project(world));

    info_span!("save.thumbnail")
        .in_scope(|| crate::thumbnail::capture_scene_thumbnail(world, Path::new(&path)));

    Ok(())
}

/// The file holds the tab as of history state `state`: the tab reads saved if it is
/// in that state, and dirty if it has moved on, and again once an undo or a redo
/// takes it elsewhere. `redirected` is the new file of a legacy scene. `document_moved`
/// says the document was written to since the snapshot with no history entry, so the
/// file holds no state of the history.
fn settle_tab_after_save(
    world: &mut World,
    tab: u64,
    state: u64,
    redirected: Option<&String>,
    document_moved: bool,
) {
    let scenes = world.resource::<crate::scenes::Scenes>();
    let Some(index) = scenes.tabs.iter().position(|held| held.id == tab) else {
        return;
    };
    let in_front = index == scenes.active;
    if redirected.is_some() && in_front {
        world.resource_mut::<SceneFilePath>().path = redirected.cloned();
    }
    let dirty = if in_front {
        let mut history = world.resource_mut::<jackdaw_commands::CommandHistory>();
        if document_moved {
            history.forget_saved();
        } else {
            history.mark_saved(state);
        }
        !history.is_saved()
    } else {
        let mut scenes = world.resource_mut::<crate::scenes::Scenes>();
        let history = &mut scenes.tabs[index].history;
        history.mark_saved(state);
        !history.is_saved()
    };
    let current = world.resource::<jackdaw_commands::CommandHistory>().state();
    let mut scenes = world.resource_mut::<crate::scenes::Scenes>();
    let saved = &mut scenes.tabs[index];
    saved.dirty = dirty;
    if in_front {
        saved.history_state_at_last_check = Some(current);
    }
    if let Some(path) = redirected {
        saved.path = Some(PathBuf::from(path));
    }
}

/// The prefab tab's document, sidecars and file, for [`save_scene_inner`].
fn write_prefab_files(
    world: &mut World,
    path: &Path,
    parent: &Path,
    pending: &mut super::hooks::PendingSave,
) -> Result<(), BevyError> {
    let text = emit_bsn_scene_with_inline_assets(world, parent);
    let bsn = jackdaw_bsn::parse_emitted(&text)
        .map_err(|err| BevyError::from(format!("cannot save {}: {err}", path.display())))?;
    world
        .resource_mut::<crate::prefab::PrefabAstCache>()
        .insert(path, bsn);
    let sidecar_base = path.to_string_lossy();
    let sidecars = super::hooks::encode_sidecars(world, &sidecar_base, pending)?;
    super::hooks::write_sidecars(&sidecars, false)?;
    if let Err(err) = crate::prefab::operators::save_prefab_to_disk(world, path) {
        if !sidecars.is_empty() {
            let active = world.resource::<crate::scenes::Scenes>().active;
            crate::scenes::mark_tab_unsaved(world, active);
        }
        return Err(BevyError::from(format!("prefab save failed: {err}")));
    }
    Ok(())
}

/// How much brush text a scene holds before a save says it is large.
const LARGE_BRUSH_TEXT_BYTES: usize = 512_000;

/// How many times over a scene's brush text has to grow before a save says so
/// again.
const LARGE_BRUSH_TEXT_GROWTH: f64 = 1.5;

/// The brush text size each large scene was last told about, by path. Kept for
/// the session, so saving the same scene again does not repeat the message.
#[derive(Resource, Default)]
struct LargeSceneNotices(HashMap<String, usize>);

/// Say a scene is large, once for each scene unless it grows well past what
/// was last said.
fn note_brush_text_size(world: &mut World, path: &str, bytes: usize) {
    if !brush_notice_possible(world, path, bytes) {
        return;
    }
    let mut notices = world.get_resource_or_init::<LargeSceneNotices>();
    notices.0.insert(path.to_string(), bytes);
    let message = large_scene_message(bytes);
    warn!("{message}");
    crate::status_bar::notify_warn(world, message);
}

/// Whether a scene whose brush text is at most `bytes` could earn a notice,
/// which it cannot once the user was told of one nearly as large.
fn brush_notice_possible(world: &World, path: &str, bytes: usize) -> bool {
    if bytes < LARGE_BRUSH_TEXT_BYTES {
        return false;
    }
    let told = world
        .get_resource::<LargeSceneNotices>()
        .and_then(|notices| notices.0.get(path).copied());
    !told.is_some_and(|told| (bytes as f64) < told as f64 * LARGE_BRUSH_TEXT_GROWTH)
}

fn large_scene_message(bytes: usize) -> String {
    let megabytes = bytes as f64 / 1_000_000.0;
    format!("Large scene may be slow ({megabytes:.1} MB)")
}

pub fn save_layout_to_project(world: &mut World) {
    let Some(root) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.root.clone())
    else {
        return;
    };

    // Snapshot the live tree into the active workspace before
    // serializing, so the saved registry reflects what's on screen.
    let live_tree = world.resource::<jackdaw_panels::tree::DockTree>().clone();
    let active_id = world
        .resource::<jackdaw_panels::WorkspaceRegistry>()
        .active
        .clone();
    if let Some(id) = active_id {
        let mut registry = world.resource_mut::<jackdaw_panels::WorkspaceRegistry>();
        if let Some(ws) = registry.get_mut(&id) {
            ws.tree = live_tree;
        }
    }

    let persist = jackdaw_panels::WorkspacesPersist::from_registry(
        world.resource::<jackdaw_panels::WorkspaceRegistry>(),
    );
    let layout_json = match serde_json::to_value(&persist) {
        Ok(v) => v,
        Err(e) => {
            warn!("Failed to serialize workspaces: {e}");
            return;
        }
    };

    let mut project = world
        .resource_mut::<crate::project::ProjectRoot>()
        .config
        .clone();
    project.layout = Some(layout_json);

    if let Err(e) = crate::project::save_project_config(&root, &project) {
        warn!("Failed to save project config: {e}");
    } else {
        world.resource_mut::<crate::project::ProjectRoot>().config = project;
    }
}

/// What each loaded asset is referenced by when a document emits a handle to
/// it: the path of the file the index holds it under, else the `#Name` the
/// open document embeds it as, else the name the catalog resolves.
///
/// An unsaved material is left out so it embeds inline: a name for it would
/// resolve nowhere outside this editor run.
pub fn asset_reference_seed(
    world: &World,
) -> bevy::platform::collections::HashMap<UntypedAssetId, String> {
    let references = AssetReferences::new(world);
    references
        .known_assets()
        .into_iter()
        .filter_map(|id| Some((id, references.name_of(id)?)))
        .collect()
}

/// The runtime inline assets referenced by a BSN document's kept components,
/// collected at emit time so they can be embedded and resolved.
pub(crate) struct BsnInlineAssetPass {
    /// Asset id to the reference string emitted for its `Handle<T>` fields:
    /// `#Name` for scene-inline entries and `@Name` for catalog entries.
    names: bevy::platform::collections::HashMap<UntypedAssetId, String>,
    /// New runtime assets that must be embedded as `#Name` roots in the
    /// emitted document. Already-embedded scene assets and catalog assets are
    /// excluded (they resolve through their existing roots or the catalog).
    refs: Vec<jackdaw_bsn::CatalogAssetRef>,
    /// The (entity, component type path) pairs whose patch carries at least one
    /// asset `Handle<T>` field, re-derived so the names this pass settled on reach
    /// the emitted text.
    touched: Vec<(Entity, String)>,
}

/// The document's entities in a stable pre-order (roots in order, each followed
/// by its descendants). Asset roots have no ECS mapping and are skipped. A
/// deterministic order makes the generated inline-asset names stable across
/// repeated captures of the same state.
pub(crate) fn doc_entities_in_order(ast: &jackdaw_bsn::SceneBsnAst) -> Vec<Entity> {
    fn visit(ast: &jackdaw_bsn::SceneBsnAst, node: Entity, out: &mut Vec<Entity>) {
        if let Some(ecs) = ast.ecs_for_ast(node) {
            out.push(ecs);
        }
        for child in ast.get_children_ast(node) {
            visit(ast, child, out);
        }
    }
    let mut out = Vec::new();
    for &root in &ast.roots {
        visit(ast, root, &mut out);
    }
    out
}

/// Walk every kept component on `entities` for asset `Handle<T>` fields.
///
/// `names` is pre-seeded with the assets that already resolve (catalog `@Name`
/// and scene-inline `#Name` entries); any handle found there is left alone.
/// Pathless runtime handles that are not yet known get a fresh `#Name` and a
/// [`jackdaw_bsn::CatalogAssetRef`] so they embed as document roots. Every
/// component that references any asset handle is recorded in `touched`, whose
/// patch is derived again so an asset named just now reaches it.
fn collect_bsn_inline_assets(
    world: &World,
    registry: &TypeRegistry,
    entities: &[Entity],
    mut names: bevy::platform::collections::HashMap<UntypedAssetId, String>,
    mut taken: HashSet<String>,
) -> BsnInlineAssetPass {
    taken.extend(
        names
            .values()
            .filter_map(|reference| reference.get(1..))
            .map(str::to_owned),
    );
    let kept = KeptComponents::new(registry);
    let sprite_render = crate::scene_io::sprite_render_component_paths();

    let mut refs: Vec<jackdaw_bsn::CatalogAssetRef> = Vec::new();
    let mut touched: Vec<(Entity, String)> = Vec::new();
    let mut counters: HashMap<String, usize> = HashMap::new();
    let mut slots: HashMap<ComponentId, Option<usize>> = HashMap::new();

    for &entity in entities {
        let Ok(entity_ref) = world.get_entity(entity) else {
            continue;
        };
        let mut present: Vec<usize> = entity_ref
            .archetype()
            .components()
            .iter()
            .filter_map(|&id| *slots.entry(id).or_insert_with(|| kept.slot(world, id)))
            .collect();
        present.sort_unstable();
        let sprite = entity_ref.contains::<Sprite>();
        for slot in present {
            let (reflect_component, type_path) = kept.components[slot];
            if sprite && sprite_render.contains(&type_path) {
                continue;
            }
            let Some(component) = reflect_component.reflect(entity_ref) else {
                continue;
            };
            let found = collect_bsn_handles_from_reflect(
                component.as_partial_reflect(),
                registry,
                &mut names,
                &mut refs,
                &mut counters,
                &mut taken,
            );
            if found {
                touched.push((entity, type_path.to_string()));
            }
        }
    }

    BsnInlineAssetPass {
        names,
        refs,
        touched,
    }
}

/// Every `#Name` a document root or descendant already carries, entity and
/// asset alike, so a generated asset name cannot share a spelling with one.
fn names_in_document(ast: &jackdaw_bsn::SceneBsnAst) -> HashSet<String> {
    let mut names = HashSet::new();
    for &root in &ast.roots {
        for node in std::iter::once(root).chain(ast.descendants_of(root)) {
            if let Some(name) = ast.get_name(node) {
                names.insert(name.to_owned());
            }
        }
    }
    names
}

/// The reflected components a saved document keeps, in type registry order,
/// so a walk over one entity's own components visits them in the same order
/// as a walk over the whole registry.
struct KeptComponents<'a> {
    components: Vec<(&'a ReflectComponent, &'a str)>,
    slots: HashMap<std::any::TypeId, usize>,
}

impl<'a> KeptComponents<'a> {
    fn new(registry: &'a TypeRegistry) -> Self {
        let skip_ids = doc_skip_type_ids();
        let mut components = Vec::new();
        let mut slots = HashMap::new();
        for registration in registry.iter() {
            if skip_ids.contains(&registration.type_id()) {
                continue;
            }
            let type_path = registration.type_info().type_path_table().path();
            if should_skip_component(type_path) {
                continue;
            }
            let Some(reflect_component) = registration.data::<ReflectComponent>() else {
                continue;
            };
            slots.insert(registration.type_id(), components.len());
            components.push((reflect_component, type_path));
        }
        Self { components, slots }
    }

    /// Where the component `id` sits in [`Self::components`], if it is kept.
    fn slot(&self, world: &World, id: ComponentId) -> Option<usize> {
        let type_id = world.components().get_info(id)?.type_id()?;
        self.slots.get(&type_id).copied()
    }
}

/// Recursively walk a reflected value for asset `Handle<T>` fields. Returns
/// whether any asset handle was seen. Pathless runtime handles not already in
/// `names` are named and pushed to `refs`; path-backed, UUID, catalog, and
/// already-known handles are recorded as seen but neither renamed nor embedded.
fn collect_bsn_handles_from_reflect(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    names: &mut bevy::platform::collections::HashMap<UntypedAssetId, String>,
    refs: &mut Vec<jackdaw_bsn::CatalogAssetRef>,
    counters: &mut HashMap<String, usize>,
    taken: &mut HashSet<String>,
) -> bool {
    let Some(value) = value.try_as_reflect() else {
        return false;
    };
    let type_id = value.reflect_type_info().type_id();

    if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(type_id) {
        let Some(untyped_handle) = reflect_handle.downcast_handle_untyped(value.as_any()) else {
            return false;
        };
        let id = untyped_handle.id();

        // Already resolvable (catalog, an existing scene root, or collected
        // earlier in this walk): the component still needs re-deriving so the
        // reference survives, but no new root is embedded.
        if names.contains_key(&id) {
            return true;
        }
        // File-backed handles emit as their asset path through the asset
        // server; nothing to embed.
        if untyped_handle.path().is_some() {
            return true;
        }
        // Default / UUID handles are not backed by a live asset.
        if matches!(untyped_handle, UntypedHandle::Uuid { .. }) {
            return true;
        }

        let asset_type_id = reflect_handle.asset_type_id();
        let Some(asset_registration) = registry.get(asset_type_id) else {
            return true;
        };
        if asset_registration.data::<ReflectAsset>().is_none() {
            return true;
        }
        let asset_type_path = asset_registration
            .type_info()
            .type_path_table()
            .path()
            .to_string();
        let counter = counters.entry(asset_type_path.clone()).or_insert(0);
        let short_name = asset_type_path
            .rsplit("::")
            .next()
            .unwrap_or(&asset_type_path);
        let ref_name = loop {
            let candidate = format!("{short_name}{counter}");
            *counter += 1;
            if taken.insert(candidate.clone()) {
                break candidate;
            }
        };

        names.insert(id, format!("#{ref_name}"));
        refs.push(jackdaw_bsn::CatalogAssetRef {
            name: ref_name,
            type_id: asset_type_id,
            asset_id: id,
        });
        return true;
    }

    let mut found = false;
    #[expect(
        clippy::allow_attributes,
        reason = "this match is exhaustive or not depending on Bevy feature unification"
    )]
    #[allow(
        unreachable_patterns,
        reason = "ReflectRef gains host-only variants when reflection function support is unified"
    )]
    match value.reflect_ref() {
        bevy::reflect::ReflectRef::Struct(s) => {
            for i in 0..s.field_len() {
                if let Some(field) = s.field_at(i) {
                    found |= collect_bsn_handles_from_reflect(
                        field, registry, names, refs, counters, taken,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::TupleStruct(ts) => {
            for i in 0..ts.field_len() {
                if let Some(field) = ts.field(i) {
                    found |= collect_bsn_handles_from_reflect(
                        field, registry, names, refs, counters, taken,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::Tuple(t) => {
            for i in 0..t.field_len() {
                if let Some(field) = t.field(i) {
                    found |= collect_bsn_handles_from_reflect(
                        field, registry, names, refs, counters, taken,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::List(l) => {
            for i in 0..l.len() {
                if let Some(item) = l.get(i) {
                    found |= collect_bsn_handles_from_reflect(
                        item, registry, names, refs, counters, taken,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::Array(a) => {
            for i in 0..a.len() {
                if let Some(item) = a.get(i) {
                    found |= collect_bsn_handles_from_reflect(
                        item, registry, names, refs, counters, taken,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::Map(m) => {
            for (_k, v) in m.iter() {
                found |=
                    collect_bsn_handles_from_reflect(v, registry, names, refs, counters, taken);
            }
        }
        bevy::reflect::ReflectRef::Set(s) => {
            for item in s.iter() {
                found |=
                    collect_bsn_handles_from_reflect(item, registry, names, refs, counters, taken);
            }
        }
        bevy::reflect::ReflectRef::Enum(e) => {
            for i in 0..e.field_len() {
                if let Some(field) = e.field_at(i) {
                    found |= collect_bsn_handles_from_reflect(
                        field, registry, names, refs, counters, taken,
                    );
                }
            }
        }
        // Opaque values and host-only variants such as reflected functions
        // cannot contain serializable asset handles.
        _ => {}
    }
    found
}

/// Reset a button's theme tokens to the resting values for its variant.
///
/// feathers writes interaction state into the same components that hold a
/// button's authored styling, so a save taken with the pointer on a button
/// would otherwise record its hover token. Runs on the emission clone.
fn normalize_derived_button_styles(
    world: &World,
    ast: &mut jackdaw_bsn::SceneBsnAst,
    registry: &AppTypeRegistry,
) {
    use bevy::feathers::{
        controls::ButtonVariant,
        theme::{InheritableThemeTextColor, ThemeBackgroundColor},
        tokens,
    };
    use bevy::ui::InteractionDisabled;

    let reg = registry.read();
    for entity in doc_entities_in_order(ast) {
        let Ok(entity_ref) = world.get_entity(entity) else {
            continue;
        };
        let Some(variant) = entity_ref.get::<ButtonVariant>() else {
            continue;
        };
        // A disabled button rests disabled, so that token is authored state.
        let disabled = entity_ref.contains::<InteractionDisabled>();
        let background = match (variant, disabled) {
            (ButtonVariant::Normal, false) => tokens::BUTTON_BG,
            (ButtonVariant::Normal, true) => tokens::BUTTON_BG_DISABLED,
            (ButtonVariant::Primary, false) => tokens::BUTTON_PRIMARY_BG,
            (ButtonVariant::Primary, true) => tokens::BUTTON_PRIMARY_BG_DISABLED,
            (ButtonVariant::Plain, false) => tokens::BUTTON_PLAIN_BG,
            (ButtonVariant::Plain, true) => tokens::BUTTON_PLAIN_BG_DISABLED,
        };
        let text = match (variant, disabled) {
            (ButtonVariant::Primary, false) => tokens::BUTTON_PRIMARY_TEXT,
            (ButtonVariant::Primary, true) => tokens::BUTTON_PRIMARY_TEXT_DISABLED,
            (_, false) => tokens::BUTTON_TEXT,
            (_, true) => tokens::BUTTON_TEXT_DISABLED,
        };
        replace_patch(ast, entity, &reg, &ThemeBackgroundColor(background));
        replace_patch(ast, entity, &reg, &InheritableThemeTextColor(text));
    }
}

/// Take the values a widget derives for itself -- a progress fill's `width`, a
/// separator's long axis and `flex_shrink`, a nine-patch's sliced image mode --
/// back out of the document, so they are not recorded as authored.
///
/// Only fields are removed, never whole patches, and only from the emission
/// clone. Runs last: re-deriving a patch from its live component is what put
/// the image mode back.
fn normalize_runtime_derived_values(world: &World, ast: &mut jackdaw_bsn::SceneBsnAst) {
    use jackdaw_widgets_runtime::{NineSlice, ProgressFill, Separator};

    let node_path = crate::inspector::node_card::node_type_path();
    let image_path = <ImageNode as bevy::reflect::TypePath>::type_path();
    for entity in doc_entities_in_order(ast) {
        let Ok(entity_ref) = world.get_entity(entity) else {
            continue;
        };
        let Some(node) = ast.ast_for(entity) else {
            continue;
        };
        for path in crate::scene_io::computed_ui_component_paths() {
            ast.remove_component_patch(node, path);
        }
        if entity_ref.contains::<Sprite>() {
            for path in crate::scene_io::sprite_render_component_paths() {
                ast.remove_component_patch(node, path);
            }
        }
        if entity_ref.contains::<ProgressFill>() {
            jackdaw_bsn::remove_bsn_field(ast, node, node_path, "width");
        }
        if entity_ref.contains::<Separator>() {
            let along_the_flow = entity_ref
                .get::<ChildOf>()
                .and_then(|child_of| world.get::<Node>(child_of.parent()))
                .is_some_and(|parent| {
                    matches!(
                        parent.flex_direction,
                        FlexDirection::Column | FlexDirection::ColumnReverse
                    )
                });
            let filled = if along_the_flow { "width" } else { "height" };
            jackdaw_bsn::remove_bsn_field(ast, node, node_path, filled);
            jackdaw_bsn::remove_bsn_field(ast, node, node_path, "flex_shrink");
        }
        if entity_ref.contains::<NineSlice>() {
            jackdaw_bsn::remove_bsn_field(ast, node, image_path, "image_mode");
        }
    }
}

/// Leave `Visibility::Inherited` out of a file: a load gives it to every entity
/// with a `Transform` that has none. Inside a prefab instance it stays, since
/// there it can override the prefab's own visibility.
fn drop_inherited_visibility(ast: &mut jackdaw_bsn::SceneBsnAst) {
    let visibility = <Visibility as bevy::reflect::TypePath>::type_path();
    let inherited = format!("{visibility}::Inherited");
    for node in ast.entities_with_component(visibility) {
        let Some(patch) = ast.find_patch_by_type_path(node, visibility) else {
            continue;
        };
        let is_inherited = matches!(ast.get_patch(patch), Some(jackdaw_bsn::BsnPatch::Type(path)) if *path == inherited);
        let in_instance = ast
            .ancestor_with_component(node, jackdaw_prefab::ISA_TYPE)
            .is_some();
        if is_inherited && !in_instance {
            ast.remove_component_patch(node, visibility);
        }
    }
}

/// Rewrite `entity`'s existing patch for `component`'s type, adding none.
fn replace_patch(
    ast: &mut jackdaw_bsn::SceneBsnAst,
    entity: Entity,
    registry: &TypeRegistry,
    component: &dyn Reflect,
) {
    let type_path = component.reflect_type_path();
    let Some(node) = ast.ast_for(entity) else {
        return;
    };
    let Some(patch) = ast.find_patch_by_type_path(node, type_path) else {
        return;
    };
    let new_patch = jackdaw_bsn::component_to_bsn_patch(component.as_partial_reflect(), registry);
    ast.set_patch(patch, new_patch);
}

/// Emit the live BSN document to text with runtime inline assets embedded and
/// their handle references resolved.
///
/// The live document names the assets it can, but an asset held only in memory
/// has no name and reads as an empty string on a bare
/// [`jackdaw_bsn::emit_scene`]. This does a capture-time asset pass: it walks
/// the document's entities for pathless runtime asset handles, embeds each as
/// a `#Name` root, and re-derives the handle-bearing component patches so
/// their fields emit the reference names. Assets that already carry a
/// filesystem path or a catalog `@Name`, and scene assets already embedded as
/// roots, resolve through their existing sources. Inherited prefab-instance
/// content reduces to sparse override entries.
///
/// All of that happens on a deep clone of the document, so emission never
/// mutates the live state.
pub fn emit_bsn_scene_with_inline_assets(world: &mut World, parent_path: &Path) -> String {
    emit_bsn_scene(world, parent_path, SourceSpelling::AsHeld).0
}

/// [`emit_bsn_scene_with_inline_assets`] for text that becomes a file at
/// `parent_path`, with every asset named under the project's assets folder.
///
/// Refuses, naming the field, when a field that names an asset still holds an
/// absolute path once the assets-relative spelling has been written: such a
/// path holds only on the machine that saved it.
pub fn emit_bsn_scene_for_file(world: &mut World, parent_path: &Path) -> Result<String, String> {
    let (text, named) = emit_bsn_scene(world, parent_path, SourceSpelling::RelativeToFile);
    match named.is_empty() {
        true => Ok(text),
        false => Err(crate::scene_io::asset_fields::refusal(&named)),
    }
}

/// A document for a running game, authored as a save authors it.
pub(crate) struct GameDocument {
    pub(crate) ast: jackdaw_bsn::SceneBsnAst,
    /// The fields naming an asset by an absolute path, which a save would refuse
    /// and Play only warns about.
    pub(crate) absolute: Vec<AbsoluteAssetField>,
    /// The asset behind each root the text embeds, by name without the `#`, where
    /// the live stores hold it.
    pub(crate) live_assets: bevy::platform::collections::HashMap<String, UntypedAssetId>,
}

/// The live document as the game spawns it in place of its file at
/// `parent_path`. Writes nothing.
pub(crate) fn live_document_for_game(world: &mut World, parent_path: &Path) -> GameDocument {
    let held = crate::preview_context::suspend_preview_writes(world);
    let authored = authored_live(world, parent_path, SourceSpelling::RelativeToFile);
    crate::preview_context::resume_preview_writes(world, held);
    let Some(authored) = authored else {
        return GameDocument {
            ast: jackdaw_bsn::SceneBsnAst::default(),
            absolute: Vec::new(),
            live_assets: Default::default(),
        };
    };
    game_document(world, authored)
}

/// `part`, a copy of some of the live document's nodes still linked to their
/// entities, the way [`live_document_for_game`] makes the whole document.
pub(crate) fn part_of_live_document_for_game(
    world: &mut World,
    part: jackdaw_bsn::SceneBsnAst,
    parent_path: &Path,
) -> GameDocument {
    let held = crate::preview_context::suspend_preview_writes(world);
    let authored = author(world, part, parent_path, SourceSpelling::RelativeToFile);
    crate::preview_context::resume_preview_writes(world, held);
    game_document(world, authored)
}

fn game_document(world: &World, authored: AuthoredDocument) -> GameDocument {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let absolute = absolute_fields(world, &authored.ast, &registry);
    let mut live_assets: bevy::platform::collections::HashMap<String, UntypedAssetId> = world
        .get_resource::<jackdaw_bsn::BsnSceneAssets>()
        .into_iter()
        .flat_map(|assets| &assets.0)
        .filter_map(|(reference, handle)| {
            Some((reference.strip_prefix('#')?.to_string(), handle.id()))
        })
        .collect();
    live_assets.extend(authored.embedded);
    GameDocument {
        ast: authored.ast,
        absolute,
        live_assets,
    }
}

/// `document`, an inactive tab's snapshot, the way [`live_document_for_game`]
/// makes the live one. Its handles were named when the tab was captured, and
/// no store holds its assets under those names.
pub(crate) fn snapshot_for_game(
    world: &World,
    document: &jackdaw_bsn::SceneBsnAst,
    parent_path: &Path,
) -> GameDocument {
    let mut ast = document.deep_clone();
    crate::prefab::save_load::relativize_for_file(world, &mut ast, parent_path);
    drop_inherited_visibility(&mut ast);
    let registry = world.resource::<AppTypeRegistry>().clone();
    let absolute = absolute_fields(world, &ast, &registry);
    GameDocument {
        ast,
        absolute,
        live_assets: Default::default(),
    }
}

/// Run `visit` over every string in `ast`'s component values that can be an asset
/// reference: one the registry types as taking an asset path, and one of a type
/// it does not know, such as a field of a project type. For the second, `visit`
/// must act only on a value that exactly names something it knows.
pub(crate) fn visit_asset_references(
    ast: &mut jackdaw_bsn::SceneBsnAst,
    reg: &TypeRegistry,
    mut visit: impl FnMut(&mut String),
) {
    use jackdaw_bsn::BsnPatch;
    let mut patches = ast.world.query::<&mut BsnPatch>();
    for mut patch in patches.iter_mut(&mut ast.world) {
        match &mut *patch {
            BsnPatch::Struct(data) => {
                let owner = component_type(reg, &data.type_path);
                visit_in_fields(reg, owner, &data.type_path, &mut data.fields, &mut visit);
            }
            BsnPatch::TupleStruct(data) => {
                let owner = component_type(reg, &data.type_path);
                visit_in_tuple(reg, owner, &data.type_path, &mut data.values, &mut visit);
            }
            BsnPatch::Template(type_path, Some(fields)) => {
                let owner = component_type(reg, type_path);
                visit_in_fields(reg, owner, type_path, fields, &mut visit);
            }
            _ => {}
        }
    }
}

/// The type a patch's path names: the path itself, or the enum a
/// `Enum::Variant` path is a variant of.
fn component_type(reg: &TypeRegistry, type_path: &str) -> Option<std::any::TypeId> {
    reg.get_with_type_path(type_path)
        .or_else(|| reg.get_with_type_path(type_path.rsplit_once("::")?.0))
        .map(bevy::reflect::TypeRegistration::type_id)
}

fn visit_in_fields(
    reg: &TypeRegistry,
    owner: Option<std::any::TypeId>,
    type_path: &str,
    fields: &mut jackdaw_bsn::BsnStructFields,
    visit: &mut impl FnMut(&mut String),
) {
    for field in &mut fields.0 {
        let field_type =
            owner.and_then(|owner| named_field_type(reg, owner, type_path, &field.name));
        visit_in_value(reg, field_type, &mut field.value, visit);
    }
}

fn visit_in_tuple(
    reg: &TypeRegistry,
    owner: Option<std::any::TypeId>,
    type_path: &str,
    values: &mut [jackdaw_bsn::BsnValue],
    visit: &mut impl FnMut(&mut String),
) {
    for (index, value) in values.iter_mut().enumerate() {
        let field_type = owner.and_then(|owner| tuple_field_type(reg, owner, type_path, index));
        visit_in_value(reg, field_type, value, visit);
    }
}

fn visit_in_value(
    reg: &TypeRegistry,
    value_type: Option<std::any::TypeId>,
    value: &mut jackdaw_bsn::BsnValue,
    visit: &mut impl FnMut(&mut String),
) {
    use jackdaw_bsn::BsnValue;
    match value {
        BsnValue::String(text) => {
            if value_type.is_none_or(|ty| crate::typed_values::takes_asset_path(reg, ty)) {
                visit(text);
            }
        }
        BsnValue::Struct(data) => {
            visit_in_fields(reg, value_type, &data.type_path, &mut data.fields, visit);
        }
        BsnValue::TupleStruct(data) => {
            visit_in_tuple(reg, value_type, &data.type_path, &mut data.values, visit);
        }
        BsnValue::List(items) => {
            for (index, item) in items.iter_mut().enumerate() {
                let item_type = value_type.and_then(|owner| list_item_type(reg, owner, index));
                visit_in_value(reg, item_type, item, visit);
            }
        }
        BsnValue::Map(entries) => {
            let item_type = value_type.and_then(|owner| match reg.get(owner)?.type_info() {
                TypeInfo::Map(info) => Some(info.value_ty().id()),
                _ => None,
            });
            for (_, item) in entries {
                visit_in_value(reg, item_type, item, visit);
            }
        }
        _ => {}
    }
}

/// How the emitted document spells the prefabs its instances point at.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SourceSpelling {
    /// The absolute paths the live document holds.
    AsHeld,
    /// Relative to the file being written.
    RelativeToFile,
}

/// Every emission of the live document passes through here, with preview
/// writes suspended so it captures authored values.
fn emit_bsn_scene(
    world: &mut World,
    parent_path: &Path,
    spelling: SourceSpelling,
) -> (String, Vec<AbsoluteAssetField>) {
    let held = crate::preview_context::suspend_preview_writes(world);
    let emitted = emit_bsn_scene_authored(world, parent_path, spelling);
    crate::preview_context::resume_preview_writes(world, held);
    emitted
}

fn emit_bsn_scene_authored(
    world: &mut World,
    parent_path: &Path,
    spelling: SourceSpelling,
) -> (String, Vec<AbsoluteAssetField>) {
    let Some(ast) = authored_document(world, parent_path, spelling) else {
        return (String::new(), Vec::new());
    };
    let registry = world.resource::<AppTypeRegistry>().clone();
    let text = emitted(world, &ast, spelling, &registry);
    jackdaw_bsn::drop_document(ast);
    text
}

/// The live document as a save writes it, held as a document rather than
/// text: prefab instances reduced to their overrides and handle fields naming
/// the assets the document embeds. Preview writes are suspended around the
/// read, as for every emission.
pub(crate) fn authored_document_for_history(
    world: &mut World,
    parent_path: &Path,
) -> Option<jackdaw_bsn::SceneBsnAst> {
    authored_live_for_history(world, parent_path).map(|authored| authored.ast)
}

/// [`authored_document_for_history`] with what the read found on the way.
pub(crate) fn authored_live_for_history(
    world: &mut World,
    parent_path: &Path,
) -> Option<AuthoredDocument> {
    let ast = world
        .get_resource::<jackdaw_bsn::SceneBsnAst>()?
        .deep_clone();
    Some(author_for_history(world, ast, parent_path))
}

/// `ast`, a copy of some of the live document's nodes still linked to their
/// entities, made the way [`authored_document_for_history`] makes the whole
/// document.
pub(crate) fn author_for_history(
    world: &mut World,
    ast: jackdaw_bsn::SceneBsnAst,
    parent_path: &Path,
) -> AuthoredDocument {
    let held = crate::preview_context::suspend_preview_writes(world);
    let authored = author(world, ast, parent_path, SourceSpelling::AsHeld);
    crate::preview_context::resume_preview_writes(world, held);
    authored
}

/// A document made the way a save writes it.
pub(crate) struct AuthoredDocument {
    pub(crate) ast: jackdaw_bsn::SceneBsnAst,
    /// The entities and component types whose handle fields were written
    /// from the live components rather than the document.
    pub(crate) handles: Vec<(Entity, String)>,
    /// Whether the document embeds runtime assets it named itself, numbered
    /// in the order the whole document reached them.
    pub(crate) names_new_assets: bool,
    /// The runtime assets it named itself, by name without the `#`.
    pub(crate) embedded: Vec<(String, UntypedAssetId)>,
}

/// The live document taken out of the world for a tab to hold while inactive,
/// authored in place as an emission would author a copy: handles named, inherited
/// prefab content sparse, values the world derives gone. It holds no links, since
/// the entities are about to go. `None` when the world holds no document.
pub(crate) fn take_live_document(
    world: &mut World,
    parent_path: &Path,
) -> Option<jackdaw_bsn::SceneBsnAst> {
    let held = crate::preview_context::suspend_preview_writes(world);
    let live = world
        .get_resource_mut::<jackdaw_bsn::SceneBsnAst>()
        .map(|mut live| std::mem::take(&mut *live));
    let authored = live.map(|ast| author(world, ast, parent_path, SourceSpelling::AsHeld).ast);
    crate::preview_context::resume_preview_writes(world, held);
    let mut authored = authored?;
    authored.clear_links();
    Some(authored)
}

fn authored_document(
    world: &mut World,
    parent_path: &Path,
    spelling: SourceSpelling,
) -> Option<jackdaw_bsn::SceneBsnAst> {
    authored_live(world, parent_path, spelling).map(|authored| authored.ast)
}

fn authored_live(
    world: &mut World,
    parent_path: &Path,
    spelling: SourceSpelling,
) -> Option<AuthoredDocument> {
    let ast = world
        .get_resource::<jackdaw_bsn::SceneBsnAst>()?
        .deep_clone();
    Some(author(world, ast, parent_path, spelling))
}

fn author(
    world: &mut World,
    mut ast: jackdaw_bsn::SceneBsnAst,
    parent_path: &Path,
    spelling: SourceSpelling,
) -> AuthoredDocument {
    let _span = info_span!("doc.author").entered();
    let registry = world.resource::<AppTypeRegistry>().clone();
    normalize_derived_button_styles(world, &mut ast, &registry);

    // Name the assets handle fields reference before sparsifying: compared
    // against a prefab baseline that names the asset, an inherited component the
    // live document holds no name for would read as an override.
    let entities = doc_entities_in_order(&ast);
    let pass = info_span!("author.name_handles")
        .in_scope(|| name_handles_in(world, &mut ast, &entities, parent_path));

    // Reduce inherited prefab-instance content to sparse override entries
    // (`PrefabEntityId` plus only diverged fields). No-op when there is no
    // prefab cache or no prefab instances.
    if world
        .get_resource::<crate::prefab::PrefabAstCache>()
        .is_some()
    {
        let _sparsify = info_span!("author.sparsify").entered();
        let cache = world.resource::<crate::prefab::PrefabAstCache>();
        let get_prefab = |p: &Path| cache.get(p);
        let references = AssetReferences::new(world);
        let scene = world.get_resource::<jackdaw_bsn::BsnSceneAssets>();
        let filed_name = |reference: &str| {
            let handle = scene?
                .0
                .get(reference.starts_with('@').then_some(reference)?)?;
            jackdaw_bsn::AssetNames::name_of(&references, handle.id())
        };
        crate::prefab::resolver_bsn::sparsify_inherited_descendants(
            &mut ast,
            &get_prefab,
            &filed_name,
        );
    }

    // After sparsifying, which reads sources as the cache keys them.
    if spelling == SourceSpelling::RelativeToFile {
        let _relativize = info_span!("author.relativize").entered();
        crate::prefab::save_load::relativize_for_file(world, &mut ast, parent_path);
        drop_inherited_visibility(&mut ast);
    }

    info_span!("author.normalize").in_scope(|| normalize_runtime_derived_values(world, &mut ast));
    AuthoredDocument {
        ast,
        names_new_assets: !pass.refs.is_empty(),
        embedded: pass
            .refs
            .iter()
            .map(|asset| (asset.name.clone(), asset.asset_id))
            .collect(),
        handles: pass.touched,
    }
}

/// The emitted text, paired with the fields that still name a file by an
/// absolute path when the text is bound for one.
fn emitted(
    world: &World,
    ast: &jackdaw_bsn::SceneBsnAst,
    spelling: SourceSpelling,
    registry: &AppTypeRegistry,
) -> (String, Vec<AbsoluteAssetField>) {
    let named = match spelling {
        SourceSpelling::RelativeToFile => absolute_fields(world, ast, registry),
        SourceSpelling::AsHeld => Vec::new(),
    };
    (jackdaw_bsn::emit_scene(ast), named)
}

/// The fields of `ast` naming a file by an absolute path.
fn absolute_fields(
    world: &World,
    ast: &jackdaw_bsn::SceneBsnAst,
    registry: &AppTypeRegistry,
) -> Vec<AbsoluteAssetField> {
    info_span!("emit.absolute_fields").in_scope(|| {
        crate::scene_io::asset_fields::absolute_asset_fields(
            ast,
            &registry.read(),
            world.get_resource::<crate::project_types::ProjectTypes>(),
        )
    })
}

/// Name the assets `entities` reference in `ast`, a document other than the
/// live one whose nodes are linked to those entities, the way a save does: embed
/// the runtime assets they use and re-derive their handle-bearing patches.
pub(crate) fn name_handles_in(
    world: &World,
    ast: &mut jackdaw_bsn::SceneBsnAst,
    entities: &[Entity],
    parent_path: &Path,
) -> BsnInlineAssetPass {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let seed = asset_reference_seed(world);
    let pass = {
        let reg = registry.read();
        collect_bsn_inline_assets(world, &reg, entities, seed, names_in_document(ast))
    };
    if pass.touched.is_empty() {
        return pass;
    }
    if !pass.refs.is_empty() {
        jackdaw_bsn::append_assets_to_ast(ast, world, &pass.refs);
    }
    rederive_handle_patches(world, ast, &registry, parent_path, &pass);
    pass
}

/// Re-derive every component patch in `pass.touched` from its live value, so
/// `Handle<T>` fields emit names or paths.
fn rederive_handle_patches(
    world: &World,
    ast: &mut jackdaw_bsn::SceneBsnAst,
    registry: &AppTypeRegistry,
    parent_path: &Path,
    pass: &BsnInlineAssetPass,
) {
    let Some(asset_server) = world.get_resource::<AssetServer>().cloned() else {
        return;
    };
    let reg = registry.read();
    let ctx = jackdaw_bsn::BsnAssetContext {
        asset_server: &asset_server,
        parent_path,
        asset_names: Some(&pass.names),
    };
    let mut defaults = jackdaw_bsn::TypeDefaults::new(&reg);
    for (entity, type_path) in &pass.touched {
        let Some(patches_entity) = ast.ast_for(*entity) else {
            continue;
        };
        let Some(patch_entity) = ast.find_patch_by_type_path(patches_entity, type_path) else {
            continue;
        };
        let Ok(entity_ref) = world.get_entity(*entity) else {
            continue;
        };
        let Some(registration) = reg.get_with_type_path(type_path) else {
            continue;
        };
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            continue;
        };
        let Some(component) = reflect_component.reflect(entity_ref) else {
            continue;
        };
        let mut new_patch = jackdaw_bsn::component_to_bsn_patch_in_pass(
            component.as_partial_reflect(),
            &reg,
            &ctx,
            &mut defaults,
        );
        if let Some(authored) = ast.get_patch(patch_entity) {
            write_clears(&reg, registration.type_id(), authored, &mut new_patch);
        }
        ast.set_patch(patch_entity, new_patch);
    }
}

/// Write a cleared handle field of `fresh` as `""` where `authored` spells the
/// field, which overrides an inherited value where an omitted field would not. A
/// list element is not inherited field by field, so it omits a cleared one.
pub(super) fn write_clears(
    reg: &TypeRegistry,
    type_id: std::any::TypeId,
    authored: &jackdaw_bsn::BsnPatch,
    fresh: &mut jackdaw_bsn::BsnPatch,
) {
    use jackdaw_bsn::BsnPatch;
    let authored = match authored {
        BsnPatch::Struct(data) => &data.fields,
        BsnPatch::Template(_, Some(fields)) => fields,
        _ => return,
    };
    match fresh {
        BsnPatch::Struct(data) => {
            let type_path = data.type_path.clone();
            clears_in_fields(reg, type_id, &type_path, authored, &mut data.fields);
        }
        BsnPatch::Type(type_path) => {
            let type_path = type_path.clone();
            let mut fields = jackdaw_bsn::BsnStructFields::default();
            clears_in_fields(reg, type_id, &type_path, authored, &mut fields);
            if !fields.0.is_empty() {
                *fresh = BsnPatch::Struct(jackdaw_bsn::BsnStructData { type_path, fields });
            }
        }
        _ => {}
    }
}

fn clears_in_fields(
    reg: &TypeRegistry,
    type_id: std::any::TypeId,
    type_path: &str,
    authored: &jackdaw_bsn::BsnStructFields,
    fresh: &mut jackdaw_bsn::BsnStructFields,
) {
    use jackdaw_bsn::BsnValue;
    for was in &authored.0 {
        let Some(field_type) = named_field_type(reg, type_id, type_path, &was.name) else {
            continue;
        };
        match fresh.0.iter_mut().find(|field| field.name == was.name) {
            None if matches!(was.value, BsnValue::String(_))
                && crate::typed_values::takes_asset_path(reg, field_type) =>
            {
                fresh.0.push(jackdaw_bsn::BsnField {
                    name: was.name.clone(),
                    value: BsnValue::String(String::new()),
                });
            }
            Some(jackdaw_bsn::BsnField {
                value: BsnValue::Struct(now),
                ..
            }) => {
                if let BsnValue::Struct(data) = &was.value
                    && same_variant(reg, field_type, &data.type_path, &now.type_path)
                {
                    let path = now.type_path.clone();
                    clears_in_fields(reg, field_type, &path, &data.fields, &mut now.fields);
                }
            }
            _ => {}
        }
    }
}

/// Whether two spellings name one shape of `type_id`: always for a struct, and
/// for an enum when they are the same variant. The two sides of another variant
/// have nothing to line up.
fn same_variant(reg: &TypeRegistry, type_id: std::any::TypeId, a: &str, b: &str) -> bool {
    match reg
        .get(type_id)
        .map(bevy::reflect::TypeRegistration::type_info)
    {
        Some(TypeInfo::Enum(_)) => a == b,
        _ => true,
    }
}

/// The variant `type_path`, `Enum::Variant`, spells of the enum `type_id`.
fn enum_variant<'a>(
    reg: &'a TypeRegistry,
    type_id: std::any::TypeId,
    type_path: &str,
) -> Option<&'a bevy::reflect::enums::VariantInfo> {
    let Some(TypeInfo::Enum(info)) = reg
        .get(type_id)
        .map(bevy::reflect::TypeRegistration::type_info)
    else {
        return None;
    };
    info.variant(type_path.rsplit("::").next()?)
}

/// The type of the field `name` of `type_path`, a struct or a struct variant.
fn named_field_type(
    reg: &TypeRegistry,
    type_id: std::any::TypeId,
    type_path: &str,
    name: &str,
) -> Option<std::any::TypeId> {
    if let Some(TypeInfo::Struct(info)) = reg
        .get(type_id)
        .map(bevy::reflect::TypeRegistration::type_info)
    {
        return info.field(name).map(bevy::reflect::NamedField::type_id);
    }
    match enum_variant(reg, type_id, type_path)? {
        bevy::reflect::enums::VariantInfo::Struct(variant) => {
            variant.field(name).map(bevy::reflect::NamedField::type_id)
        }
        _ => None,
    }
}

/// The type of field `index` of `type_path`, a tuple struct or a tuple variant.
fn tuple_field_type(
    reg: &TypeRegistry,
    type_id: std::any::TypeId,
    type_path: &str,
    index: usize,
) -> Option<std::any::TypeId> {
    if let Some(TypeInfo::TupleStruct(info)) = reg
        .get(type_id)
        .map(bevy::reflect::TypeRegistration::type_info)
    {
        return info
            .field_at(index)
            .map(bevy::reflect::UnnamedField::type_id);
    }
    match enum_variant(reg, type_id, type_path)? {
        bevy::reflect::enums::VariantInfo::Tuple(variant) => variant
            .field_at(index)
            .map(bevy::reflect::UnnamedField::type_id),
        _ => None,
    }
}

/// The type of item `index` of a value the document writes as a list: a list,
/// an array, a set or a tuple.
fn list_item_type(
    reg: &TypeRegistry,
    type_id: std::any::TypeId,
    index: usize,
) -> Option<std::any::TypeId> {
    match reg.get(type_id)?.type_info() {
        TypeInfo::List(info) => Some(info.item_ty().id()),
        TypeInfo::Array(info) => Some(info.item_ty().id()),
        TypeInfo::Set(info) => Some(info.value_ty().id()),
        TypeInfo::Tuple(info) => info
            .field_at(index)
            .map(bevy::reflect::UnnamedField::type_id),
        _ => None,
    }
}

/// Emit a subset of the live BSN document (the given document nodes with
/// their subtrees) as BSN text for the clipboard.
///
/// Same-scene inline `#Name` refs are preserved via the live
/// [`jackdaw_bsn::BsnSceneAssets`] seed, as in a full-scene save. Pathless
/// handles unknown to the live scene embed as `#Name` roots. Stable node ids are
/// stripped so paste can mint fresh ones. Works on a deep clone of the document,
/// so emission does not mutate live state.
pub(crate) fn emit_bsn_entities_with_inline_assets(
    world: &mut World,
    parent_path: &Path,
    nodes: &[Entity],
) -> String {
    let held = crate::preview_context::suspend_preview_writes(world);
    let text = emit_bsn_entities_authored(world, parent_path, nodes);
    crate::preview_context::resume_preview_writes(world, held);
    text
}

fn emit_bsn_entities_authored(world: &mut World, parent_path: &Path, nodes: &[Entity]) -> String {
    let Some(live) = world.get_resource::<jackdaw_bsn::SceneBsnAst>() else {
        return String::new();
    };

    // Translate the live document nodes into the clone through their linked
    // ECS entities (the clone re-mints node entities but keeps the links).
    let node_entities: Vec<Entity> = nodes.iter().filter_map(|&n| live.ecs_for_ast(n)).collect();
    let mut ast = live.deep_clone();
    let clone_nodes: Vec<Entity> = node_entities
        .iter()
        .filter_map(|&e| ast.ast_for(e))
        .collect();

    let registry = world.resource::<AppTypeRegistry>().clone();
    normalize_derived_button_styles(world, &mut ast, &registry);

    let seed = asset_reference_seed(world);

    // The copied subtrees' document nodes and their live ECS entities.
    let mut subtree_nodes: Vec<Entity> = Vec::new();
    for &node in &clone_nodes {
        subtree_nodes.push(node);
        subtree_nodes.extend(ast.descendants_of(node));
    }
    let entities: Vec<Entity> = subtree_nodes
        .iter()
        .filter_map(|&n| ast.ecs_for_ast(n))
        .collect();

    let pass = {
        let reg = registry.read();
        collect_bsn_inline_assets(world, &reg, &entities, seed, names_in_document(&ast))
    };

    // Strip stable node ids from the copied subtrees: paste mints fresh ones before
    // grafting, so the clipboard must not carry the source ids.
    for &node in &subtree_nodes {
        let found = ast.get_patches(node).and_then(|patches| {
            patches.0.iter().copied().find(|&pe| {
                matches!(
                    ast.get_patch(pe),
                    Some(jackdaw_bsn::BsnPatch::TupleStruct(data))
                        if data.type_path.ends_with("SceneNodeId")
                )
            })
        });
        if let Some(pe) = found {
            if let Some(patches) = ast.get_patches_mut(node) {
                patches.0.retain(|&x| x != pe);
            }
            ast.world.despawn(pe);
        }
    }

    // Embed referenced runtime assets as roots and re-derive handle-bearing
    // component patches so their fields emit the reference names.
    let roots_before = ast.roots.len();
    if !pass.touched.is_empty() {
        if !pass.refs.is_empty() {
            jackdaw_bsn::append_assets_to_ast(&mut ast, world, &pass.refs);
        }
        rederive_handle_patches(world, &mut ast, &registry, parent_path, &pass);
    }
    let added_roots: Vec<Entity> = ast.roots[roots_before..].to_vec();

    normalize_runtime_derived_values(world, &mut ast);

    let mut emit_list = added_roots;
    emit_list.extend_from_slice(&clone_nodes);
    jackdaw_bsn::emit_entities(&ast, &emit_list)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_save_renames_the_tab_it_saves() {
        use crate::scenes::{SceneTab, Scenes};

        let mut world = World::new();
        world.init_resource::<SceneFilePath>();
        let mut scenes = Scenes::default();
        scenes.push_tab(SceneTab::new_untitled(1));
        world.insert_resource(scenes);

        retarget_active_scene(&mut world, "/tmp/zones/harbour.bsn").expect("an absolute path");

        assert_eq!(
            world.resource::<SceneFilePath>().path.as_deref(),
            Some("/tmp/zones/harbour.bsn")
        );
        let scenes = world.resource::<Scenes>();
        assert_eq!(scenes.tabs[0].display_name, "harbour");
        assert_eq!(
            scenes.tabs[0].path.as_deref(),
            Some(Path::new("/tmp/zones/harbour.bsn"))
        );
    }

    /// One node whose document Transform is `[1, 2, 3]`.
    fn build_save_world() -> World {
        use jackdaw_bsn::SceneBsnAst;
        use jackdaw_scene_types::SceneNodeId;

        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let mut w = registry.write();
            w.register::<Name>();
            w.register::<Transform>();
            w.register::<SceneNodeId>();
        }
        world.init_resource::<jackdaw_commands::CommandHistory>();
        world.init_resource::<SceneFilePath>();

        let node_id = SceneNodeId::next();
        let preview = world
            .spawn((
                Name::new("Authored"),
                Transform::from_xyz(9.0, 9.0, 9.0),
                node_id,
            ))
            .id();

        let mut ast = SceneBsnAst::default();
        let patches = {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let registry = registry.read();
            vec![
                jackdaw_bsn::component_to_bsn_patch(&Transform::from_xyz(1.0, 2.0, 3.0), &registry),
                jackdaw_bsn::BsnPatch::TupleStruct(jackdaw_bsn::BsnTupleStructData {
                    type_path: jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH.to_string(),
                    values: vec![jackdaw_bsn::BsnValue::Int(node_id.0 as i128)],
                }),
            ]
        };
        let node = ast.create_entity_node(patches);
        ast.add_to_roots(node);
        ast.link(preview, node);
        world.insert_resource(ast);

        world
    }

    #[test]
    fn failed_authoritative_write_keeps_the_scene_dirty() {
        let mut world = build_save_world();
        let tmp = tempfile::tempdir().expect("temp directory");
        let blocked_parent = tmp.path().join("not-a-directory");
        std::fs::write(&blocked_parent, b"file blocks directory creation")
            .expect("create blocking file");
        let scene_path = blocked_parent.join("zone.bsn");

        let mut tab = crate::scenes::SceneTab::new_untitled(1);
        tab.path = Some(scene_path.clone());
        tab.dirty = true;
        world.insert_resource(crate::scenes::Scenes {
            tabs: vec![tab],
            active: 0,
        });
        world.resource_mut::<SceneFilePath>().path =
            Some(scene_path.to_string_lossy().into_owned());

        world.insert_resource(super::super::hooks::DocumentIoHooks::default());
        world
            .resource_mut::<super::super::hooks::DocumentIoHooks>()
            .register(BlockedSidecar);

        let error = save_scene_inner(&mut world).expect_err("the blocked sidecar fails the save");

        assert!(error.to_string().contains(BlockedSidecar::NAME));
        assert!(world.resource::<crate::scenes::Scenes>().tabs[0].dirty);
    }

    /// A sidecar that always fails to encode.
    struct BlockedSidecar;

    impl BlockedSidecar {
        const NAME: &str = "zone.blocked-sidecar";
    }

    impl super::super::hooks::DocumentIoHook for BlockedSidecar {
        fn begin_save(&self, _world: &mut World, _path: &str) -> Box<dyn std::any::Any + Send> {
            Box::new(())
        }

        fn encode_sidecars(
            &self,
            _world: &mut World,
            _path: &str,
            _state: &mut (dyn std::any::Any + Send),
        ) -> Result<Vec<super::super::hooks::SidecarWrite>, BevyError> {
            Err(BevyError::from(format!("cannot write {}", Self::NAME)))
        }

        fn finish_save(
            &self,
            _world: &mut World,
            _state: Box<dyn std::any::Any + Send>,
            _written: bool,
        ) {
        }

        fn import_sidecars(
            &self,
            _world: &mut World,
            _path: &str,
            _mode: super::super::SidecarImport,
        ) {
        }
    }

    /// `save_scene_with_outcome` distinguishes a failure from an opened dialog, so a caller
    /// with work to defer (see `scene_io::load::on_new_scene_save`) does not give up on a
    /// save still in flight.
    #[test]
    fn save_scene_with_outcome_reports_saved_on_success() {
        let mut world = build_save_world();
        let tmp = tempfile::tempdir().expect("temp directory");
        let scene_path = tmp.path().join("zone.bsn");

        let mut tab = crate::scenes::SceneTab::new_untitled(1);
        tab.path = Some(scene_path);
        tab.dirty = true;
        world.insert_resource(crate::scenes::Scenes {
            tabs: vec![tab],
            active: 0,
        });

        assert_eq!(save_scene_with_outcome(&mut world), SaveOutcome::Saved);
    }

    #[test]
    fn save_scene_with_outcome_reports_failed_on_a_blocked_write() {
        let mut world = build_save_world();
        let tmp = tempfile::tempdir().expect("temp directory");
        let blocked_parent = tmp.path().join("not-a-directory");
        std::fs::write(&blocked_parent, b"blocks directory creation").expect("seed blocker");
        let scene_path = blocked_parent.join("zone.bsn");

        let mut tab = crate::scenes::SceneTab::new_untitled(1);
        tab.path = Some(scene_path);
        tab.dirty = true;
        world.insert_resource(crate::scenes::Scenes {
            tabs: vec![tab],
            active: 0,
        });

        assert_eq!(save_scene_with_outcome(&mut world), SaveOutcome::Failed);
    }

    #[test]
    fn successful_scene_write_finishes_before_the_tab_becomes_clean() {
        let mut world = build_save_world();
        let tmp = tempfile::tempdir().expect("temp directory");
        let scene_path = tmp.path().join("zone.bsn");

        let mut tab = crate::scenes::SceneTab::new_untitled(1);
        tab.path = Some(scene_path.clone());
        tab.dirty = true;
        world.insert_resource(crate::scenes::Scenes {
            tabs: vec![tab],
            active: 0,
        });
        world.resource_mut::<SceneFilePath>().path =
            Some(scene_path.to_string_lossy().into_owned());

        save_scene_inner(&mut world).expect("scene write succeeds");

        let saved = std::fs::read_to_string(&scene_path)
            .expect("successful save has already reached disk on return");
        assert!(saved.contains("bevy_transform::components::transform::Transform"));
        assert!(!world.resource::<crate::scenes::Scenes>().tabs[0].dirty);
    }

    #[test]
    fn a_large_scene_is_told_about_once_until_it_grows_well_past_that() {
        let mut world = World::new();
        world.insert_resource(crate::status_bar::StatusNotice::default());
        let notice = |world: &mut World| {
            let mut notice = world.resource_mut::<crate::status_bar::StatusNotice>();
            let text = notice.is_active().then(|| notice.text().to_string());
            *notice = crate::status_bar::StatusNotice::default();
            text
        };

        note_brush_text_size(&mut world, "a/scene.bsn", LARGE_BRUSH_TEXT_BYTES - 1);
        assert_eq!(notice(&mut world), None, "a smaller scene says nothing");

        note_brush_text_size(&mut world, "a/scene.bsn", 1_000_000);
        let said = notice(&mut world).expect("a large scene says so");
        assert_eq!(said, "Large scene may be slow (1.0 MB)");

        note_brush_text_size(&mut world, "a/scene.bsn", 1_400_000);
        assert_eq!(notice(&mut world), None, "saving again does not nag");

        note_brush_text_size(&mut world, "a/scene.bsn", 1_500_000);
        assert!(
            notice(&mut world).is_some(),
            "much larger, so it says again"
        );

        note_brush_text_size(&mut world, "a/other.bsn", 600_000);
        assert!(notice(&mut world).is_some(), "each scene has its own");
    }

    #[test]
    fn write_atomic_replaces_existing_content_and_cleans_up_its_temp_file() {
        let tmp = tempfile::tempdir().expect("temp directory");
        let target = tmp.path().join("scene.bsn");
        std::fs::write(&target, b"old contents, much longer than the new ones")
            .expect("seed original file");

        write_atomic(&target, b"new").expect("atomic write succeeds");

        assert_eq!(std::fs::read(&target).expect("read back"), b"new");
        let stray_temp = std::fs::read_dir(tmp.path())
            .expect("read temp dir")
            .filter_map(Result::ok)
            .any(|entry| entry.path() != target);
        assert!(!stray_temp, "no temp file may remain beside the target");
    }

    /// C2: a failed write must not touch the destination at all -- the
    /// crash-safety property this exists for is that a partial write can
    /// only ever land in the temp file, never in the file callers read.
    #[test]
    fn write_atomic_leaves_the_destination_untouched_on_failure() {
        let tmp = tempfile::tempdir().expect("temp directory");
        let missing_parent = tmp.path().join("does-not-exist").join("scene.bsn");

        let err = write_atomic(&missing_parent, b"new").expect_err("parent dir is missing");
        assert!(!missing_parent.exists());
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }
}
