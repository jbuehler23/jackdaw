use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::asset::{ReflectAsset, ReflectHandle, UntypedAssetId};
use bevy::reflect::TypeRegistry;
use bevy::{
    ecs::reflect::AppTypeRegistry,
    prelude::*,
    tasks::{AsyncComputeTaskPool, IoTaskPool},
};
use rfd::AsyncFileDialog;

use super::load::SceneDialogTask;
use super::{
    SceneDirtyState, SceneFilePath, get_window_handle, should_skip_component,
    structural_skip_type_ids,
};

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

pub fn save_scene(world: &mut World) {
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
        return;
    }

    if let Err(err) = save_scene_inner(world) {
        error!("scene save failed: {err}");
    }
}

pub fn save_scene_as(world: &mut World) {
    if world.contains_resource::<SceneDialogTask>() {
        return; // Dialog already open
    }
    spawn_save_dialog(world);
}

pub(super) fn save_scene_inner(world: &mut World) -> Result<(), BevyError> {
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
        if let Err(err) = crate::prefab::operators::save_prefab_to_disk(world, &path) {
            warn!("scene.save: prefab save failed: {err}");
        }
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
    if legacy_backup.is_some() {
        scene_path.path = Some(path.clone());
    }

    // Mark scene as clean
    let history_len = world
        .resource::<jackdaw_commands::CommandHistory>()
        .undo_stack
        .len();
    world.resource_mut::<SceneDirtyState>().undo_len_at_save = history_len;

    // Clear the active scene tab's dirty flag, resync its history depth
    // marker so `mark_active_dirty_on_history_growth` does not immediately
    // re-dirty the tab on the next frame, and retarget a redirected tab at
    // its new `.bsn` path.
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

    // Write to disk on the IO task pool. A redirected save renames the
    // legacy source to `.jsn.bak` first so a stale `.jsn` cannot shadow the
    // fresh `.bsn` on the next open.
    let path_clone = path.clone();
    IoTaskPool::get()
        .spawn(async move {
            if let Some(old_path) = legacy_backup {
                let backup = format!("{old_path}.bak");
                if let Err(err) = std::fs::rename(&old_path, &backup) {
                    warn!("Could not back up legacy scene {old_path} to {backup}: {err}");
                }
            }
            match std::fs::write(&path_clone, &contents) {
                Ok(()) => info!("Scene saved to {path_clone}"),
                Err(err) => warn!("Failed to write scene file: {err}"),
            }
        })
        .detach();

    // Also persist the in-memory baked navmesh (if any) to a sibling
    // `<scene>.nav` file so it survives reload. No-op when nothing is baked.
    export_navmesh_sibling(world, &path);

    // Save catalog alongside scene if dirty
    crate::asset_catalog::save_catalog(world);

    // Persist current editor layout to project.jsn
    save_layout_to_project(world);

    Ok(())
}

/// Export the in-memory baked navmesh to a sibling `<scene>.nav` file,
/// reusing the same bincode serialization as the manual `navmesh.save`
/// operator (and the same contract `navmesh.load` / a headless server
/// reads back). This is a clean no-op when no navmesh is baked (the common
/// case): it never errors the scene save and never writes an empty file.
///
/// Known limitation: a previously-exported `.nav` is never deleted here. If
/// a scene once had a navmesh (so `.nav` exists) and the navmesh is later
/// removed, the stale `.nav` remains on disk. This is intentional for now -
/// deletion risks clobbering a file another tool or the user owns, and a
/// stale `.nav` is better caught by a freshness check on the load side.
fn export_navmesh_sibling(world: &World, scene_path: &str) {
    use bevy_rerecast::Navmesh;

    use crate::navmesh::NavmeshHandleRes;

    let Some(handle) = world.get_resource::<NavmeshHandleRes>() else {
        return;
    };
    let Some(assets) = world.get_resource::<Assets<Navmesh>>() else {
        return;
    };
    let Some(navmesh) = assets.get(&handle.0) else {
        return;
    };

    let nav_path = PathBuf::from(scene_path).with_extension("nav");
    let navmesh = navmesh.clone();
    IoTaskPool::get()
        .spawn(async move {
            let result = (|| -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
                if let Some(parent) = nav_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let mut file = std::fs::File::create(&nav_path)?;
                let config = bincode::config::standard();
                bincode::serde::encode_into_std_write(&navmesh, &mut file, config)?;
                Ok(())
            })();
            match result {
                Ok(()) => info!("Navmesh exported to {}", nav_path.display()),
                Err(err) => warn!("Failed to export navmesh: {err}"),
            }
        })
        .detach();
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
    let mut seed: bevy::platform::collections::HashMap<UntypedAssetId, String> =
        bevy::platform::collections::HashMap::default();
    if let Some(catalog) = world.get_resource::<crate::asset_catalog::AssetCatalog>() {
        for (id, name) in &catalog.id_to_name {
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
/// their subtrees) as self-contained BSN text for the clipboard.
///
/// Referenced runtime inline assets embed as `#Name` roots so a cross-scene
/// paste carries them along; only catalog `@Name` assets are assumed to
/// resolve at the destination. Stable node ids are stripped from the emitted
/// text, so a paste mints fresh ones. Works on a deep clone of the document,
/// so emission never mutates the live state.
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

    // Seed only catalog names: scene-inline assets referenced by the copied
    // components must embed into the clipboard text to survive a paste into
    // another scene.
    let mut seed: bevy::platform::collections::HashMap<UntypedAssetId, String> =
        bevy::platform::collections::HashMap::default();
    if let Some(catalog) = world.get_resource::<crate::asset_catalog::AssetCatalog>() {
        for (id, name) in &catalog.id_to_name {
            seed.entry(*id).or_insert_with(|| name.clone());
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

    // Strip stable node ids from the copied subtrees; a paste mints fresh
    // ones, so the clipboard must not carry the source ids.
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

    /// In Live view the preview world carries streamed game values, so a save    /// In Live view the preview world carries streamed game values, so a save
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
}

/// Tests for the navmesh auto-export-on-save sibling writer.
///
/// `export_navmesh_sibling` dispatches the actual file write onto
/// `IoTaskPool` and `.detach()`es it, so "assert the file exists right
/// after calling" is inherently racy. This build enables the
/// `multi_threaded` task-pool feature, so detached IO-pool tasks run on
/// dedicated OS threads and make progress without any main-thread tick.
/// We therefore exercise the *real* production code path (Option 1 from
/// the plan) and poll the filesystem with a bounded timeout for the
/// sibling `.nav` to appear. The no-navmesh case early-returns before
/// any task is spawned, so it is deterministic without polling.
#[cfg(test)]
mod navmesh_export_tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use bevy::prelude::*;
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    use bevy_rerecast::Navmesh;
    use bevy_rerecast::rerecast::{DetailNavmesh, PolygonNavmesh};

    use super::export_navmesh_sibling;
    use crate::navmesh::NavmeshHandleRes;

    /// Build a minimal, valid `Navmesh`. `Navmesh` itself does not derive
    /// `Default`, but each of its three fields does (`PolygonNavmesh` and
    /// `DetailNavmesh` derive it; `NavmeshSettings` has a hand-written
    /// `impl Default`), so we assemble it field-by-field.
    fn empty_navmesh() -> Navmesh {
        Navmesh {
            polygon: PolygonNavmesh::default(),
            detail: DetailNavmesh::default(),
            settings: bevy_rerecast::NavmeshSettings::default(),
        }
    }

    /// A unique temp directory for one test, namespaced by PID + a label
    /// so concurrent test runs (and the two tests here) never collide.
    fn unique_tmp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jd_nav_{}_{label}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// A minimal App with just enough to host `Assets<Navmesh>`:
    /// `MinimalPlugins` provides the task pools that `AssetPlugin`
    /// requires, `AssetPlugin` provides the asset infrastructure, and
    /// `init_asset::<Navmesh>` registers the store.
    fn minimal_asset_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default())
            .init_asset::<Navmesh>();
        app
    }

    #[test]
    fn exports_decodable_nav_when_baked() {
        // The production write runs on IoTaskPool; make sure it exists.
        // `get_or_init` is idempotent, so this is safe even if another
        // test/plugin already initialized it.
        IoTaskPool::get_or_init(|| TaskPoolBuilder::new().build());

        let tmp = unique_tmp_dir("baked");
        let scene_path = tmp.join("zone.jsn");
        let nav_path = tmp.join("zone.nav");

        let mut app = minimal_asset_app();

        // Insert a baked navmesh and point NavmeshHandleRes at it.
        let handle = app
            .world_mut()
            .resource_mut::<Assets<Navmesh>>()
            .add(empty_navmesh());
        app.world_mut().insert_resource(NavmeshHandleRes(handle));

        // Call the real production helper: it spawns a detached IoTaskPool
        // write of the sibling `.nav`.
        export_navmesh_sibling(app.world(), &scene_path.to_string_lossy());

        // Poll for the file to appear (bounded ~3s). The IO pool runs on
        // its own threads, so no app tick is needed; we still tick the
        // global pools each iteration as a belt-and-suspenders nudge in
        // case the runner constrained the pool to the calling thread.
        // Poll for the sibling `.nav` to both exist AND fully decode (bounded
        // ~3s). The IO pool runs on its own threads and creates the file before
        // the bytes are flushed, so `exists()` alone races the write and can
        // read a truncated file; a successful decode is the real readiness
        // signal. The bytes must decode back into a Navmesh via the same bincode
        // contract the loader uses (`navmesh.load`). We still tick the global
        // pools each iteration as a belt-and-suspenders nudge in case the runner
        // constrained the pool to the calling thread.
        let config = bincode::config::standard();
        let mut decoded: Option<Navmesh> = None;
        for _ in 0..300 {
            if nav_path.exists()
                && let Ok(mut file) = std::fs::File::open(&nav_path)
                && let Ok(nav) = bincode::serde::decode_from_std_read(&mut file, config)
            {
                decoded = Some(nav);
                break;
            }
            bevy::tasks::tick_global_task_pools_on_main_thread();
            std::thread::sleep(Duration::from_millis(10));
        }
        let decoded = decoded.unwrap_or_else(|| {
            panic!(
                "sibling .nav was not written and decodable within the timeout: {}",
                nav_path.display()
            )
        });
        assert_eq!(
            decoded,
            empty_navmesh(),
            "round-tripped navmesh must equal the baked input"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn no_nav_when_not_baked() {
        let tmp = unique_tmp_dir("empty");
        let scene_path = tmp.join("empty.jsn");
        let nav_path = tmp.join("empty.nav");

        // Asset store exists, but NO NavmeshHandleRes resource is inserted,
        // so the helper early-returns before spawning any task. This path
        // is deterministic and needs no polling.
        let app = minimal_asset_app();

        export_navmesh_sibling(app.world(), &scene_path.to_string_lossy());

        // Give any (erroneously) spawned async write a chance to land, so
        // a regression that drops the guard would actually be caught.
        for _ in 0..20 {
            bevy::tasks::tick_global_task_pools_on_main_thread();
            std::thread::sleep(Duration::from_millis(5));
        }

        assert!(
            !nav_path.exists(),
            "no .nav must be written when nothing is baked: {}",
            nav_path.display()
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
