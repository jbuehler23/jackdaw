use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::asset::{ReflectAsset, ReflectHandle, UntypedAssetId};
use bevy::reflect::TypeRegistry;
use bevy::{ecs::reflect::AppTypeRegistry, prelude::*, tasks::AsyncComputeTaskPool};
use rfd::AsyncFileDialog;

use super::load::SceneDialogTask;
use super::{
    SceneDirtyState, SceneFilePath, get_window_handle, should_skip_component,
    structural_skip_type_ids,
};

/// Write `contents` to `path` atomically: write to a temp file beside
/// `path`, then rename over the target. `std::fs::write` truncates the
/// destination in place, so a crash or a full disk mid-write would leave
/// a truncated file where the last-known-good copy used to be; a rename
/// within one directory is a single filesystem operation and cannot
/// observe a half-written state.
///
/// The temp file is created in `path`'s own directory rather than a
/// system temp dir: a rename across filesystems (e.g. `/tmp` to a
/// project on another mount) is not atomic and fails outright on most
/// platforms.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other(format!("{} has no file name", path.display())))?;
    let temp_path = parent.join(format!(
        ".{}.tmp-{}",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    if let Err(err) = std::fs::write(&temp_path, contents) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&temp_path, path) {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err);
    }
    Ok(())
}

fn spawn_save_dialog(world: &mut World) {
    let raw_handle = get_window_handle(world);
    let last_dir = world.resource::<SceneFilePath>().last_directory.clone();

    let mut dialog = AsyncFileDialog::new()
        .add_filter("BSN Scene", &["bsn"])
        .set_file_name("scene.bsn");

    if let Some(dir) = &last_dir {
        dialog = dialog.set_directory(dir);
    }
    if let Some(ref rh) = raw_handle {
        // SAFETY: called on the main thread during an exclusive system
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }

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
            SaveOutcome::Failed
        }
    }
}

/// Point the active tab, and the global scene path, at `path`.
///
/// The tab keeps its contents and its history and takes the new file. The next
/// save writes there, and everything that lives beside a scene follows it.
pub fn retarget_active_scene(world: &mut World, path: &str) {
    let path = PathBuf::from(path);
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
    // A rename leaves the ground unchanged, so the bake taken from it follows the new name.
    // Left pointed at the old file, the check that stops one scene's navmesh landing beside
    // another's would refuse the next save's artifact write.
    if let Some(mut state) =
        world.get_resource_mut::<crate::terrain::navmesh_bake::TerrainNavmeshState>()
        && let Some(baked) = state.baked.as_mut()
    {
        baked.scene = Some(path);
    }
}

pub fn save_scene_as(world: &mut World) {
    if world.contains_resource::<SceneDialogTask>() {
        return; // Dialog already open
    }
    spawn_save_dialog(world);
}

/// Save the active tab's scene text and terrain sidecars, synchronously,
/// on the calling (main) thread.
///
/// This used to spawn the write onto an async task. It does not anymore:
/// ordering has to hold across the whole boundary -- sidecars before
/// scene text, dirty state cleared only after every authoritative write
/// lands (see the ordering comments below) -- and that is far simpler to
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
/// different concurrency shape for `scene.save_all` specifically, not
/// bringing async back to this function -- the ordering argument above
/// still applies to any single save.
pub(crate) fn save_scene_inner(world: &mut World) -> Result<(), BevyError> {
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
        let text = emit_bsn_scene_with_inline_assets(world, &parent);
        match jackdaw_bsn::parse_bsn_text(&text) {
            Ok(bsn) => {
                world
                    .resource_mut::<crate::prefab::PrefabAstCache>()
                    .insert(&path, bsn);
            }
            Err(err) => warn!("scene.save: prefab snapshot parse failed: {err}"),
        }
        crate::prefab::operators::save_prefab_to_disk(world, &path)
            .map_err(|err| BevyError::from(format!("prefab save failed: {err}")))?;
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

    let contents = {
        let parent_path = Path::new(&path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let body = emit_bsn_scene_with_inline_assets(world, &parent_path);
        // Record the jackdaw + Bevy version at the disk boundary only, so
        // the in-memory undo / tab-swap snapshots from the same emitter stay
        // stamp-free.
        crate::scene_io::stamp::with_stamp(&body)
    };

    // Terrain heights and paint channels are authoritative authored data.
    // Complete those writes before the scene text is committed and before
    // the tab is marked clean, so failures remain visible and repeated saves
    // cannot finish out of order. Each write lands via write_atomic, so a
    // sidecar failure partway through never truncates one that already
    // landed; the scene text below is written the same way. There is no
    // cross-file rollback if the scene text write fails after sidecars
    // already landed -- the sidecars stay updated and the tab stays dirty,
    // so the next save retries the scene text with the same sidecars.
    export_terrain_sidecars(world, &path)?;

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
    write_atomic(Path::new(&path), contents.as_bytes())
        .map_err(|err| BevyError::from(format!("failed to write scene file {path}: {err}")))?;
    info!("Scene saved to {path}");

    // A terrain bake writes its own artifact when it finishes; this covers a scene that has
    // moved since, keeping the two files named after each other.
    crate::terrain::navmesh_bake::export_beside_scene(world, &path);

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

    // Save catalog alongside scene if dirty
    crate::asset_catalog::save_catalog(world);

    // Persist current editor layout to project.jsn
    save_layout_to_project(world);

    Ok(())
}

/// Write every terrain's bulk data to its sidecar beside the scene.
///
/// A scene with no terrain is a clean no-op. Each write completes before
/// returning (atomically, via `write_atomic`), and an invalid path,
/// encoding failure, or filesystem error is returned to the save boundary
/// so the tab stays dirty. One file per terrain is named by the
/// scene-relative `data_path` the `Terrain` component carries, so a scene
/// and its terrain data move together.
///
/// A path with no store entry (never loaded -- its sidecar was missing --
/// and never edited) is silently skipped: there is nothing to write and
/// nothing lost. A path marked load-failed (a sidecar existed but could
/// not decode) is also skipped, with a warning: writing zeroed data over
/// a real, if damaged, file would be worse than leaving it alone. Neither
/// case fails the save.
///
/// Only terrains that are actually in the scene are written. The store
/// can outlive them -- an undo can bring one back, and other tabs keep
/// their own entries -- so writing the whole store would scatter files
/// for terrains this scene does not own.
pub(crate) fn export_terrain_sidecars(
    world: &mut World,
    scene_path: &str,
) -> Result<(), BevyError> {
    use jackdaw_terrain::sidecar;

    if world
        .get_resource::<crate::terrain::TerrainDataStore>()
        .is_none()
    {
        return Ok(());
    }
    let scene_dir = Path::new(scene_path)
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();

    let mut paths: Vec<String> = Vec::new();
    let mut query = world.query::<&jackdaw_scene_types::Terrain>();
    for terrain in query.iter(world) {
        if !terrain.data_path.is_empty() && !paths.contains(&terrain.data_path) {
            paths.push(terrain.data_path.clone());
        }
    }

    let store = world.resource::<crate::terrain::TerrainDataStore>();
    let mut writes: Vec<(PathBuf, Vec<u8>)> = Vec::new();
    for data_path in paths {
        let path = match sidecar::resolve_path(&scene_dir, &data_path) {
            Ok(path) => path,
            Err(err) => {
                return Err(BevyError::from(format!(
                    "invalid terrain data path {data_path:?}: {err}"
                )));
            }
        };
        if store.is_load_failed(&data_path) {
            warn!(
                "Not saving terrain data {data_path:?}: it failed to load, so writing \
                 would overwrite the original file with empty data. Fix or replace the \
                 sidecar and reload the scene."
            );
            continue;
        }
        // No entry means this path was never loaded (its sidecar was
        // missing, and the load stayed lenient) and never edited since:
        // nothing to write, and nothing lost by skipping it.
        let Some(data) = store.get(&data_path) else {
            continue;
        };
        let bytes = sidecar::save(data).map_err(|err| {
            BevyError::from(format!(
                "terrain sidecar {data_path:?} cannot be written: {err}"
            ))
        })?;
        writes.push((path, bytes));
    }

    for (path, bytes) in writes {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                BevyError::from(format!(
                    "failed to create terrain data directory {} for {}: {err}",
                    parent.display(),
                    path.display()
                ))
            })?;
        }
        write_atomic(&path, &bytes).map_err(|err| {
            BevyError::from(format!(
                "failed to write terrain data {}: {err}",
                path.display()
            ))
        })?;
        info!("Terrain data written to {}", path.display());
    }
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

/// The runtime inline assets referenced by a BSN document's kept components,
/// collected at emit time so they can be embedded and resolved.
struct BsnInlineAssetPass {
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
    let skip_ids = structural_skip_type_ids();

    let mut refs: Vec<jackdaw_bsn::CatalogAssetRef> = Vec::new();
    let mut touched: Vec<(Entity, String)> = Vec::new();
    let mut counters: HashMap<String, usize> = HashMap::new();

    for &entity in entities {
        let Ok(entity_ref) = world.get_entity(entity) else {
            continue;
        };
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
        // Generic asset type paths cannot round-trip through the parser, so the
        // embed would be dropped; leave the handle unresolved as before.
        if asset_type_path.contains('<') {
            return true;
        }

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
    let Some(live) = world.get_resource::<jackdaw_bsn::SceneBsnAst>() else {
        return String::new();
    };
    let mut ast = live.deep_clone();

    let registry = world.resource::<AppTypeRegistry>().clone();

    // Seed the reference map with assets that already resolve: catalog entries
    // and scene-inline entries already embedded as document roots.
    //
    // An unsaved material is left out: nothing on disk defines it, so seeding it would emit
    // an `@Name` that resolves nowhere outside this editor run. Unseeded, it embeds inline
    // and the scene stands alone.
    let mut seed: bevy::platform::collections::HashMap<UntypedAssetId, String> =
        bevy::platform::collections::HashMap::default();
    let ephemeral = crate::material_assets::ephemeral_material_ids(world);
    if let Some(catalog) = world.get_resource::<crate::asset_catalog::AssetCatalog>() {
        for (id, name) in &catalog.id_to_name {
            if ephemeral.contains(id) {
                continue;
            }
            seed.entry(*id).or_insert_with(|| name.clone());
        }
    }
    if let Some(scene_assets) = world.get_resource::<jackdaw_bsn::BsnSceneAssets>() {
        for (ref_name, handle) in &scene_assets.0 {
            if ref_name.starts_with('#') {
                seed.insert(handle.id(), ref_name.clone());
            }
        }
    }

    let entities = doc_entities_in_order(&ast);
    let pass = {
        let reg = registry.read();
        collect_bsn_inline_assets(world, &reg, &entities, seed)
    };

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

    // No kept component references an asset handle: the document already emits
    // faithfully once sparsified.
    if pass.touched.is_empty() {
        return jackdaw_bsn::emit_scene(&ast);
    }

    if !pass.refs.is_empty() {
        jackdaw_bsn::append_assets_to_ast(&mut ast, world, &pass.refs);
    }

    // Re-derive each handle-bearing component patch with the asset context so
    // its handle fields emit reference names or asset paths.
    rederive_handle_patches(world, &mut ast, &registry, parent_path, &pass);

    jackdaw_bsn::emit_scene(&ast)
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
        let new_patch = jackdaw_bsn::component_to_bsn_patch_with_assets(
            component.as_partial_reflect(),
            &reg,
            &ctx,
        );
        ast.set_patch(patch_entity, new_patch);
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

    // Seed catalog and live scene-inline names so same-scene copy keeps the existing `#Name`
    // refs. Cross-scene paste of unknown inline assets is unsupported; those handles would
    // need embedding and merge. Unsaved materials are left unseeded so the clip carries
    // them inline.
    let mut seed: bevy::platform::collections::HashMap<UntypedAssetId, String> =
        bevy::platform::collections::HashMap::default();
    let ephemeral = crate::material_assets::ephemeral_material_ids(world);
    if let Some(catalog) = world.get_resource::<crate::asset_catalog::AssetCatalog>() {
        for (id, name) in &catalog.id_to_name {
            if ephemeral.contains(id) {
                continue;
            }
            seed.entry(*id).or_insert_with(|| name.clone());
        }
    }
    if let Some(scene_assets) = world.get_resource::<jackdaw_bsn::BsnSceneAssets>() {
        for (ref_name, handle) in &scene_assets.0 {
            if ref_name.starts_with('#') {
                seed.insert(handle.id(), ref_name.clone());
            }
        }
    }

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

        retarget_active_scene(&mut world, "/tmp/zones/harbour.bsn");

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

        let data_path = "zone.terrain-0.jdterrain";
        let mut store = crate::terrain::TerrainDataStore::default();
        store.insert(
            data_path.to_string(),
            jackdaw_terrain::RegionTerrainData::from_legacy_v1(&jackdaw_terrain::TerrainData {
                resolution: 2,
                heights: vec![0.0; 4],
                channels: vec![],
            })
            .expect("a power-of-two resolution migrates"),
        );
        world.insert_resource(store);
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 2,
            data_path: data_path.to_string(),
            ..default()
        });

        let baseline = world.resource::<SceneDirtyState>().undo_len_at_save;
        let error = save_scene_inner(&mut world).expect_err("the blocked sidecar fails the save");

        assert!(error.to_string().contains(data_path));
        assert!(world.resource::<crate::scenes::Scenes>().tabs[0].dirty);
        assert_eq!(
            world.resource::<SceneDirtyState>().undo_len_at_save,
            baseline,
            "failed saves must not advance the clean-history baseline",
        );
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

/// Tests for the terrain sidecar sibling writer.
///
/// These drive the synchronous save boundary directly: success means the
/// bytes are already durable enough to read back, and failure is returned.
#[cfg(test)]
mod terrain_sidecar_tests {
    use std::path::PathBuf;

    use bevy::prelude::*;
    use jackdaw_terrain::{RegionTerrainData, TerrainData, sidecar};

    use super::{emit_bsn_scene_with_inline_assets, export_terrain_sidecars};
    use crate::terrain::TerrainDataStore;

    fn unique_tmp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jd_terr_{}_{label}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// A sculpted 4x4 terrain, distinctive enough that a zeroed or
    /// truncated round-trip fails loudly.
    fn sculpted() -> TerrainData {
        TerrainData {
            resolution: 4,
            heights: (0..16).map(|i| i as f32 * 0.5).collect(),
            channels: vec![],
        }
    }

    /// A store holding ground for `terrain`, in regions sized to keep the fixture small.
    /// Nothing allocates implicitly, so a terrain a test writes to is laid down first.
    fn store_holding(terrain: &jackdaw_scene_types::Terrain) -> TerrainDataStore {
        let mut regions = jackdaw_terrain::TerrainRegions::new(
            jackdaw_terrain::RegionSize::new(terrain.resolution.next_power_of_two())
                .expect("a power of two"),
        );
        regions
            .ensure_grid(terrain.resolution)
            .expect("inside the region cap");
        let mut store = TerrainDataStore::default();
        store.insert(
            terrain.data_path.clone(),
            jackdaw_terrain::RegionTerrainData {
                regions,
                ..Default::default()
            },
        );
        store
    }

    fn document(data: &TerrainData) -> RegionTerrainData {
        let mut document =
            RegionTerrainData::from_legacy_v1(data).expect("a power-of-two resolution migrates");
        // The load path settles a document onto the geometry its cells are drawn at before
        // a save, so a stand-in for a document the editor holds states its own.
        document.grid = Some(jackdaw_terrain::sidecar::GridGeometry::DEFAULT);
        document
    }

    fn world_with_terrain(data_path: &str, data: TerrainData) -> World {
        let mut world = World::new();
        let mut store = TerrainDataStore::default();
        store.insert(data_path.to_string(), document(&data));
        world.insert_resource(store);
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: data_path.to_string(),
            ..default()
        });
        world
    }

    fn read_decode(path: &std::path::Path) -> Option<RegionTerrainData> {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| sidecar::load(&bytes).ok())
    }

    #[test]
    fn writes_a_sidecar_that_decodes_back_to_the_sculpted_heights() {
        let tmp = unique_tmp_dir("roundtrip");
        let scene_path = tmp.join("zone.bsn");
        let sidecar_path = tmp.join("zone.terrain-0.jdterrain");

        let mut world = world_with_terrain("zone.terrain-0.jdterrain", sculpted());
        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("sidecar write succeeds");

        let decoded = read_decode(&sidecar_path)
            .unwrap_or_else(|| panic!("sidecar not written: {}", sidecar_path.display()));
        assert_eq!(decoded, document(&sculpted()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Saving twice with no edits in between must produce the same bytes,
    /// so a sidecar can be committed and diffed like any other artifact.
    #[test]
    fn saving_twice_produces_identical_bytes() {
        let tmp = unique_tmp_dir("stable");
        let scene_path = tmp.join("zone.bsn");
        let sidecar_path = tmp.join("zone.terrain-0.jdterrain");

        let mut world = world_with_terrain("zone.terrain-0.jdterrain", sculpted());
        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("first write succeeds");
        let first = std::fs::read(&sidecar_path).expect("read first write");

        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("second write succeeds");
        let second = std::fs::read(&sidecar_path).expect("read second write");

        assert_eq!(first, second);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_sidecar_write_failure_is_returned_to_the_save_caller() {
        let tmp = unique_tmp_dir("write-failure");
        let blocked_parent = tmp.join("not-a-directory");
        std::fs::write(&blocked_parent, b"file blocks directory creation")
            .expect("create blocking file");
        let scene_path = blocked_parent.join("zone.bsn");
        let sidecar_path = blocked_parent.join("zone.terrain-0.jdterrain");

        let mut world = world_with_terrain("zone.terrain-0.jdterrain", sculpted());
        let error = export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect_err("the write failure must reach the save boundary");

        assert!(
            error
                .to_string()
                .contains(&sidecar_path.display().to_string()),
            "the error names the failed sidecar: {error}",
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A scene with no terrain must not litter the project with an empty
    /// sidecar, exactly as the navmesh writer no-ops when nothing is baked.
    #[test]
    fn no_sidecar_when_the_scene_has_no_terrain() {
        let tmp = unique_tmp_dir("bare");
        let scene_path = tmp.join("bare.bsn");

        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("terrain-less scene is a no-op");
        let stray = std::fs::read_dir(&tmp)
            .expect("temp dir readable")
            .filter_map(Result::ok)
            .any(|e| {
                e.path()
                    .extension()
                    .is_some_and(|ext| ext == sidecar::EXTENSION)
            });
        assert!(!stray, "no sidecar may be written for a terrain-less scene");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Terrain heights live in the sidecar, so a 512-resolution terrain contributes a
    /// handful of lines to the scene text rather than 262,144 floats.
    #[test]
    fn a_512_terrain_contributes_almost_nothing_to_the_scene_text() {
        use bevy::ecs::reflect::AppTypeRegistry;
        use jackdaw_bsn::SceneBsnAst;

        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let mut writer = registry.write();
            writer.register::<Name>();
            writer.register::<jackdaw_scene_types::Terrain>();
            writer.register::<jackdaw_scene_types::SceneNodeId>();
        }
        world.init_resource::<SceneBsnAst>();

        let mut store = TerrainDataStore::default();
        store.insert(
            "zone.terrain-0.jdterrain".to_string(),
            document(&TerrainData {
                resolution: 512,
                heights: vec![0.75; 512 * 512],
                channels: vec![],
            }),
        );
        world.insert_resource(store);

        let entity = world
            .spawn((
                Name::new("ground"),
                jackdaw_scene_types::Terrain {
                    resolution: 512,
                    data_path: "zone.terrain-0.jdterrain".to_string(),
                    ..default()
                },
            ))
            .id();
        crate::scene_io::register_entity_in_ast(&mut world, entity);

        let text = emit_bsn_scene_with_inline_assets(&mut world, std::path::Path::new("."));
        assert!(
            text.contains("zone.terrain-0.jdterrain"),
            "the scene must name the sidecar it depends on:\n{text}"
        );
        assert!(
            text.len() < 2048,
            "a 512 terrain must not bloat the scene text; got {} bytes:\n{text}",
            text.len()
        );
    }

    /// Store entries whose terrain is not in this scene belong to another
    /// tab (or to an undone delete) and must not be written beside it.
    #[test]
    fn only_terrains_present_in_the_scene_are_written() {
        let tmp = unique_tmp_dir("scoped");
        let scene_path = tmp.join("zone.bsn");

        let mut world = world_with_terrain("zone.terrain-0.jdterrain", sculpted());
        world.resource_mut::<TerrainDataStore>().insert(
            "other-scene.terrain-0.jdterrain".to_string(),
            document(&sculpted()),
        );
        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("this scene's sidecar writes");

        assert!(
            read_decode(&tmp.join("zone.terrain-0.jdterrain")).is_some(),
            "this scene's terrain is written"
        );
        assert!(
            !tmp.join("other-scene.terrain-0.jdterrain").exists(),
            "another scene's terrain must not be written here"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A sidecar written by a newer build (`SidecarError::UnsupportedVersion`) cannot be
    /// read here. The user paints a stroke anyway and saves; the save must not write a
    /// zeroed document over the real file.
    #[test]
    fn an_unreadable_sidecar_is_never_overwritten_by_a_save() {
        let tmp = unique_tmp_dir("unreadable");
        let scene_path = tmp.join("zone.bsn");
        let sidecar_path = tmp.join("zone.terrain-0.jdterrain");

        let mut original = sidecar::save(&document(&sculpted())).expect("encodes");
        original[8..10].copy_from_slice(&(sidecar::VERSION_5 + 1).to_le_bytes());
        std::fs::write(&sidecar_path, &original).expect("write sidecar");

        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        });

        crate::scene_io::import_terrain_sidecars(
            &mut world,
            &scene_path.to_string_lossy(),
            crate::scene_io::SidecarImport::Reload,
        );
        assert!(
            world
                .resource::<TerrainDataStore>()
                .is_load_failed("zone.terrain-0.jdterrain"),
            "a decode failure must mark the entry load-failed",
        );

        // The stroke: an edit attempt must be refused, not minted as
        // zeroed data.
        let brushed = jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        };
        assert!(
            world
                .resource_mut::<TerrainDataStore>()
                .entry_for(&brushed)
                .is_none(),
            "edits to a load-failed terrain must be refused",
        );

        // Ctrl+S: the save must succeed and must not touch the file.
        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("save must not hard-error on a load-failed entry");
        assert_eq!(
            std::fs::read(&sidecar_path).expect("sidecar still on disk"),
            original,
            "the unreadable original must survive the save byte-for-byte",
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The texture-set reference and the control word under every cell are authored through
    /// the store, written by a save, and come back from the file identical.
    #[test]
    fn authored_paint_and_materials_survive_a_save_and_reload() {
        use jackdaw_terrain::Control;

        let tmp = unique_tmp_dir("paint-roundtrip");
        let scene_path = tmp.join("zone.bsn");
        let data_path = "zone.terrain-0.jdterrain";
        let terrain = jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: data_path.to_string(),
            ..default()
        };
        let painted = Control::default()
            .with_base_id(3)
            .with_overlay_id(7)
            .with_blend(200);

        let mut world = World::new();
        world.insert_resource(store_holding(&terrain));
        world.spawn(terrain.clone());
        let authored = {
            let mut store = world.resource_mut::<TerrainDataStore>();
            store
                .entry_for(&terrain)
                .expect("keyed")
                .set_heights(&sculpted().heights);
            store.control_mut(&terrain).expect("keyed")[5] = painted;
            store
                .set_materials(
                    data_path,
                    vec![
                        jackdaw_terrain::sidecar::TerrainMaterialSlot::new("grass"),
                        jackdaw_terrain::sidecar::TerrainMaterialSlot {
                            material: "rock_05".to_string(),
                            uv_scale: 0.25,
                            detile: 0.5,
                        },
                    ],
                )
                .expect("plain material names are accepted");
            store.get(data_path).expect("authored").clone()
        };

        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy()).expect("save succeeds");

        // A fresh store, as a reopened editor has.
        let mut reopened = World::new();
        reopened.insert_resource(TerrainDataStore::default());
        reopened.spawn(terrain.clone());
        crate::scene_io::import_terrain_sidecars(
            &mut reopened,
            &scene_path.to_string_lossy(),
            crate::scene_io::SidecarImport::Reload,
        );

        let store = reopened.resource::<TerrainDataStore>();
        assert_eq!(
            store.get(data_path),
            Some(&authored),
            "the reloaded document must equal what was authored",
        );
        assert_eq!(store.control(data_path)[5], painted);
        assert_eq!(store.materials(data_path).len(), 2);
        assert_eq!(store.materials(data_path)[1].material, "rock_05");
        assert_eq!(store.materials(data_path)[1].uv_scale, 0.25);
        assert_eq!(store.materials(data_path)[1].detile, 0.5);
        assert_eq!(store.heights(data_path), sculpted().heights.as_slice());
        assert!(
            store.take_control_dirty(data_path).is_some(),
            "loaded paint must be marked for upload, or it never reaches the material",
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 129 vertices per edge is 2^7 + 1, so no single power-of-two region holds it. It has
    /// to open, be editable, and round-trip every height, seam row included.
    #[test]
    fn a_non_power_of_two_sidecar_opens_and_embeds_every_height() {
        let tmp = unique_tmp_dir("non-pow2");
        let scene_path = tmp.join("zone.bsn");
        let sidecar_path = tmp.join("zone.terrain-0.jdterrain");

        let heights: Vec<f32> = (0..129 * 129).map(|i| i as f32 * 0.5).collect();
        let original = sidecar::encode(&TerrainData {
            resolution: 129,
            heights: heights.clone(),
            channels: vec![],
        })
        .expect("v1 encodes");
        std::fs::write(&sidecar_path, &original).expect("write sidecar");

        let odd = jackdaw_scene_types::Terrain {
            resolution: 129,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        };
        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(odd.clone());

        crate::scene_io::import_terrain_sidecars(
            &mut world,
            &scene_path.to_string_lossy(),
            crate::scene_io::SidecarImport::Reload,
        );
        let store = world.resource::<TerrainDataStore>();
        assert!(
            !store.is_load_failed("zone.terrain-0.jdterrain"),
            "a 2^k + 1 vertex grid is storable and must not be quarantined",
        );
        // A 129-vertex grid lands in the 2x2 block of 128-cell regions that holds it, so the
        // terrain is 256 cells across with the authored 129 embedded at its corner. Each
        // authored height stays on the cell it described; the rest is ground the regions
        // brought with them.
        let document = store.get("zone.terrain-0.jdterrain").expect("loaded");
        assert_eq!(document.grid_resolution(), 256);
        for (at, want) in heights.iter().enumerate() {
            let (x, z) = ((at % 129) as i32, (at / 129) as i32);
            assert_eq!(document.regions.height_at(x, z), *want);
        }
        assert!(
            world
                .resource_mut::<TerrainDataStore>()
                .entry_for(&odd)
                .is_some(),
            "edits to it must be accepted",
        );

        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy()).expect("save succeeds");
        let reloaded = read_decode(&sidecar_path).expect("sidecar rewritten");
        for (at, want) in heights.iter().enumerate() {
            let (x, z) = ((at % 129) as i32, (at / 129) as i32);
            assert_eq!(
                reloaded.regions.height_at(x, z),
                *want,
                "the rewrite must keep every height, seam row included",
            );
        }
        assert_eq!(reloaded.regions.region_size().get(), 128);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A pre-region sidecar opens, migrates, and is rewritten in the current format with no
    /// user action.
    #[test]
    fn a_pre_region_sidecar_migrates_on_load_and_saves_in_the_current_format() {
        let tmp = unique_tmp_dir("migrate");
        let scene_path = tmp.join("zone.bsn");
        let sidecar_path = tmp.join("zone.terrain-0.jdterrain");
        std::fs::write(
            &sidecar_path,
            sidecar::encode(&sculpted()).expect("v1 encodes"),
        )
        .expect("write sidecar");

        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        });
        crate::scene_io::import_terrain_sidecars(
            &mut world,
            &scene_path.to_string_lossy(),
            crate::scene_io::SidecarImport::Reload,
        );
        assert_eq!(
            world
                .resource::<TerrainDataStore>()
                .heights("zone.terrain-0.jdterrain"),
            sculpted().heights.as_slice(),
        );

        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy()).expect("save succeeds");
        let rewritten = std::fs::read(&sidecar_path).expect("read back");
        assert_eq!(
            u16::from_le_bytes([rewritten[8], rewritten[9]]),
            sidecar::VERSION_5,
        );
        // The load settled this terrain onto the geometry its declared rectangle drew with
        // (four vertices across the default 100 metres, cornered at -size/2) and the rewrite
        // records it, so reading the file back needs no rectangle.
        let mut migrated = document(&sculpted());
        migrated.grid = Some(sidecar::GridGeometry {
            cell_size: 100.0 / 3.0,
            anchor: Vec2::splat(-50.0),
        });
        assert_eq!(
            read_decode(&sidecar_path),
            Some(migrated),
            "the migrated document must survive the rewrite",
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A terrain the editor touched and then flattened is a region on disk, not an empty
    /// document that would reopen as no terrain at all.
    #[test]
    fn a_flat_but_authored_terrain_round_trips_as_a_present_region() {
        let tmp = unique_tmp_dir("flat-authored");
        let scene_path = tmp.join("zone.bsn");
        let data_path = "zone.terrain-0.jdterrain";
        let terrain = jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: data_path.to_string(),
            ..default()
        };

        let mut world = World::new();
        world.insert_resource(store_holding(&terrain));
        world.spawn(terrain.clone());
        {
            let mut store = world.resource_mut::<TerrainDataStore>();
            let mut entry = store.entry_for(&terrain).expect("keyed");
            entry.heights_mut()[0] = 5.0;
            entry.set_heights(&[0.0; 16]);
        }
        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy()).expect("save succeeds");

        let reloaded = read_decode(&tmp.join(data_path)).expect("sidecar written");
        assert_eq!(reloaded.regions.region_count(), 1);
        assert!(reloaded.contiguous_grid().is_some());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// C1 pinning test for the twin bug: a scene whose sidecar was never
    /// copied alongside it (missing, not corrupt) loads flat and must
    /// stay saveable indefinitely, not hard-error on every save attempt.
    #[test]
    fn a_never_loaded_missing_sidecar_stays_saveable() {
        let tmp = unique_tmp_dir("never-loaded");
        let scene_path = tmp.join("zone.bsn");
        let sidecar_path = tmp.join("zone.terrain-0.jdterrain");

        let mut world = World::new();
        world.insert_resource(TerrainDataStore::default());
        world.spawn(jackdaw_scene_types::Terrain {
            resolution: 4,
            data_path: "zone.terrain-0.jdterrain".to_string(),
            ..default()
        });

        crate::scene_io::import_terrain_sidecars(
            &mut world,
            &scene_path.to_string_lossy(),
            crate::scene_io::SidecarImport::Reload,
        );
        assert!(
            !world
                .resource::<TerrainDataStore>()
                .contains("zone.terrain-0.jdterrain")
        );
        assert!(
            !world
                .resource::<TerrainDataStore>()
                .is_load_failed("zone.terrain-0.jdterrain")
        );

        export_terrain_sidecars(&mut world, &scene_path.to_string_lossy())
            .expect("a scene with a never-loaded terrain must remain saveable");
        assert!(
            !sidecar_path.exists(),
            "nothing should be written for data that never existed"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
