use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::asset::{ReflectAsset, ReflectHandle, UntypedAssetId};
use bevy::ecs::component::ComponentId;
use bevy::reflect::{TypeInfo, TypeRegistry};
use bevy::{ecs::reflect::AppTypeRegistry, prelude::*, tasks::AsyncComputeTaskPool};

use super::asset_fields::AbsoluteAssetField;
use super::load::SceneDialogTask;
use super::{SceneDirtyState, SceneFilePath, doc_skip_type_ids, should_skip_component};

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
        return SaveOutcome::Failed;
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
        return SaveOutcome::DialogOpened;
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

/// Point the active tab, and the global scene path, at `path`.
///
/// The tab keeps its contents and its history and takes the new file. The next
/// save writes there, and everything that lives beside a scene follows it. A
/// relative path names a file in the project and is refused when it leaves it.
pub fn retarget_active_scene(world: &mut World, path: &str) -> Result<(), String> {
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

/// Save the active tab's scene text and terrain sidecars, synchronously,
/// on the calling (main) thread.
///
/// The write is not spawned onto an async task, because ordering has to
/// hold across the whole boundary -- sidecars before scene text, dirty
/// state cleared only after every authoritative write lands (see the
/// ordering comments below) -- and that is far simpler to
/// reason about, and to keep correct under future edits, as a single
/// synchronous call than as a task whose completion has to be raced
/// against the next save, the next tab swap, or the app closing mid-write.
///
/// The tradeoff: this blocks the main thread for the duration of the
/// write, and `scene.save_all` (`scenes/operators.rs`) calls this once
/// per dirty tab in a loop, so the block time is `O(tabs)`, not `O(1)`.
/// Scene text and terrain sidecars are not sized to make that
/// noticeable in practice, but a project with many large open tabs is
/// the case to watch. If it ever needs revisiting, the fix is a
/// different concurrency shape for `scene.save_all` specifically, not an
/// async version of this function -- the ordering argument above applies
/// to any single save.
pub(crate) fn save_scene_inner(world: &mut World) -> Result<(), BevyError> {
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
        let pending = super::hooks::begin_save(world, &path.to_string_lossy());
        let written = write_prefab_files(world, &path, &parent, &pending);
        super::hooks::finish_save(world, pending, written.is_ok());
        written?;
        write_alongside(world, &path.to_string_lossy())?;
        // Clear dirty bit + sync history depth so the tab stops showing
        // as unsaved.
        let history_len = world
            .resource::<jackdaw_commands::CommandHistory>()
            .undo_stack
            .len();
        world.resource_mut::<SceneDirtyState>().undo_len_at_save = history_len;
        if let Some(mut scenes) = world.get_resource_mut::<crate::scenes::Scenes>() {
            let active = scenes.active;
            if let Some(tab) = scenes.tabs.get_mut(active) {
                tab.dirty = false;
                tab.history_depth_at_last_check = history_len;
            }
        }
        return Ok(());
    }

    let path = {
        let scene_path = world.resource::<SceneFilePath>();
        scene_path
            .path
            .clone()
            .expect("save_scene_inner called without a path set")
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

    let pending = super::hooks::begin_save(world, &path);
    world.remove_resource::<FileAssetNames>();
    let written = write_scene_files(world, &path, legacy_backup.as_deref(), &pending);
    super::hooks::finish_save(world, pending, written.is_ok());
    written?;
    let named = world
        .remove_resource::<FileAssetNames>()
        .map(|names| names.0)
        .unwrap_or_default();
    world
        .get_resource_or_init::<SavedAssetNames>()
        .0
        .insert(path.clone(), named);

    world.trigger(jackdaw_api_internal::document::SceneSaved {
        path: PathBuf::from(&path),
    });

    write_alongside(world, &path)?;

    // The authoritative scene and sidecars are now on disk. Only now clear
    // dirty state and retarget a redirected tab at its new `.bsn` path.
    let history_len = world
        .resource::<jackdaw_commands::CommandHistory>()
        .undo_stack
        .len();
    world.resource_mut::<SceneDirtyState>().undo_len_at_save = history_len;
    if legacy_backup.is_some() {
        world.resource_mut::<SceneFilePath>().path = Some(path.clone());
    }
    if let Some(mut scenes) = world.get_resource_mut::<crate::scenes::Scenes>() {
        let active = scenes.active;
        if let Some(tab) = scenes.tabs.get_mut(active) {
            tab.dirty = false;
            tab.history_depth_at_last_check = history_len;
            if legacy_backup.is_some() {
                tab.path = Some(PathBuf::from(&path));
            }
        }
    }

    // Persist current editor layout to project.jsn
    save_layout_to_project(world);

    crate::thumbnail::capture_scene_thumbnail(world, Path::new(&path));

    Ok(())
}

/// The prefab tab's document, sidecars and file, for [`save_scene_inner`].
fn write_prefab_files(
    world: &mut World,
    path: &Path,
    parent: &Path,
    pending: &super::hooks::PendingSave,
) -> Result<(), BevyError> {
    let text = emit_bsn_scene_with_inline_assets(world, parent);
    let bsn = jackdaw_bsn::parse_emitted(&text)
        .map_err(|err| BevyError::from(format!("cannot save {}: {err}", path.display())))?;
    world
        .resource_mut::<crate::prefab::PrefabAstCache>()
        .insert(path, bsn);
    let sidecar_base = path.to_string_lossy();
    super::hooks::export_sidecars(world, &sidecar_base, pending)?;
    crate::prefab::operators::save_prefab_to_disk(world, path)
        .map_err(|err| BevyError::from(format!("prefab save failed: {err}")))?;
    Ok(())
}

/// The scene's sidecars and text, for [`save_scene_inner`]. Sidecars land
/// first so a failed one leaves the tab dirty.
fn write_scene_files(
    world: &mut World,
    path: &str,
    legacy_backup: Option<&str>,
    pending: &super::hooks::PendingSave,
) -> Result<(), BevyError> {
    let contents = {
        let parent_path = Path::new(&path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let body = emit_bsn_scene_for_file(world, &parent_path)
            .map_err(|refusal| BevyError::from(format!("cannot save {path}: {refusal}")))?;
        jackdaw_bsn::parse_emitted(&body)
            .map_err(|err| BevyError::from(format!("cannot save {path}: {err}")))?;
        // Record the jackdaw + Bevy version at the disk boundary only, so
        // the in-memory undo / tab-swap snapshots from the same emitter stay
        // stamp-free.
        crate::scene_io::stamp::with_stamp(&body)
    };

    // Sidecars (terrain heights and paint channels, tilemap data) are
    // authoritative authored data. Complete those writes before the scene text is committed and before
    // the tab is marked clean, so failures remain visible and repeated saves
    // cannot finish out of order. Each write lands via write_atomic, so a
    // sidecar failure partway through never truncates one that already
    // landed; the scene text below is written the same way. There is no
    // cross-file rollback if the scene text write fails after sidecars
    // already landed -- the sidecars stay updated and the tab stays dirty,
    // so the next save retries the scene text with the same sidecars.
    // A folder that cannot be made fails the writes below, which say which.
    // Only a project's own folders are made.
    if let Some(folder) = Path::new(&path).parent()
        && world.contains_resource::<crate::project::ProjectRoot>()
        && crate::scenes::operators::resolve_scene_path(world, folder).is_ok()
        && let Err(err) = std::fs::create_dir_all(folder)
    {
        warn!("cannot make the folder for {path}: {err}");
    }
    super::hooks::export_sidecars(world, path, pending)?;

    // A redirected save renames the legacy source to `.jsn.bak` first so a
    // stale `.jsn` cannot shadow the fresh `.bsn` on the next open.
    if let Some(old_path) = legacy_backup.as_ref() {
        let backup = format!("{old_path}.bak");
        std::fs::rename(old_path, &backup).map_err(|err| {
            BevyError::from(format!(
                "could not back up legacy scene {old_path} to {backup}: {err}"
            ))
        })?;
    }
    let contents = jackdaw_bsn::document_bytes(Path::new(&path), &contents)
        .map_err(|err| BevyError::from(format!("failed to write scene file {path}: {err}")))?;
    write_atomic(Path::new(&path), &contents)
        .map_err(|err| BevyError::from(format!("failed to write scene file {path}: {err}")))?;
    info!("Scene saved to {path}");

    // Record the written bytes so the open-tab watcher does not read this
    // write back as an outside edit.
    crate::scenes::external_watch::note_known_content(world, Path::new(&path), &contents);
    Ok(())
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

/// The `#Name`s the last save of each scene file gave the pathless runtime
/// assets its document uses, by the file's path. The live document does not
/// carry them, so the play sync reads them here to name those assets the way
/// the file the game loaded does.
#[derive(Resource, Default)]
pub(crate) struct SavedAssetNames(
    HashMap<String, bevy::platform::collections::HashMap<UntypedAssetId, String>>,
);

/// The names the latest document authored for a file gave its runtime assets,
/// taken by the save that wrote it.
#[derive(Resource)]
struct FileAssetNames(bevy::platform::collections::HashMap<UntypedAssetId, String>);

/// The names [`SavedAssetNames`] holds for the scene file open now.
pub(crate) fn saved_asset_names(world: &World) -> impl Iterator<Item = (&UntypedAssetId, &String)> {
    let path = world
        .get_resource::<SceneFilePath>()
        .and_then(|scene| scene.path.as_deref());
    world
        .get_resource::<SavedAssetNames>()
        .zip(path)
        .and_then(|(saved, path)| saved.0.get(path))
        .into_iter()
        .flatten()
}

/// What each loaded asset is referenced by when a document emits a handle to
/// it: the path of the file the index holds it under, else the `#Name` the
/// open document embeds it as, else the name the catalog resolves.
///
/// An unsaved material is left out so it embeds inline: a name for it would
/// resolve nowhere outside this editor run.
pub(crate) fn asset_reference_seed(
    world: &World,
) -> bevy::platform::collections::HashMap<UntypedAssetId, String> {
    let mut seed: bevy::platform::collections::HashMap<UntypedAssetId, String> =
        bevy::platform::collections::HashMap::default();
    if let Some(paths) = world.get_resource::<jackdaw_bsn::BsnAssetPaths>() {
        for (id, path) in &paths.0 {
            seed.insert(*id, path.clone());
        }
    }
    if let Some(scene_assets) = world.get_resource::<jackdaw_bsn::BsnSceneAssets>() {
        for (reference, handle) in &scene_assets.0 {
            if reference.starts_with('#') {
                seed.entry(handle.id()).or_insert_with(|| reference.clone());
            }
        }
    }
    let ephemeral = crate::material_assets::ephemeral_material_ids(world);
    if let Some(catalog) = world.get_resource::<crate::asset_catalog::AssetCatalog>() {
        for (id, name) in &catalog.id_to_name {
            if ephemeral.contains(id) {
                continue;
            }
            seed.entry(*id).or_insert_with(|| name.clone());
        }
    }
    seed
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
    /// asset `Handle<T>` field, so it must be re-derived with an asset context
    /// for the reference to survive emission.
    touched: Vec<(Entity, String)>,
}

/// The document's entities in a stable pre-order (roots in order, each followed
/// by its descendants). Asset roots have no ECS mapping and are skipped. A
/// deterministic order makes the generated inline-asset names stable across
/// repeated captures of the same state.
fn doc_entities_in_order(ast: &jackdaw_bsn::SceneBsnAst) -> Vec<Entity> {
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
/// component that references any asset handle is recorded in `touched`, since
/// its patch was maintained without an asset context and must be re-derived.
fn collect_bsn_inline_assets(
    world: &World,
    registry: &TypeRegistry,
    entities: &[Entity],
    mut names: bevy::platform::collections::HashMap<UntypedAssetId, String>,
) -> BsnInlineAssetPass {
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
        let ref_name = format!("{short_name}{counter}");
        *counter += 1;

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
                    found |=
                        collect_bsn_handles_from_reflect(field, registry, names, refs, counters);
                }
            }
        }
        bevy::reflect::ReflectRef::TupleStruct(ts) => {
            for i in 0..ts.field_len() {
                if let Some(field) = ts.field(i) {
                    found |=
                        collect_bsn_handles_from_reflect(field, registry, names, refs, counters);
                }
            }
        }
        bevy::reflect::ReflectRef::Tuple(t) => {
            for i in 0..t.field_len() {
                if let Some(field) = t.field(i) {
                    found |=
                        collect_bsn_handles_from_reflect(field, registry, names, refs, counters);
                }
            }
        }
        bevy::reflect::ReflectRef::List(l) => {
            for i in 0..l.len() {
                if let Some(item) = l.get(i) {
                    found |=
                        collect_bsn_handles_from_reflect(item, registry, names, refs, counters);
                }
            }
        }
        bevy::reflect::ReflectRef::Array(a) => {
            for i in 0..a.len() {
                if let Some(item) = a.get(i) {
                    found |=
                        collect_bsn_handles_from_reflect(item, registry, names, refs, counters);
                }
            }
        }
        bevy::reflect::ReflectRef::Map(m) => {
            for (_k, v) in m.iter() {
                found |= collect_bsn_handles_from_reflect(v, registry, names, refs, counters);
            }
        }
        bevy::reflect::ReflectRef::Set(s) => {
            for item in s.iter() {
                found |= collect_bsn_handles_from_reflect(item, registry, names, refs, counters);
            }
        }
        bevy::reflect::ReflectRef::Enum(e) => {
            for i in 0..e.field_len() {
                if let Some(field) = e.field_at(i) {
                    found |=
                        collect_bsn_handles_from_reflect(field, registry, names, refs, counters);
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
/// The editor maintains the document incrementally with plain component
/// patches, so any asset `Handle<T>` field on a kept component was recorded
/// without an asset context and resolves to an empty string on a bare
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
    emitted(world, &ast, spelling, &registry)
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
}

fn authored_document(
    world: &mut World,
    parent_path: &Path,
    spelling: SourceSpelling,
) -> Option<jackdaw_bsn::SceneBsnAst> {
    let ast = world
        .get_resource::<jackdaw_bsn::SceneBsnAst>()?
        .deep_clone();
    Some(author(world, ast, parent_path, spelling).ast)
}

fn author(
    world: &mut World,
    mut ast: jackdaw_bsn::SceneBsnAst,
    parent_path: &Path,
    spelling: SourceSpelling,
) -> AuthoredDocument {
    let registry = world.resource::<AppTypeRegistry>().clone();
    normalize_derived_button_styles(world, &mut ast, &registry);

    // Name the assets handle fields reference before sparsifying: the live
    // document holds the asset-blind placeholder (`""`) an ECS mirror stored,
    // and compared against a prefab baseline that names the asset, an unchanged
    // inherited component would read as an override.
    let entities = doc_entities_in_order(&ast);
    let pass = name_handles_in(world, &mut ast, &entities, parent_path);

    // Reduce inherited prefab-instance content to sparse override entries
    // (`PrefabEntityId` plus only diverged fields). No-op when there is no
    // prefab cache or no prefab instances.
    if world
        .get_resource::<crate::prefab::PrefabAstCache>()
        .is_some()
    {
        let cache = world.resource::<crate::prefab::PrefabAstCache>();
        let get_prefab = |p: &Path| cache.get(p);
        crate::prefab::resolver_bsn::sparsify_inherited_descendants(&mut ast, &get_prefab);
    }

    // After sparsifying, which reads sources as the cache keys them.
    if spelling == SourceSpelling::RelativeToFile {
        crate::prefab::save_load::relativize_for_file(world, &mut ast, parent_path);
        drop_inherited_visibility(&mut ast);
    }

    normalize_runtime_derived_values(world, &mut ast);
    if spelling == SourceSpelling::RelativeToFile {
        let named = pass
            .refs
            .iter()
            .filter_map(|asset| {
                let name = pass.names.get(&asset.asset_id)?;
                Some((asset.asset_id, name.clone()))
            })
            .collect();
        world.insert_resource(FileAssetNames(named));
    }
    AuthoredDocument {
        ast,
        names_new_assets: !pass.refs.is_empty(),
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
        SourceSpelling::RelativeToFile => crate::scene_io::asset_fields::absolute_asset_fields(
            ast,
            &registry.read(),
            world.get_resource::<crate::project_types::ProjectTypes>(),
        ),
        SourceSpelling::AsHeld => Vec::new(),
    };
    (jackdaw_bsn::emit_scene(ast), named)
}

/// Name the assets `entities` reference in `ast`, a document other than the
/// live one whose nodes are linked to those entities: embed the runtime assets
/// they use and re-derive their handle-bearing patches with the asset context,
/// the way a save does, so a `Handle<T>` field names its asset instead of the
/// placeholder the live document's asset-blind mirror stored.
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
        collect_bsn_inline_assets(world, &reg, entities, seed)
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

/// Re-derive every component patch listed in `pass.touched` from its live ECS
/// value with the asset context, so `Handle<T>` fields emit reference names
/// or asset paths instead of the placeholder an asset-blind capture stored.
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
    let mut kept: Vec<String> = Vec::new();
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
        let mut new_patch = jackdaw_bsn::component_to_bsn_patch_with_assets(
            component.as_partial_reflect(),
            &reg,
            &ctx,
        );
        if let Some(authored) = ast.get_patch(patch_entity) {
            keep_unresolved_references(
                &reg,
                registration.type_id(),
                authored,
                &mut new_patch,
                &mut kept,
            );
        }
        ast.set_patch(patch_entity, new_patch);
    }
    for reference in kept {
        warn!("keeping the asset reference '{reference}', which this project does not resolve");
    }
}

/// Keep a reference the editor could not resolve.
///
/// A handle field whose reference resolved to nothing holds the default handle,
/// which re-derives as an empty string. What the file said is better than
/// nothing: another project, or this one once the file is there, resolves it,
/// so the authored string stays and is reported. The emitter elides a field
/// left at its default, so a kept reference brings its field back with it, and
/// a component whose every field elided comes back as a struct patch.
fn keep_unresolved_references(
    reg: &TypeRegistry,
    type_id: std::any::TypeId,
    authored: &jackdaw_bsn::BsnPatch,
    fresh: &mut jackdaw_bsn::BsnPatch,
    kept: &mut Vec<String>,
) {
    use jackdaw_bsn::BsnPatch;
    match (authored, &mut *fresh) {
        (BsnPatch::Struct(authored), BsnPatch::Struct(fresh)) => {
            keep_in_fields(reg, type_id, &authored.fields, &mut fresh.fields, kept);
        }
        (BsnPatch::TupleStruct(authored), BsnPatch::TupleStruct(fresh)) => {
            keep_in_values(reg, type_id, &authored.values, &mut fresh.values, kept);
        }
        (BsnPatch::Struct(authored), BsnPatch::Type(type_path)) => {
            let type_path = type_path.clone();
            let mut fields = jackdaw_bsn::BsnStructFields::default();
            keep_in_fields(reg, type_id, &authored.fields, &mut fields, kept);
            if !fields.0.is_empty() {
                *fresh = BsnPatch::Struct(jackdaw_bsn::BsnStructData { type_path, fields });
            }
        }
        _ => {}
    }
}

fn keep_in_fields(
    reg: &TypeRegistry,
    type_id: std::any::TypeId,
    authored: &jackdaw_bsn::BsnStructFields,
    fresh: &mut jackdaw_bsn::BsnStructFields,
    kept: &mut Vec<String>,
) {
    let Some(TypeInfo::Struct(info)) = reg
        .get(type_id)
        .map(bevy::reflect::TypeRegistration::type_info)
    else {
        return;
    };
    for field in &mut fresh.0 {
        let Some(field_type) = info
            .field(&field.name)
            .map(bevy::reflect::NamedField::type_id)
        else {
            continue;
        };
        let Some(was) = authored.0.iter().find(|other| other.name == field.name) else {
            continue;
        };
        keep_in_value(reg, field_type, &was.value, &mut field.value, kept);
    }
    for was in &authored.0 {
        if fresh.0.iter().any(|field| field.name == was.name) {
            continue;
        }
        let Some(field_type) = info
            .field(&was.name)
            .map(bevy::reflect::NamedField::type_id)
        else {
            continue;
        };
        let jackdaw_bsn::BsnValue::String(reference) = &was.value else {
            continue;
        };
        if reference.is_empty() || !crate::typed_values::takes_asset_path(reg, field_type) {
            continue;
        }
        if !kept.contains(reference) {
            kept.push(reference.clone());
        }
        fresh.0.push(jackdaw_bsn::BsnField {
            name: was.name.clone(),
            value: was.value.clone(),
        });
    }
}

fn keep_in_values(
    reg: &TypeRegistry,
    type_id: std::any::TypeId,
    authored: &[jackdaw_bsn::BsnValue],
    fresh: &mut [jackdaw_bsn::BsnValue],
    kept: &mut Vec<String>,
) {
    let Some(TypeInfo::TupleStruct(info)) = reg
        .get(type_id)
        .map(bevy::reflect::TypeRegistration::type_info)
    else {
        return;
    };
    for (index, value) in fresh.iter_mut().enumerate() {
        let Some(field_type) = info
            .field_at(index)
            .map(bevy::reflect::UnnamedField::type_id)
        else {
            continue;
        };
        let Some(was) = authored.get(index) else {
            continue;
        };
        keep_in_value(reg, field_type, was, value, kept);
    }
}

fn keep_in_value(
    reg: &TypeRegistry,
    type_id: std::any::TypeId,
    authored: &jackdaw_bsn::BsnValue,
    fresh: &mut jackdaw_bsn::BsnValue,
    kept: &mut Vec<String>,
) {
    use jackdaw_bsn::BsnValue;
    if crate::typed_values::takes_asset_path(reg, type_id) {
        if let (BsnValue::String(was), BsnValue::String(now)) = (authored, &mut *fresh)
            && now.is_empty()
            && !was.is_empty()
        {
            now.clone_from(was);
            if !kept.contains(was) {
                kept.push(was.clone());
            }
        }
        return;
    }
    match (authored, fresh) {
        (BsnValue::Struct(authored), BsnValue::Struct(fresh)) => {
            keep_in_fields(reg, type_id, &authored.fields, &mut fresh.fields, kept);
        }
        (BsnValue::TupleStruct(authored), BsnValue::TupleStruct(fresh)) => {
            keep_in_values(reg, type_id, &authored.values, &mut fresh.values, kept);
        }
        (BsnValue::List(authored), BsnValue::List(fresh)) => {
            let Some(TypeInfo::List(info)) = reg
                .get(type_id)
                .map(bevy::reflect::TypeRegistration::type_info)
            else {
                return;
            };
            let item = info.item_ty().id();
            for (index, value) in fresh.iter_mut().enumerate() {
                let Some(was) = authored.get(index) else {
                    continue;
                };
                keep_in_value(reg, item, was, value, kept);
            }
        }
        _ => {}
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
        collect_bsn_inline_assets(world, &reg, &entities, seed)
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

    /// In Live view the preview world carries streamed game values, so a save
    /// must persist the AST's authored entity payload, not the live overlay.
    /// Authored Transform is `[1, 2, 3]`; the live ECS Transform is `[9, 9, 9]`.
    /// The save must write the authored values.
    fn build_live_save_world() -> World {
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
        world.init_resource::<SceneDirtyState>();

        // Authored node: Transform translation [1, 2, 3], bound to a preview
        // entity whose live ECS Transform is the [9, 9, 9] overlay.
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

        world.insert_resource(crate::pie_mirror::PieViewMode::Live);
        world
    }

    #[test]
    fn live_mode_save_uses_authored_values_not_live_overlays() {
        let mut world = build_live_save_world();

        // A save emits the live document, which holds only authored values;
        // live overlays exist solely on the ECS entities.
        let text = emit_bsn_scene_with_inline_assets(&mut world, Path::new("."));
        let saved = jackdaw_bsn::parse_bsn_text(&text).expect("saved text parses");
        let node = *saved.roots.first().expect("one authored node saved");
        let translation = jackdaw_bsn::get_bsn_field(
            &saved,
            node,
            "bevy_transform::components::transform::Transform",
            "translation.x",
        );
        assert!(
            matches!(translation, Some(jackdaw_bsn::BsnValue::Float(x)) if (x - 1.0).abs() < 1e-6),
            "Live save must persist authored values, not the live overlay",
        );
    }

    #[test]
    fn failed_authoritative_write_keeps_the_scene_dirty() {
        let mut world = build_live_save_world();
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

        let baseline = world.resource::<SceneDirtyState>().undo_len_at_save;
        let error = save_scene_inner(&mut world).expect_err("the blocked sidecar fails the save");

        assert!(error.to_string().contains(BlockedSidecar::NAME));
        assert!(world.resource::<crate::scenes::Scenes>().tabs[0].dirty);
        assert_eq!(
            world.resource::<SceneDirtyState>().undo_len_at_save,
            baseline,
            "failed saves must not advance the clean-history baseline",
        );
    }

    /// A sidecar whose write always fails.
    struct BlockedSidecar;

    impl BlockedSidecar {
        const NAME: &str = "zone.blocked-sidecar";
    }

    impl super::super::hooks::DocumentIoHook for BlockedSidecar {
        fn begin_save(&self, _world: &mut World, _path: &str) -> Box<dyn std::any::Any + Send> {
            Box::new(())
        }

        fn export_sidecars(
            &self,
            _world: &mut World,
            _path: &str,
            _state: &(dyn std::any::Any + Send),
        ) -> Result<(), BevyError> {
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
        let mut world = build_live_save_world();
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
        let mut world = build_live_save_world();
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
        let mut world = build_live_save_world();
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

    /// C2: `write_atomic` must fully replace an existing file's content
    /// (proving the write goes through a rename, not an in-place
    /// truncate) and must not leave its temp file behind.
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
