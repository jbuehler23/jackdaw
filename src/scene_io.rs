use std::any::TypeId;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Formatter};
use std::path::{Path, PathBuf};
use std::result::Result;

use bevy::asset::{ReflectAsset, ReflectHandle, UntypedAssetId};
use bevy::image::ImageLoaderSettings;
use bevy::reflect::serde::{ReflectDeserializerProcessor, ReflectSerializerProcessor};
use bevy::reflect::{TypeInfo, TypeRegistration, TypeRegistry};
use bevy::{
    asset::AssetPath,
    ecs::reflect::AppTypeRegistry,
    prelude::*,
    reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer},
    tasks::{AsyncComputeTaskPool, IoTaskPool, Task, futures_lite::future},
    transform::components::TransformTreeChanged,
    window::{PrimaryWindow, RawHandleWrapper},
};
use jackdaw_jsn::format::{JsnAssets, JsnEntity, JsnMetadata};
use rfd::{AsyncFileDialog, FileHandle};
use serde::de::{DeserializeSeed, Visitor};
use serde::{Deserializer, Serializer};

use crate::EditorEntity;

/// Component type path prefixes that should never be saved (runtime-only / internal).
const SKIP_COMPONENT_PREFIXES: &[&str] = &[
    "bevy_render::",
    "bevy_picking::",
    "bevy_window::",
    "bevy_ecs::observer::",
    "bevy_camera::primitives::",
    "bevy_camera::visibility::",
    // AnimationPlayer / AnimationGraphHandle / AnimationTargetId / AnimatedBy
    // are installed on targets at runtime by the animation plugin.
    // They're derived from the authored clip components and must not be
    // serialized; otherwise load would restore stale player state and
    // dangling asset handles.
    "bevy_animation::",
];

/// Specific component type paths that should never be saved.
const SKIP_COMPONENT_PATHS: &[&str] = &[
    "bevy_transform::components::transform::TransformTreeChanged",
    "bevy_light::cascade::Cascades",
    // Runtime activation state, granted and revoked by the rig systems (the
    // multiplayer gate on clients). Persisting it plants a rig that fights
    // those systems on every load.
    "jackdaw_camera_rig::ActiveCameraRig",
    // Render-state handles are always derived in the editor (brush chunks,
    // terrain chunks, GLTF instances, reference-image quads) and rebuilt
    // from the authored components on load; serializing them would inline
    // runtime mesh/material assets into the scene.
    "bevy_mesh::components::Mesh3d",
    "bevy_pbr::mesh_material::MeshMaterial3d<bevy_pbr::pbr_material::StandardMaterial>",
];

/// Paths that override the skip prefixes  -- these are always saved even if
/// they match a skip prefix.
const ALWAYS_SAVE_PATHS: &[&str] = &[
    "bevy_camera::visibility::Visibility",
    // Overrides the `jackdaw::` skip so `apply_ast_to_world` can
    // match selected brushes by stable id across an undo.
    "jackdaw::draw_brush::BrushStableId",
    // The stable node id must persist so a running game can map a live
    // entity back to its authored node. It is written as the structural
    // `JsnEntity::id` field rather than a component entry, but this keeps
    // any other save path from stripping it.
    jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH,
    // Prefab marker components must round-trip through save and AST
    // registration; stripping them breaks instance inheritance and
    // causes `revert_component` to lose track of the prefab source.
    "jackdaw::prefab::components::Prefab",
    "jackdaw::prefab::components::IsA",
    "jackdaw::prefab::components::PrefabEntityId",
    // Reference image boards persist with the scene; the quad mesh and
    // material are derived from this component at runtime.
    "jackdaw::reference_image::ReferenceImage",
];

pub fn should_skip_component(type_path: &str) -> bool {
    // Always-save takes priority over any skip rule
    if ALWAYS_SAVE_PATHS.contains(&type_path) {
        return false;
    }
    if type_path.starts_with("jackdaw::") {
        return true;
    }
    for prefix in SKIP_COMPONENT_PREFIXES {
        if type_path.starts_with(prefix) {
            return true;
        }
    }
    SKIP_COMPONENT_PATHS.contains(&type_path)
}

pub struct SceneIoPlugin;

impl Plugin for SceneIoPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SceneFilePath>()
            .init_resource::<SceneDirtyState>()
            .add_systems(
                Update,
                (poll_scene_dialog, cleanup_pending_new_scene)
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(on_new_scene_save)
            .add_observer(on_new_scene_discard);
    }
}

/// Tracks whether the scene has unsaved changes by comparing the current
/// undo stack length against the length at the time of last save/load/new.
#[derive(Resource, Default)]
pub struct SceneDirtyState {
    pub undo_len_at_save: usize,
}

/// Returns `true` when the scene has unsaved changes.
pub fn is_scene_dirty(world: &World) -> bool {
    let history = world.resource::<jackdaw_commands::CommandHistory>();
    let dirty_state = world.resource::<SceneDirtyState>();
    history.undo_stack.len() != dirty_state.undo_len_at_save
}

/// Marker resource: a "save before new scene?" dialog is currently open.
#[derive(Resource)]
struct PendingNewScene;

#[derive(Resource)]
enum SceneDialogTask {
    Save(Task<Option<FileHandle>>),
    Load(Task<Option<FileHandle>>),
}

/// Stores the currently active scene file path and metadata.
#[derive(Resource, Default)]
pub struct SceneFilePath {
    pub path: Option<String>,
    pub metadata: JsnMetadata,
    pub last_directory: Option<PathBuf>,
}

fn get_window_handle(world: &mut World) -> Option<RawHandleWrapper> {
    world
        .query_filtered::<&RawHandleWrapper, With<PrimaryWindow>>()
        .single(world)
        .ok()
        .cloned()
}

fn spawn_save_dialog(world: &mut World) {
    let raw_handle = get_window_handle(world);
    let last_dir = world.resource::<SceneFilePath>().last_directory.clone();

    let mut dialog = AsyncFileDialog::new()
        .add_filter("JSN Scene", &["jsn"])
        .set_file_name("scene.jsn");

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

fn spawn_open_dialog(world: &mut World) {
    let raw_handle = get_window_handle(world);
    let last_dir = world.resource::<SceneFilePath>().last_directory.clone();

    let mut dialog = AsyncFileDialog::new()
        .add_filter("JSN Scene", &["jsn"])
        .add_filter("Legacy Scene", &["scene.json"]);

    if let Some(dir) = &last_dir {
        dialog = dialog.set_directory(dir);
    }
    if let Some(ref rh) = raw_handle {
        // SAFETY: called on the main thread during an exclusive system
        let handle = unsafe { rh.get_handle() };
        dialog = dialog.set_parent(&handle);
    }

    let task = AsyncComputeTaskPool::get().spawn(async move { dialog.pick_file().await });
    world.insert_resource(SceneDialogTask::Load(task));
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

fn save_scene_inner(world: &mut World) -> Result<(), BevyError> {
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
        emit_bsn_scene_with_inline_assets(world, &parent_path)
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
    project.project.layout = Some(layout_json);

    if let Err(e) = crate::project::save_project_config(&root, &project) {
        warn!("Failed to save project config: {e}");
    } else {
        world.resource_mut::<crate::project::ProjectRoot>().config = project;
    }
}

pub fn load_scene(world: &mut World) {
    if world.contains_resource::<SceneDialogTask>() {
        return; // Dialog already open
    }
    spawn_open_dialog(world);
}

struct JsnSerializerProcessor<'a> {
    parent_path: Cow<'a, Path>,
    /// Maps runtime asset IDs (no path) to inline `#Name` references.
    inline_assets: &'a HashMap<UntypedAssetId, String>,
    /// Maps scene entities to their index in the entity array.
    entity_to_index: &'a HashMap<Entity, usize>,
}

impl<'a> ReflectSerializerProcessor for JsnSerializerProcessor<'a> {
    fn try_serialize<S>(
        &self,
        value: &dyn PartialReflect,
        registry: &TypeRegistry,
        serializer: S,
    ) -> Result<Result<S::Ok, S>, S::Error>
    where
        S: Serializer,
    {
        let Some(value) = value.try_as_reflect() else {
            return Ok(Err(serializer));
        };
        let type_id = value.reflect_type_info().type_id();

        // Non-finite floats: JSON has no infinity/NaN, serialize as descriptive strings
        if type_id == TypeId::of::<f32>()
            && let Some(&v) = value.as_any().downcast_ref::<f32>()
            && !v.is_finite()
        {
            let s = if v == f32::INFINITY {
                "inf"
            } else if v == f32::NEG_INFINITY {
                "-inf"
            } else {
                "NaN"
            };
            return Ok(Ok(serializer.serialize_str(s)?));
        }
        if type_id == TypeId::of::<f64>()
            && let Some(&v) = value.as_any().downcast_ref::<f64>()
            && !v.is_finite()
        {
            let s = if v == f64::INFINITY {
                "inf"
            } else if v == f64::NEG_INFINITY {
                "-inf"
            } else {
                "NaN"
            };
            return Ok(Ok(serializer.serialize_str(s)?));
        }

        // Handle<T> -> path string or inline #Name
        if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(type_id) {
            let untyped_handle = reflect_handle
                .downcast_handle_untyped(value.as_any())
                .expect("This must have been a handle");

            // Check collected asset references first (both inline and external)
            if let Some(inline_name) = self.inline_assets.get(&untyped_handle.id()) {
                return Ok(Ok(serializer.serialize_str(inline_name)?));
            }

            if let Some(path) = untyped_handle.path() {
                // Uncollected external asset  -- serialize as relative path (backward compat)
                let rel = pathdiff::diff_paths(path.path(), &self.parent_path)
                    .unwrap_or_else(|| path.path().to_owned());
                let mut path_str = rel.to_string_lossy().into_owned();
                if let Some(label) = path.label() {
                    path_str.push('#');
                    path_str.push_str(label);
                }
                return Ok(Ok(serializer.serialize_str(&path_str)?));
            }

            // Unknown handle (no path, not inline)  -- serialize as null
            return Ok(Ok(serializer.serialize_unit()?));
        }

        // Entity -> scene-local index
        if type_id == TypeId::of::<Entity>() {
            if let Some(entity) = value.as_any().downcast_ref::<Entity>()
                && let Some(&idx) = self.entity_to_index.get(entity)
            {
                return Ok(Ok(serializer.serialize_u64(idx as u64)?));
            }
            return Ok(Ok(serializer.serialize_unit()?));
        }

        Ok(Err(serializer))
    }
}

pub(crate) struct JsnDeserializerProcessor<'a> {
    pub(crate) asset_server: &'a AssetServer,
    pub(crate) parent_path: &'a Path,
    /// Maps inline `#Name` references to loaded handles.
    pub(crate) local_assets: &'a HashMap<String, UntypedHandle>,
    /// Maps catalog `@Name` references to loaded handles.
    pub(crate) catalog_assets: &'a HashMap<String, UntypedHandle>,
    /// Maps scene-local indices to spawned entities.
    pub(crate) entity_map: &'a [Entity],
}

impl<'a> ReflectDeserializerProcessor for JsnDeserializerProcessor<'a> {
    fn try_deserialize<'de, D>(
        &mut self,
        registration: &TypeRegistration,
        _registry: &TypeRegistry,
        deserializer: D,
    ) -> Result<Result<Box<dyn PartialReflect>, D>, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Non-finite floats: deserialize from string ("inf", "-inf", "NaN") or number
        if registration.type_id() == TypeId::of::<f32>() {
            let val = deserializer
                .deserialize_any(F32Visitor)
                .map_err(<D::Error as serde::de::Error>::custom)?;
            return Ok(Ok(Box::new(val).into_partial_reflect()));
        }
        if registration.type_id() == TypeId::of::<f64>() {
            let val = deserializer
                .deserialize_any(F64Visitor)
                .map_err(<D::Error as serde::de::Error>::custom)?;
            return Ok(Ok(Box::new(val).into_partial_reflect()));
        }

        // Handle<T>  -- deserialize from path string or #Name
        if registration.data::<ReflectHandle>().is_some() {
            let type_info = registration.type_info();

            let relative_path = match deserializer.deserialize_any(&*self) {
                Ok(path) => path,
                Err(error) => {
                    error!(
                        "Failed to deserialize `{}`: {:?}",
                        type_info.type_path(),
                        error
                    );
                    return Err(error);
                }
            };

            // Null sentinel (from old files with "material": null) -> default handle
            if relative_path.is_empty()
                && let Some(reflect_default) = registration.data::<ReflectDefault>()
            {
                return Ok(Ok(reflect_default.default().into_partial_reflect()));
            }

            // Check for catalog asset reference (@Name)
            if relative_path.starts_with('@') {
                if let Some(handle) = self.catalog_assets.get(&relative_path) {
                    return Ok(Ok(Box::new(handle.clone()).into_partial_reflect()));
                }
                warn!(
                    "Catalog asset '{}' not found  -- using default",
                    relative_path
                );
                if let Some(reflect_default) = registration.data::<ReflectDefault>() {
                    return Ok(Ok(reflect_default.default().into_partial_reflect()));
                }
            }

            // Check for inline asset reference (#Name)
            if let Some(handle) = self.local_assets.get(&relative_path) {
                return Ok(Ok(Box::new(handle.clone()).into_partial_reflect()));
            }

            // External asset path. Resolve to a filesystem path
            // first (in case it was scene-relative), then strip
            // the assets-dir prefix so AssetServer treats it as
            // an approved path.
            let stem_pos = relative_path.find('#').unwrap_or(relative_path.len());
            let stem = self.relative_path_to_asset_path(&relative_path[0..stem_pos]);
            let stem_fs = stem.to_string_lossy().into_owned();
            let mut asset_path = crate::entity_ops::to_asset_path(&stem_fs);
            asset_path.push_str(&relative_path[stem_pos..]);

            let handle = self.asset_server.load_builder().load_untyped(asset_path);
            return Ok(Ok(Box::new(handle).into_partial_reflect()));
        }

        // Entity  -- deserialize from scene-local index
        if registration.type_id() == TypeId::of::<Entity>() {
            let Ok(idx_str) = deserializer.deserialize_u64(&*self) else {
                // Not a valid index, return placeholder
                return Ok(Ok(Box::new(Entity::PLACEHOLDER).into_partial_reflect()));
            };
            let idx: usize = idx_str.parse().unwrap_or(usize::MAX);
            let entity = self
                .entity_map
                .get(idx)
                .copied()
                .unwrap_or(Entity::PLACEHOLDER);
            return Ok(Ok(Box::new(entity).into_partial_reflect()));
        }

        Ok(Err(deserializer))
    }
}

impl<'a> Visitor<'_> for &'a JsnDeserializerProcessor<'a> {
    type Value = String;

    fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
        write!(formatter, "a string, integer, or null")
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(String::new())
    }

    fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.to_owned())
    }

    fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(v.to_string())
    }
}

struct F32Visitor;

impl Visitor<'_> for F32Visitor {
    type Value = f32;

    fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
        write!(formatter, "a number or float string (inf, -inf, NaN)")
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
        Ok(v as f32)
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(v as f32)
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(v as f32)
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        match v {
            "inf" | "Infinity" => Ok(f32::INFINITY),
            "-inf" | "-Infinity" => Ok(f32::NEG_INFINITY),
            "NaN" | "nan" => Ok(f32::NAN),
            _ => Err(E::custom(format!("unexpected float string: {v}"))),
        }
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(0.0) // backward compat: old files with null
    }
}

struct F64Visitor;

impl Visitor<'_> for F64Visitor {
    type Value = f64;

    fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
        write!(formatter, "a number or float string (inf, -inf, NaN)")
    }

    fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
        Ok(v)
    }

    fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
        Ok(v as f64)
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(v as f64)
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        match v {
            "inf" | "Infinity" => Ok(f64::INFINITY),
            "-inf" | "-Infinity" => Ok(f64::NEG_INFINITY),
            "NaN" | "nan" => Ok(f64::NAN),
            _ => Err(E::custom(format!("unexpected float string: {v}"))),
        }
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(0.0) // backward compat: old files with null
    }
}

impl<'a> JsnDeserializerProcessor<'a> {
    fn relative_path_to_asset_path(&self, asset_path: &str) -> PathBuf {
        let mut asset_path = Path::new(asset_path).to_owned();
        if asset_path.is_relative() {
            asset_path = self.parent_path.join(asset_path);
        }
        asset_path
    }
}

/// Recursively walk a reflected value looking for `Handle<T>` fields that are runtime-created.
fn collect_handles_from_reflect(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    world: &World,
    parent_path: &Path,
    id_to_name: &mut HashMap<UntypedAssetId, String>,
    asset_data: &mut HashMap<String, HashMap<String, serde_json::Value>>,
    counters: &mut HashMap<String, usize>,
    catalog_id_to_name: &HashMap<UntypedAssetId, String>,
) {
    let Some(value) = value.try_as_reflect() else {
        return;
    };
    let type_id = value.reflect_type_info().type_id();

    // Check if this is a Handle<T>
    if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(type_id) {
        let untyped_handle = reflect_handle
            .downcast_handle_untyped(value.as_any())
            .expect("This must have been a handle");

        // Already collected  -- skip
        if id_to_name.contains_key(&untyped_handle.id()) {
            return;
        }

        // Check catalog first  -- if this handle is a catalog asset with an @Name,
        // emit @Name and don't inline it into the scene's asset table.
        // Skip #-prefixed entries (internal catalog references like #Image8)
        // because those are only meaningful inside the catalog, not in scenes.
        if let Some(catalog_name) = catalog_id_to_name.get(&untyped_handle.id())
            && catalog_name.starts_with('@')
        {
            id_to_name.insert(untyped_handle.id(), catalog_name.clone());
            return;
        }

        // External file-backed resource  -- store as a path string entry
        if let Some(asset_path) = untyped_handle.path() {
            let asset_type_id = reflect_handle.asset_type_id();
            let Some(asset_registration) = registry.get(asset_type_id) else {
                return;
            };
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
            let inline_name = format!("#{short_name}{counter}");
            *counter += 1;

            let rel = pathdiff::diff_paths(asset_path.path(), parent_path)
                .unwrap_or_else(|| asset_path.path().to_owned());
            let mut path_str = rel.to_string_lossy().into_owned();
            if let Some(label) = asset_path.label() {
                path_str.push('#');
                path_str.push_str(label);
            }

            id_to_name.insert(untyped_handle.id(), inline_name.clone());
            asset_data
                .entry(asset_type_path)
                .or_default()
                .insert(inline_name, serde_json::Value::String(path_str));
            return;
        }

        // Skip default/UUID handles (not backed by a live asset)
        if matches!(untyped_handle, UntypedHandle::Uuid { .. }) {
            return;
        }

        let asset_type_id = reflect_handle.asset_type_id();
        let Some(asset_registration) = registry.get(asset_type_id) else {
            return;
        };
        let Some(reflect_asset) = asset_registration.data::<ReflectAsset>() else {
            return;
        };

        let asset_type_path = asset_registration
            .type_info()
            .type_path_table()
            .path()
            .to_string();

        // Get the asset data and serialize it
        let Some(asset_reflect) = reflect_asset.get(world, untyped_handle.id()) else {
            return;
        };

        // Recurse into the asset to collect nested handles (e.g. textures inside materials)
        // before serializing, so they get #Name entries and the serializer emits refs not paths.
        collect_handles_from_reflect(
            asset_reflect.as_partial_reflect(),
            registry,
            world,
            parent_path,
            id_to_name,
            asset_data,
            counters,
            catalog_id_to_name,
        );

        // Generate a name like "Material0", "Material1"
        let counter = counters.entry(asset_type_path.clone()).or_insert(0);
        let short_name = asset_type_path
            .rsplit("::")
            .next()
            .unwrap_or(&asset_type_path);
        let inline_name = format!("#{short_name}{counter}");
        *counter += 1;

        // Serialize the asset using the processor (for nested handles like textures inside materials)
        let ser_processor = JsnSerializerProcessor {
            parent_path: Cow::Borrowed(parent_path),
            inline_assets: id_to_name, // partial map, but handles already collected will be there
            entity_to_index: &HashMap::new(),
        };
        let serializer =
            TypedReflectSerializer::with_processor(asset_reflect, registry, &ser_processor);
        if let Ok(json_value) = serde_json::to_value(&serializer) {
            id_to_name.insert(untyped_handle.id(), inline_name.clone());
            asset_data
                .entry(asset_type_path)
                .or_default()
                .insert(inline_name, json_value);
        }

        return;
    }

    // Recurse into struct/tuple/list/map fields
    match value.reflect_ref() {
        bevy::reflect::ReflectRef::Struct(s) => {
            for i in 0..s.field_len() {
                if let Some(field) = s.field_at(i) {
                    collect_handles_from_reflect(
                        field,
                        registry,
                        world,
                        parent_path,
                        id_to_name,
                        asset_data,
                        counters,
                        catalog_id_to_name,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::TupleStruct(ts) => {
            for i in 0..ts.field_len() {
                if let Some(field) = ts.field(i) {
                    collect_handles_from_reflect(
                        field,
                        registry,
                        world,
                        parent_path,
                        id_to_name,
                        asset_data,
                        counters,
                        catalog_id_to_name,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::Tuple(t) => {
            for i in 0..t.field_len() {
                if let Some(field) = t.field(i) {
                    collect_handles_from_reflect(
                        field,
                        registry,
                        world,
                        parent_path,
                        id_to_name,
                        asset_data,
                        counters,
                        catalog_id_to_name,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::List(l) => {
            for i in 0..l.len() {
                if let Some(item) = l.get(i) {
                    collect_handles_from_reflect(
                        item,
                        registry,
                        world,
                        parent_path,
                        id_to_name,
                        asset_data,
                        counters,
                        catalog_id_to_name,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::Array(a) => {
            for i in 0..a.len() {
                if let Some(item) = a.get(i) {
                    collect_handles_from_reflect(
                        item,
                        registry,
                        world,
                        parent_path,
                        id_to_name,
                        asset_data,
                        counters,
                        catalog_id_to_name,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::Map(m) => {
            for (_k, v) in m.iter() {
                collect_handles_from_reflect(
                    v,
                    registry,
                    world,
                    parent_path,
                    id_to_name,
                    asset_data,
                    counters,
                    catalog_id_to_name,
                );
            }
        }
        bevy::reflect::ReflectRef::Set(s) => {
            for item in s.iter() {
                collect_handles_from_reflect(
                    item,
                    registry,
                    world,
                    parent_path,
                    id_to_name,
                    asset_data,
                    counters,
                    catalog_id_to_name,
                );
            }
        }
        bevy::reflect::ReflectRef::Enum(e) => {
            for i in 0..e.field_len() {
                if let Some(field) = e.field_at(i) {
                    collect_handles_from_reflect(
                        field,
                        registry,
                        world,
                        parent_path,
                        id_to_name,
                        asset_data,
                        counters,
                        catalog_id_to_name,
                    );
                }
            }
        }
        bevy::reflect::ReflectRef::Opaque(_) => {}
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
    let skip_ids: HashSet<TypeId> = HashSet::from([
        TypeId::of::<GlobalTransform>(),
        TypeId::of::<InheritedVisibility>(),
        TypeId::of::<ViewVisibility>(),
        TypeId::of::<ChildOf>(),
        TypeId::of::<Children>(),
    ]);

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
        bevy::reflect::ReflectRef::Opaque(_) => {}
    }
    found
}

/// Emit the live BSN document to text with runtime inline assets embedded and
/// their handle references resolved.
///
/// The editor maintains the document incrementally with plain component
/// patches, so any asset `Handle<T>` field on a kept component was recorded
/// without an asset context and resolves to an empty string on a bare
/// [`jackdaw_bsn::emit_scene`]. Mirroring the JSN snapshot path, this does a
/// capture-time asset pass: it walks the document's entities for pathless
/// runtime asset handles, embeds each as a `#Name` root, and re-derives the
/// handle-bearing component patches so their fields emit the reference names.
/// Assets that already carry a filesystem path or a catalog `@Name`, and scene
/// assets already embedded as roots, resolve through their existing sources.
///
/// The document is restored to its prior state before returning, so emission
/// stays read-only: the temporary roots are dropped and the rewritten patches
/// are reverted.
pub fn emit_bsn_scene_with_inline_assets(world: &mut World, parent_path: &Path) -> String {
    if world.get_resource::<jackdaw_bsn::SceneBsnAst>().is_none() {
        return String::new();
    }

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

    let entities = doc_entities_in_order(world.resource::<jackdaw_bsn::SceneBsnAst>());
    let pass = {
        let reg = registry.read();
        collect_bsn_inline_assets(world, &reg, &entities, seed)
    };

    // Take the document out of the world so the sparsify pass,
    // `append_assets_to_ast`, and the patch rewrite can borrow the world
    // immutably alongside it.
    let mut ast = world
        .remove_resource::<jackdaw_bsn::SceneBsnAst>()
        .unwrap_or_default();

    // Reduce inherited prefab-instance descendants to sparse override entries
    // (`PrefabEntityId` plus only diverged fields). No-op when there is no
    // prefab cache or no prefab instances. Recorded so the live document is
    // restored after this read-only emit.
    let sparsify_restore = if world
        .get_resource::<crate::prefab::PrefabAstCache>()
        .is_some()
    {
        let cache = world.resource::<crate::prefab::PrefabAstCache>();
        let get_prefab = |p: &Path| cache.get(p);
        crate::prefab::resolver_bsn::sparsify_inherited_descendants_recording(&mut ast, &get_prefab)
    } else {
        Vec::new()
    };

    // No kept component references an asset handle: the document already emits
    // faithfully once sparsified.
    if pass.touched.is_empty() {
        let text = jackdaw_bsn::emit_scene(&ast);
        crate::prefab::resolver_bsn::restore_sparsified(&mut ast, sparsify_restore);
        world.insert_resource(ast);
        return text;
    }

    let asset_server = world.resource::<AssetServer>().clone();

    let roots_before = ast.roots.len();
    if !pass.refs.is_empty() {
        jackdaw_bsn::append_assets_to_ast(&mut ast, world, &pass.refs);
    }
    let added_roots: Vec<Entity> = ast.roots[roots_before..].to_vec();

    // Re-derive each handle-bearing component patch with the asset context so
    // its handle fields emit reference names or asset paths. Remember the prior
    // patch to restore the live document afterward.
    let mut restore: Vec<(Entity, jackdaw_bsn::BsnPatch)> = Vec::new();
    {
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
            if let Some(old) = ast.get_patch(patch_entity).cloned() {
                restore.push((patch_entity, old));
            }
            ast.set_patch(patch_entity, new_patch);
        }
    }

    let text = jackdaw_bsn::emit_scene(&ast);

    // Restore the live document: revert the rewritten patches and drop the
    // temporary asset roots so a later capture starts from the same state.
    for (patch_entity, old) in restore {
        ast.set_patch(patch_entity, old);
    }
    for root in added_roots {
        let child_patches = ast
            .get_patches(root)
            .map(|p| p.0.clone())
            .unwrap_or_default();
        ast.remove_from_roots(root);
        for pe in child_patches {
            ast.world.despawn(pe);
        }
        ast.world.despawn(root);
    }

    crate::prefab::resolver_bsn::restore_sparsified(&mut ast, sparsify_restore);

    world.insert_resource(ast);
    text
}

/// Emit a subset of the live BSN document (the given document nodes with
/// their subtrees) as self-contained BSN text for the clipboard.
///
/// Referenced runtime inline assets embed as `#Name` roots so a cross-scene
/// paste carries them along; only catalog `@Name` assets are assumed to
/// resolve at the destination. Stable node ids are stripped from the emitted
/// text, so a paste mints fresh ones. The live document is restored to its
/// prior state before returning.
pub(crate) fn emit_bsn_entities_with_inline_assets(
    world: &mut World,
    parent_path: &Path,
    nodes: &[Entity],
) -> String {
    if world.get_resource::<jackdaw_bsn::SceneBsnAst>().is_none() {
        return String::new();
    }

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
    let (subtree_nodes, entities) = {
        let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
        let mut subtree_nodes: Vec<Entity> = Vec::new();
        for &node in nodes {
            subtree_nodes.push(node);
            subtree_nodes.extend(ast.descendants_of(node));
        }
        let entities: Vec<Entity> = subtree_nodes
            .iter()
            .filter_map(|&n| ast.ecs_for_ast(n))
            .collect();
        (subtree_nodes, entities)
    };

    let pass = {
        let reg = registry.read();
        collect_bsn_inline_assets(world, &reg, &entities, seed)
    };

    let mut ast = world
        .remove_resource::<jackdaw_bsn::SceneBsnAst>()
        .unwrap_or_default();

    // Strip stable node ids from the copied subtrees; a paste mints fresh
    // ones, so the clipboard must not carry the source ids.
    let mut removed_id_patches: Vec<(Entity, jackdaw_bsn::BsnPatch)> = Vec::new();
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
            if let Some(patch) = ast.get_patch(pe).cloned() {
                removed_id_patches.push((node, patch));
            }
            if let Some(patches) = ast.get_patches_mut(node) {
                patches.0.retain(|&x| x != pe);
            }
            ast.world.despawn(pe);
        }
    }

    // Embed referenced runtime assets as roots and re-derive handle-bearing
    // component patches so their fields emit the reference names.
    let roots_before = ast.roots.len();
    let mut restore: Vec<(Entity, jackdaw_bsn::BsnPatch)> = Vec::new();
    let mut added_roots: Vec<Entity> = Vec::new();
    if !pass.touched.is_empty() {
        if !pass.refs.is_empty() {
            jackdaw_bsn::append_assets_to_ast(&mut ast, world, &pass.refs);
        }
        added_roots = ast.roots[roots_before..].to_vec();

        let asset_server = world.resource::<AssetServer>().clone();
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
            if let Some(old) = ast.get_patch(patch_entity).cloned() {
                restore.push((patch_entity, old));
            }
            ast.set_patch(patch_entity, new_patch);
        }
    }

    let mut emit_list = added_roots.clone();
    emit_list.extend_from_slice(nodes);
    let text = jackdaw_bsn::emit_entities(&ast, &emit_list);

    // Restore the live document: revert rewritten patches, drop the
    // temporary asset roots, and re-add the stripped id patches.
    for (patch_entity, old) in restore {
        ast.set_patch(patch_entity, old);
    }
    for root in added_roots {
        let child_patches = ast
            .get_patches(root)
            .map(|p| p.0.clone())
            .unwrap_or_default();
        ast.remove_from_roots(root);
        for pe in child_patches {
            ast.world.despawn(pe);
        }
        ast.world.despawn(root);
    }
    for (node, patch) in removed_id_patches {
        let pe = ast.world.spawn(patch).id();
        if let Some(patches) = ast.get_patches_mut(node) {
            patches.0.push(pe);
        }
    }

    world.insert_resource(ast);
    text
}

/// Serialize a single runtime asset (and its nested handles like textures)
/// into `JsnAssets` format. `parent_path` is used to compute relative file paths
/// (should be the assets directory so texture paths resolve correctly on reload).
pub fn serialize_asset_into(
    world: &World,
    handle: UntypedHandle,
    name: &str,
    parent_path: &Path,
    assets: &mut JsnAssets,
) {
    let registry = world.resource::<AppTypeRegistry>().read();

    // UntypedHandle::type_id() returns the *asset* type ID directly (e.g. StandardMaterial)
    let asset_type_id = handle.type_id();
    let Some(asset_registration) = registry.get(asset_type_id) else {
        return;
    };
    let Some(reflect_asset) = asset_registration.data::<ReflectAsset>() else {
        return;
    };
    let asset_type_path = asset_registration
        .type_info()
        .type_path_table()
        .path()
        .to_string();

    let Some(asset_reflect) = reflect_asset.get(world, handle.id()) else {
        return;
    };

    // Collect nested handles (e.g. textures inside a StandardMaterial)
    let empty_catalog = HashMap::new();
    let mut id_to_name: HashMap<UntypedAssetId, String> = HashMap::new();
    let mut nested_assets: HashMap<String, HashMap<String, serde_json::Value>> = HashMap::new();

    // Seed counters from existing entries so subsequent calls don't reuse names
    let mut counters: HashMap<String, usize> = HashMap::new();
    for (type_path, entries) in &assets.0 {
        counters.insert(type_path.clone(), entries.len());
    }

    collect_handles_from_reflect(
        asset_reflect.as_partial_reflect(),
        &registry,
        world,
        parent_path,
        &mut id_to_name,
        &mut nested_assets,
        &mut counters,
        &empty_catalog,
    );

    // Merge nested asset entries (images etc.) into the output JsnAssets
    for (type_path, entries) in nested_assets {
        let target = assets.0.entry(type_path).or_default();
        for (entry_name, value) in entries {
            target.insert(entry_name, value);
        }
    }

    // Serialize the root asset itself
    let ser_processor = JsnSerializerProcessor {
        parent_path: Cow::Borrowed(parent_path),
        inline_assets: &id_to_name,
        entity_to_index: &HashMap::new(),
    };
    let serializer =
        TypedReflectSerializer::with_processor(asset_reflect, &registry, &ser_processor);
    if let Ok(json_value) = serde_json::to_value(&serializer) {
        assets
            .0
            .entry(asset_type_path)
            .or_default()
            .insert(name.to_string(), json_value);
    }
}

pub fn load_scene_from_file(world: &mut World, chosen: &std::path::Path) {
    finish_load_scene(world, chosen);
}

fn finish_load_scene(world: &mut World, chosen: &std::path::Path) {
    let mut path = chosen.to_string_lossy().to_string();

    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(err) => {
            warn!("Failed to read scene file '{path}': {err}");
            return;
        }
    };

    // Only update `last_directory` once the file has been successfully read
    // and we're committed to the load. A failed read must NOT leak a stale
    // path into the dialog state.
    world.resource_mut::<SceneFilePath>().last_directory =
        chosen.parent().map(std::path::Path::to_path_buf);

    if path.ends_with(".scene.json") {
        // Legacy format: raw DynamicWorld JSON
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();

        use bevy::world_serialization::serde::WorldDeserializer;
        let mut asset_server = world.resource_mut::<AssetServer>();
        let scene_deserializer = WorldDeserializer {
            type_registry: &registry,
            load_from_path: &mut *asset_server,
        };
        let mut json_de = serde_json::Deserializer::from_str(&json);
        let scene = match scene_deserializer.deserialize(&mut json_de) {
            Ok(scene) => scene,
            Err(err) => {
                warn!("Failed to deserialize legacy scene: {err}");
                return;
            }
        };

        drop(registry);
        clear_scene_entities(world);
        match scene.write_to_world(world, &mut Default::default()) {
            Ok(_) => info!("Scene loaded from {path} (legacy format)"),
            Err(err) => warn!("Failed to write scene to world: {err}"),
        }
    } else {
        let parent_path = Path::new(&path)
            .parent()
            .unwrap_or(Path::new("."))
            .to_path_buf();

        // Scenes load through the BSN document path. Legacy `.jsn` files are
        // not imported directly: they convert ON DISK first (writing the
        // `.bsn` sibling, keeping the original as `.jsn.bak`), and the editor
        // opens the converted file. Interactive open paths confirm with the
        // user before reaching here; direct calls apply the conversion tool.
        // The legacy metadata and camera framing carry over below.
        let (bsn_text, legacy_jsn) = if path.ends_with(".bsn") {
            (json, None)
        } else {
            let jsn = match jackdaw_jsn::format::parse_scene(&json) {
                Ok((jsn, version)) => {
                    if version[0] < 2 {
                        warn!(
                            "JSN format version {version:?} is not supported. Please re-save with the latest editor.",
                        );
                        return;
                    }
                    if version[0] < 3 {
                        info!("Migrating JSN v2 scene to v3 format");
                    }
                    jsn
                }
                Err(err) => {
                    warn!("Failed to parse JSN file: {err}");
                    return;
                }
            };
            let (bsn_path, _report) =
                match crate::jsn_to_bsn::convert_scene_file(world, Path::new(&path)) {
                    Ok(converted) => converted,
                    Err(err) => {
                        warn!("Failed to convert legacy scene '{path}': {err}");
                        return;
                    }
                };
            let bsn_text = match std::fs::read_to_string(&bsn_path) {
                Ok(text) => text,
                Err(err) => {
                    warn!(
                        "Failed to read converted scene '{}': {err}",
                        bsn_path.display()
                    );
                    return;
                }
            };
            info!(
                "Converted legacy scene to {}; original kept as .jsn.bak",
                bsn_path.display()
            );
            path = bsn_path.to_string_lossy().into_owned();
            (bsn_text, Some(jsn))
        };

        clear_scene_entities(world);

        // Populate the prefab cache from the document's IsA references, then
        // resolve instances so the spawn produces complete entities. A
        // resolution failure (e.g. cycle) falls back to the authored text so
        // the editor stays usable. Worlds without a prefab cache (headless
        // harnesses) spawn the authored text directly.
        let resolved_text = match jackdaw_bsn::parse_bsn_text(&bsn_text) {
            Ok(authored) if world.contains_resource::<crate::prefab::PrefabAstCache>() => {
                {
                    let mut cache = world.resource_mut::<crate::prefab::PrefabAstCache>();
                    crate::prefab::save_load::populate_cache_for_scene_bsn(
                        &authored,
                        &mut cache,
                        &parent_path,
                    );
                }
                let cache = world.resource::<crate::prefab::PrefabAstCache>();
                let get_prefab = |p: &Path| cache.get(p);
                match crate::prefab::resolver_bsn::resolve_scene(&authored, &get_prefab) {
                    Ok(resolved) => jackdaw_bsn::emit_scene(&resolved),
                    Err(e) => {
                        warn!("prefab resolution failed: {e}; spawning unresolved scene");
                        bsn_text.clone()
                    }
                }
            }
            Ok(_) => bsn_text.clone(),
            Err(err) => {
                warn!("Failed to parse BSN scene '{path}': {err}");
                return;
            }
        };

        match jackdaw_bsn::load_bsn_scene(world, &resolved_text) {
            Ok(loaded) => {
                // Fill the JSN AST so the remaining mirror readers keep
                // working; the loaded entities already carry their BSN
                // document links.
                register_entities_in_ast(world, &loaded.entities);
                info!(
                    "Scene loaded from {path} ({} entities, {} embedded assets)",
                    loaded.entities.len(),
                    loaded.assets.len()
                );
            }
            Err(err) => {
                warn!("Failed to load BSN scene '{path}': {err}");
                return;
            }
        }

        if let Some(jsn) = legacy_jsn {
            // Conversion persisted any re-minted node ids into the written
            // `.bsn`, so no dirty flag is needed for id healing.

            // Restore the saved camera framing if present.
            if let Some(camera) = jsn.editor.as_ref().and_then(|e| e.camera.as_ref()) {
                let restored: Transform = camera.clone().into();
                let mut q = world
                    .query_filtered::<&mut Transform, With<crate::viewport::MainViewportCamera>>();
                for mut tf in q.iter_mut(world) {
                    *tf = restored;
                }
            }

            // Restore metadata
            let mut scene_path = world.resource_mut::<SceneFilePath>();
            scene_path.metadata = jsn.metadata;
        }
    }

    world.resource_mut::<SceneFilePath>().path = Some(path);

    // Stacks were cleared by clear_scene_entities, so dirty baseline is 0
    world.resource_mut::<SceneDirtyState>().undo_len_at_save = 0;
}

/// Deserialize inline assets from the generic assets table.
/// Returns a map of `#Name` / `@Name` -> `UntypedHandle` for the deserializer processor.
/// Scan material definitions in `JsnAssets` to find image names used in non-color slots.
/// These images must be loaded with `is_srgb = false` to avoid gamma decoding artifacts.
fn collect_linear_image_names(assets: &JsnAssets) -> HashSet<String> {
    const LINEAR_SLOTS: &[&str] = &[
        "normal_map_texture",
        "metallic_roughness_texture",
        "occlusion_texture",
        "depth_map",
    ];
    let mut linear_names = HashSet::new();
    let mat_type = "bevy_pbr::pbr_material::StandardMaterial";
    if let Some(materials) = assets.0.get(mat_type) {
        for json_value in materials.values() {
            if let serde_json::Value::Object(obj) = json_value {
                for slot in LINEAR_SLOTS {
                    if let Some(serde_json::Value::String(img_name)) = obj.get(*slot) {
                        linear_names.insert(img_name.clone());
                    }
                }
            }
        }
    }
    linear_names
}

/// Serialize a type's reflect `Default` to a JSON value. Returns
/// `None` when the type has no `ReflectDefault` or the value cannot
/// be serialized.
fn serialize_reflect_default(
    registration: &TypeRegistration,
    registry: &TypeRegistry,
) -> Option<serde_json::Value> {
    let default = registration.data::<ReflectDefault>()?.default();
    let serializer = TypedReflectSerializer::new(default.as_partial_reflect(), registry);
    serde_json::to_value(&serializer).ok()
}

/// Produce a complete JSON object for `registration` by filling
/// on-disk values into the type's serialized default. Fields present in
/// `on_disk` are kept; missing fields take the default; keys not in the
/// struct are dropped.
///
/// Only top-level struct fields are filled. Enums, tuple structs,
/// tuples, collections, and opaque leaves are taken wholly from
/// `on_disk` so variant tags and element shapes stay intact. Handle
/// refs (`@Name`/`#Name` strings, `null`) ride through unchanged and
/// resolve in the deserializer processor; default handle fields
/// serialize to `null`, which the processor turns into a default handle.
fn fill_missing_with_defaults(
    on_disk: &serde_json::Value,
    registration: &TypeRegistration,
    registry: &TypeRegistry,
) -> serde_json::Value {
    let TypeInfo::Struct(struct_info) = registration.type_info() else {
        return on_disk.clone();
    };
    let serde_json::Value::Object(disk_map) = on_disk else {
        return on_disk.clone();
    };

    let mut out = match serialize_reflect_default(registration, registry) {
        Some(serde_json::Value::Object(default_map)) => default_map,
        // No default to fill from: pass the on-disk fields through.
        _ => disk_map.clone(),
    };

    for field in struct_info.iter() {
        let Some(disk_value) = disk_map.get(field.name()) else {
            continue;
        };
        let filled = match registry.get(field.type_id()) {
            Some(field_reg) => fill_missing_with_defaults(disk_value, field_reg, registry),
            None => disk_value.clone(),
        };
        out.insert(field.name().to_string(), filled);
    }

    serde_json::Value::Object(out)
}

pub fn load_inline_assets(
    world: &mut World,
    assets: &JsnAssets,
    parent_path: &Path,
) -> HashMap<String, UntypedHandle> {
    let mut local_assets: HashMap<String, UntypedHandle> = HashMap::new();

    // Pre-populate with catalog assets so @Name references in string values resolve
    let catalog_handles = world
        .get_resource::<crate::asset_catalog::AssetCatalog>()
        .map(|c| c.handles.clone())
        .unwrap_or_default();

    let linear_image_names = collect_linear_image_names(assets);

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry_guard = registry.read();
    let asset_server = world.resource::<AssetServer>().clone();

    // First pass: load all string-value entries (external file refs like textures).
    // These must be loaded before inline assets that may reference them.
    for (type_path, named_entries) in &assets.0 {
        for (name, json_value) in named_entries {
            let serde_json::Value::String(rel_path) = json_value else {
                continue;
            };

            // @Name reference -> resolve from catalog
            if rel_path.starts_with('@') {
                if let Some(handle) = catalog_handles.get(rel_path.as_str()) {
                    local_assets.insert(name.clone(), handle.clone());
                } else {
                    warn!("Catalog asset '{rel_path}' referenced by '{name}' not found");
                }
                continue;
            }

            let abs_path = if Path::new(rel_path).is_relative() {
                parent_path.join(rel_path)
            } else {
                PathBuf::from(rel_path)
            };
            let path_str = abs_path.to_string_lossy().into_owned();
            // AssetServer is rooted at the project's `assets/`;
            // strip the prefix so the load stays inside Bevy's
            // approved-path set (no `UnapprovedPathMode::Allow`).
            let asset_path = crate::entity_ops::to_asset_path(&path_str);

            let handle = if type_path == "bevy_image::image::Image" {
                if linear_image_names.contains(name) {
                    asset_server
                        .load_builder()
                        .with_settings(|s: &mut ImageLoaderSettings| s.is_srgb = false)
                        .load::<Image>(&asset_path)
                        .untyped()
                } else {
                    asset_server.load::<Image>(&asset_path).untyped()
                }
            } else {
                warn!(
                    "External asset entry '{name}' has unknown type '{type_path}'  -- loading untyped"
                );
                asset_server
                    .load::<bevy::asset::LoadedUntypedAsset>(&asset_path)
                    .untyped()
            };
            local_assets.insert(name.clone(), handle);
        }
    }

    // Second pass: deserialize all object-value entries (inline assets like materials)
    for (type_path, named_entries) in &assets.0 {
        let Some(registration) = registry_guard.get_with_type_path(type_path) else {
            warn!("Unknown asset type '{type_path}' in inline assets  -- skipping");
            continue;
        };
        let Some(reflect_asset) = registration.data::<ReflectAsset>() else {
            warn!("Type '{type_path}' has no ReflectAsset  -- skipping");
            continue;
        };

        for (name, json_value) in named_entries {
            // String entries already handled in first pass
            if json_value.is_string() {
                continue;
            }

            // Deserialize with processor to resolve nested handles (e.g. textures in materials)
            let mut deser_processor = JsnDeserializerProcessor {
                asset_server: &asset_server,
                parent_path,
                local_assets: &local_assets,
                catalog_assets: &catalog_handles,
                entity_map: &[],
            };

            // Fill fields the on-disk value omits using the type's
            // default so a strict deserializer sees a complete object.
            let filled = fill_missing_with_defaults(json_value, registration, &registry_guard);
            let deserializer = TypedReflectDeserializer::with_processor(
                registration,
                &registry_guard,
                &mut deser_processor,
            );
            let Ok(reflected) = deserializer.deserialize(&filled) else {
                warn!("Failed to deserialize inline asset '{name}' of type '{type_path}'");
                continue;
            };

            // Add into the asset store and get a handle
            let handle = reflect_asset.add(world, reflected.as_ref());
            local_assets.insert(name.clone(), handle);
        }
    }

    local_assets
}

/// Spawn entities from a `Vec<JsnEntity>` into the world using reflection.
/// Returns the spawned entity list (index-matched to input).
pub fn load_scene_from_jsn(
    world: &mut World,
    entities: &[JsnEntity],
    parent_path: &Path,
    local_assets: &HashMap<String, UntypedHandle>,
) -> Vec<Entity> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let asset_server = world.resource::<AssetServer>().clone();
    let catalog_handles = world
        .get_resource::<crate::asset_catalog::AssetCatalog>()
        .map(|c| c.handles.clone())
        .unwrap_or_default();

    // First pass: spawn empty entities (Name/Transform/Visibility come from components)
    let mut spawned: Vec<Entity> = Vec::new();
    for _jsn in entities.iter() {
        let entity = world.spawn_empty();
        spawned.push(entity.id());
    }

    // Second pass: deserialize extensible components via reflection with processor.
    //
    // `ChildOf` is inserted last, after components + require-chain
    // backfill. Bevy's `validate_parent_has_component` on `on_insert`
    // for `InheritedVisibility` / `GlobalTransform` would otherwise
    // log spurious B0004 warnings when children get their derived
    // components before parents do.
    let registry_guard = registry.read();
    for (i, jsn) in entities.iter().enumerate() {
        for (type_path, value) in &jsn.components {
            let Some(registration) = registry_guard.get_with_type_path(type_path) else {
                warn!("Unknown type '{type_path}'  -- skipping");
                continue;
            };
            if registration.data::<ReflectComponent>().is_none() {
                warn!("Type '{type_path}' has no ReflectComponent  -- skipping");
                continue;
            }

            // A marker or otherwise-empty component serializes to
            // `null`. Insert the type's default so the marker survives
            // the round-trip; a strict deserializer rejects `null`.
            if value.is_null()
                && let Some(reflect_default) = registration.data::<ReflectDefault>()
            {
                world
                    .entity_mut(spawned[i])
                    .insert_reflect(reflect_default.default().into_partial_reflect());
                continue;
            }

            let mut deser_processor = JsnDeserializerProcessor {
                asset_server: &asset_server,
                parent_path,
                local_assets,
                catalog_assets: &catalog_handles,
                entity_map: &spawned,
            };
            // Fill fields the on-disk value omits using the type's
            // default so a strict deserializer sees a complete object.
            let filled = fill_missing_with_defaults(value, registration, &registry_guard);
            let deserializer = TypedReflectDeserializer::with_processor(
                registration,
                &registry_guard,
                &mut deser_processor,
            );
            let Ok(reflected) = deserializer.deserialize(&filled) else {
                warn!("Failed to deserialize '{type_path}'  -- skipping");
                continue;
            };

            world.entity_mut(spawned[i]).insert_reflect(reflected);
        }
    }
    drop(registry_guard);

    // `insert_reflect` doesn't fire `#[require(...)]`. Backfill the
    // hierarchy-propagation chain so Bevy doesn't B0004-warn and
    // children render at correct world positions.
    for &entity in &spawned {
        let mut ent = world.entity_mut(entity);
        if ent.contains::<Transform>() {
            if !ent.contains::<GlobalTransform>() {
                ent.insert(GlobalTransform::default());
            }
            if !ent.contains::<TransformTreeChanged>() {
                ent.insert(TransformTreeChanged);
            }
        }
        if ent.contains::<Visibility>() {
            if !ent.contains::<InheritedVisibility>() {
                ent.insert(InheritedVisibility::default());
            }
            if !ent.contains::<ViewVisibility>() {
                ent.insert(ViewVisibility::default());
            }
        }
    }

    // Attach the stable node id so the live preview entity can be mapped
    // back to its authored node (PIE "save runtime values" relies on this).
    // The structural `id` is canonical; mint a fresh one only when the
    // source entry predates node ids.
    for (i, jsn) in entities.iter().enumerate() {
        let node_id = jsn
            .id
            .map(jackdaw_scene_types::SceneNodeId)
            .unwrap_or_else(jackdaw_scene_types::SceneNodeId::next);
        world.entity_mut(spawned[i]).insert(node_id);
    }

    // Wire ChildOf relationships now that every entity has its full
    // component set (see the ChildOf-last comment above).
    for (i, jsn) in entities.iter().enumerate() {
        if let Some(parent_idx) = jsn.parent
            && let Some(&parent_entity) = spawned.get(parent_idx)
        {
            world.entity_mut(spawned[i]).insert(ChildOf(parent_entity));
        }
    }

    // Post-load: re-trigger GLTF loading for GltfSource entities
    let gltf_entities: Vec<(Entity, String, usize)> = spawned
        .iter()
        .filter_map(|&e| {
            world
                .get::<jackdaw_scene_types::GltfSource>(e)
                .map(|gs| (e, gs.path.clone(), gs.scene_index))
        })
        .collect();
    for (entity, gltf_path, scene_index) in gltf_entities {
        let asset_server = world.resource::<AssetServer>();
        let asset_path: AssetPath<'static> = crate::entity_ops::to_asset_path(&gltf_path).into();
        let scene = asset_server.load(GltfAssetLabel::Scene(scene_index).from_asset(asset_path));
        world.entity_mut(entity).insert(WorldAssetRoot(scene));
    }

    spawned
}

pub fn new_scene(world: &mut World) {
    if is_scene_dirty(world) {
        world.insert_resource(PendingNewScene);
        world.commands().trigger(
            jackdaw_feathers::dialog::OpenDialogEvent::new("Unsaved Changes", "Save")
                .with_secondary_action("Discard")
                .with_description("You have unsaved changes. Save before creating a new scene?"),
        );
        world.flush();
        return;
    }
    do_new_scene(world);
}

fn do_new_scene(world: &mut World) {
    clear_scene_entities(world);
    let mut scene_path = world.resource_mut::<SceneFilePath>();
    scene_path.path = None;
    scene_path.metadata = JsnMetadata::default();
    world.resource_mut::<SceneDirtyState>().undo_len_at_save = 0;
    spawn_default_lighting(world);
    info!("New scene created");
}

/// Spawn default lighting for a new / empty scene (Sun directional
/// light + no ambient). Idempotent: if any `DirectionalLight` already
/// exists in the world we skip the Sun spawn so loaded scenes that
/// carry their own lighting don't get a duplicate `Sun`. The ambient
/// override is always applied since it is a `Resource` mutation, not a
/// spawn.
pub fn spawn_default_lighting(world: &mut World) {
    world.insert_resource(GlobalAmbientLight::NONE);

    let has_directional = world
        .query::<&DirectionalLight>()
        .iter(world)
        .next()
        .is_some();
    if has_directional {
        return;
    }

    let sun = world
        .spawn((
            Name::new("Sun"),
            DirectionalLight {
                shadow_maps_enabled: true,
                illuminance: 10000.0,
                ..default()
            },
            Transform::from_xyz(10.0, 20.0, 10.0).with_rotation(Quat::from_euler(
                EulerRot::XYZ,
                -0.8,
                0.4,
                0.0,
            )),
        ))
        .id();
    register_entity_in_ast(world, sun);
}

fn on_new_scene_save(
    _event: On<jackdaw_feathers::dialog::DialogActionEvent>,
    mut commands: Commands,
) {
    commands.queue(|world: &mut World| {
        if world.remove_resource::<PendingNewScene>().is_none() {
            return;
        }
        save_scene(world);
        do_new_scene(world);
    });
}

fn on_new_scene_discard(
    _event: On<jackdaw_feathers::dialog::DialogSecondaryActionEvent>,
    mut commands: Commands,
) {
    commands.queue(|world: &mut World| {
        if world.remove_resource::<PendingNewScene>().is_none() {
            return;
        }
        do_new_scene(world);
    });
}

/// If `PendingNewScene` exists but no dialog is open, the user dismissed via Esc/Cancel.
fn cleanup_pending_new_scene(
    pending: Option<Res<PendingNewScene>>,
    dialogs: Query<(), With<jackdaw_feathers::dialog::EditorDialog>>,
    mut commands: Commands,
) {
    if pending.is_some() && dialogs.is_empty() {
        commands.remove_resource::<PendingNewScene>();
    }
}

/// Collect every editor entity: each `EditorEntity` root and its
/// full descendant subtree. Used to exclude editor-internal trees
/// (panels, gizmos, picker overlays) when despawning scene entities.
fn collect_editor_entities(
    world: &mut World,
    roots_query: &mut QueryState<Entity, With<EditorEntity>>,
) -> HashSet<Entity> {
    let roots: Vec<Entity> = roots_query.iter(world).collect();

    let mut editor_set = HashSet::new();
    let mut stack = roots;
    while let Some(entity) = stack.pop() {
        if !editor_set.insert(entity) {
            continue;
        }
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    editor_set
}

/// Remove scene entities from the world (named non-editor entities + their descendants).
pub(crate) fn clear_scene_entities(world: &mut World) {
    if world.contains_resource::<jackdaw_bsn::SceneBsnAst>() {
        world.insert_resource(jackdaw_bsn::SceneBsnAst::default());
    }

    world
        .resource_mut::<crate::selection::Selection>()
        .entities
        .clear();

    if let Err(err) = world.run_system_cached(crate::hierarchy::clear_all_tree_rows) {
        error!("Failed to clear tree rows: {err}");
    }

    // Clear undo/redo stacks; they hold entity references that become
    // stale when the scene is dropped. Callers who want to preserve
    // history (e.g. undo/redo itself) use `despawn_scene_entities`
    // directly.
    let mut history = world.resource_mut::<jackdaw_commands::CommandHistory>();
    history.undo_stack.clear();
    history.redo_stack.clear();

    if let Err(err) = despawn_scene_entities(world) {
        error!("clear_scene_entities failed: {err}");
    }
}

/// Despawn every non-editor scene entity, leaving editor infrastructure
/// (cameras, grids, gizmos) and the undo/redo stacks intact. Used by
/// snapshot apply during undo/redo.
///
/// `bevy_enhanced_input`'s `Action<A>` component auto-inserts a
/// `Name` component (see its `#[require(Name::new(any::type_name::<A>()), ...)]`),
/// so BEI action entities are otherwise indistinguishable from
/// scene roots. They also carry the non-generic `ActionSettings`
/// marker, so excluding those keeps every operator's input routing
/// alive across an `apply_ast_to_world` pass; without action
/// entities in `Actions<CoreExtensionInputContext>`, BEI emits no
/// `Fire` events and every editor keybind goes silent.
pub(crate) fn despawn_scene_entities(world: &mut World) -> Result<(), BevyError> {
    let editor_set = world.run_system_cached(collect_editor_entities)?;

    let roots: Vec<Entity> = world
        .query_filtered::<Entity, (
            With<Name>,
            Without<bevy_enhanced_input::prelude::ActionSettings>,
        )>()
        .iter(world)
        .filter(|e| !editor_set.contains(e))
        .collect();

    let mut scene_set = HashSet::new();
    let mut stack = roots;
    while let Some(entity) = stack.pop() {
        if !scene_set.insert(entity) {
            continue;
        }
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }

    for entity in scene_set {
        if let Ok(entity_mut) = world.get_entity_mut(entity) {
            entity_mut.despawn();
        }
    }

    // Sweep any leftover chunk mesh children. Despawning a parent brush
    // does not always cascade through `ChildOf` in time; orphan chunk
    // meshes would otherwise survive, keep their `Transform` and
    // `MeshMaterial3d`, and render as a ghost box at world origin in
    // the next scene.
    let orphan_chunks: Vec<Entity> = world
        .query_filtered::<Entity, With<crate::brush::BrushMeshChunk>>()
        .iter(world)
        .collect();
    for entity in orphan_chunks {
        if let Ok(entity_mut) = world.get_entity_mut(entity) {
            entity_mut.despawn();
        }
    }

    Ok(())
}

fn poll_scene_dialog(world: &mut World) {
    let Some(mut task) = world.remove_resource::<SceneDialogTask>() else {
        return;
    };

    match &mut task {
        SceneDialogTask::Save(t) => {
            let Some(result) = future::block_on(future::poll_once(t)) else {
                world.insert_resource(task); // Not ready, put it back
                return;
            };
            if let Some(file) = result {
                let path = file.path().to_path_buf();
                let path_str = path.to_string_lossy().to_string();
                let last_dir = path.parent().map(std::path::Path::to_path_buf);

                let mut scene_path = world.resource_mut::<SceneFilePath>();
                scene_path.path = Some(path_str);
                scene_path.last_directory = last_dir;

                // Bind the picked path onto the active scene tab so
                // subsequent swaps/saves go to the right file, and the
                // dirty-state and display name reflect "saved scene"
                // instead of "untitled-N".
                if let Some(mut scenes) = world.get_resource_mut::<crate::scenes::Scenes>() {
                    let active = scenes.active;
                    if let Some(tab) = scenes.tabs.get_mut(active) {
                        tab.path = Some(path.clone());
                        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                            tab.display_name = stem.to_string();
                        }
                    }
                }

                if let Err(err) = save_scene_inner(world) {
                    error!("scene save (after Save As dialog) failed: {err}");
                }
            }
        }
        SceneDialogTask::Load(t) => {
            let Some(result) = future::block_on(future::poll_once(t)) else {
                world.insert_resource(task);
                return;
            };
            if let Some(file) = result {
                // Legacy .jsn picks confirm conversion before loading.
                crate::migrate_dialog::request_open_with_conversion(
                    world,
                    file.path(),
                    crate::migrate_dialog::ConversionOpenTarget::Scene,
                );
            }
        }
    }
}

/// Register a single ECS entity in the live scene document: create and link
/// its node, then upsert a patch for every serializable component. Ensures
/// the entity carries a stable `SceneNodeId` (adopting an existing one,
/// minting a fresh one otherwise) so the node keeps a cross-process
/// identity. Skips entities already in the document.
pub fn register_entity_in_ast(world: &mut World, entity: Entity) {
    let Some(doc) = world.get_resource::<jackdaw_bsn::SceneBsnAst>() else {
        return;
    };
    if doc.ast_for(entity).is_some() {
        return;
    }
    if world
        .get::<jackdaw_scene_types::SceneNodeId>(entity)
        .is_none()
        && let Ok(mut entity_mut) = world.get_entity_mut(entity)
    {
        entity_mut.insert(jackdaw_scene_types::SceneNodeId::next());
    }
    let doc = world.resource::<jackdaw_bsn::SceneBsnAst>();
    let parent = world
        .get::<ChildOf>(entity)
        .map(ChildOf::parent)
        .filter(|p| doc.ast_for(*p).is_some());
    jackdaw_bsn::create_entity_in_ast(world, entity, parent);

    let registry = world.resource::<AppTypeRegistry>().clone();
    let skip_ids: HashSet<TypeId> = HashSet::from([
        TypeId::of::<GlobalTransform>(),
        TypeId::of::<InheritedVisibility>(),
        TypeId::of::<ViewVisibility>(),
        TypeId::of::<ChildOf>(),
        TypeId::of::<Children>(),
        TypeId::of::<Name>(),
        TypeId::of::<jackdaw_bsn::AstNodeRef>(),
        TypeId::of::<jackdaw_bsn::AstDirty>(),
    ]);
    let values: Vec<Box<dyn bevy::reflect::PartialReflect>> = {
        let reg = registry.read();
        let entity_ref = world.entity(entity);
        reg.iter()
            .filter(|registration| !skip_ids.contains(&registration.type_id()))
            .filter(|registration| {
                !should_skip_component(registration.type_info().type_path_table().path())
            })
            .filter_map(|registration| registration.data::<ReflectComponent>())
            .filter_map(|reflect_component| reflect_component.reflect(entity_ref))
            .map(bevy::reflect::PartialReflect::to_dynamic)
            .collect()
    };
    for value in values {
        crate::commands::sync_component_to_bsn_doc(world, entity, &*value, &registry);
    }
}

/// Register multiple ECS entities in the live scene document.
pub fn register_entities_in_ast(world: &mut World, entities: &[Entity]) {
    for &entity in entities {
        register_entity_in_ast(world, entity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A struct whose on-disk form lacks `added`, standing in for data
    /// that predates a field being added to the type.
    #[derive(Reflect, Default)]
    #[reflect(Default)]
    struct OldNew {
        kept: f32,
        added: u32,
    }

    /// Fills missing struct fields from the type default, keeps present
    /// fields, drops unknown keys, and leaves a strict deserializer able
    /// to build the value.
    #[test]
    fn fill_missing_with_defaults_completes_partial_struct() {
        let mut registry = TypeRegistry::new();
        registry.register::<OldNew>();
        registry.register::<f32>();
        registry.register::<u32>();
        let registration = registry.get(TypeId::of::<OldNew>()).unwrap();

        // Old data: `kept` only, plus a field that no longer exists.
        let on_disk = serde_json::json!({ "kept": 2.5, "gone": 9 });
        let filled = fill_missing_with_defaults(&on_disk, registration, &registry);

        let obj = filled.as_object().expect("filled value is an object");
        assert_eq!(
            obj.get("kept").and_then(serde_json::Value::as_f64),
            Some(2.5)
        );
        assert_eq!(
            obj.get("added").and_then(serde_json::Value::as_u64),
            Some(0)
        );
        assert!(!obj.contains_key("gone"), "unknown key must be dropped");

        let deserializer = TypedReflectDeserializer::new(registration, &registry);
        let reflected = deserializer
            .deserialize(&filled)
            .expect("strict deserialize succeeds on filled object");
        let value = OldNew::from_reflect(reflected.as_ref()).expect("materialize OldNew");
        assert_eq!(value.kept, 2.5);
        assert_eq!(value.added, 0);
    }

    /// A complete object round-trips unchanged: every present value is
    /// preserved, so behavior is a no-op for current data.
    #[test]
    fn fill_missing_with_defaults_is_noop_for_complete_object() {
        let mut registry = TypeRegistry::new();
        registry.register::<OldNew>();
        registry.register::<f32>();
        registry.register::<u32>();
        let registration = registry.get(TypeId::of::<OldNew>()).unwrap();

        let on_disk = serde_json::json!({ "kept": 1.0, "added": 7 });
        let filled = fill_missing_with_defaults(&on_disk, registration, &registry);

        let obj = filled.as_object().expect("filled value is an object");
        assert_eq!(
            obj.get("kept").and_then(serde_json::Value::as_f64),
            Some(1.0)
        );
        assert_eq!(
            obj.get("added").and_then(serde_json::Value::as_u64),
            Some(7)
        );
    }

    /// Every entity spawned from a scene carries a `SceneNodeId`, and the id
    /// survives a save (`build_scene_snapshot`) then load round-trip so the
    /// running game can map a live entity back to its authored node.
    #[test]
    fn spawned_entities_carry_node_id_and_round_trip() {
        use jackdaw_jsn::format::JsnEntity;
        use jackdaw_scene_types::SceneNodeId;

        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .add_plugins(AssetPlugin::default());
        app.register_type::<SceneNodeId>();
        app.register_type::<Name>();

        // Two entities with explicit on-disk ids (parent + child).
        let entities = vec![
            JsnEntity {
                id: Some(42),
                parent: None,
                components: HashMap::new(),
            },
            JsnEntity {
                id: Some(99),
                parent: Some(0),
                components: HashMap::new(),
            },
        ];

        let spawned =
            load_scene_from_jsn(app.world_mut(), &entities, Path::new("."), &HashMap::new());
        assert_eq!(spawned.len(), 2);

        let id0 = app
            .world()
            .get::<SceneNodeId>(spawned[0])
            .expect("spawned entity should carry SceneNodeId");
        let id1 = app
            .world()
            .get::<SceneNodeId>(spawned[1])
            .expect("spawned child should carry SceneNodeId");
        assert_eq!(*id0, SceneNodeId(42));
        assert_eq!(*id1, SceneNodeId(99));

        // Register the spawned entities in the live scene document and
        // confirm the ids ride along as the nodes' stable ids.
        app.world_mut()
            .insert_resource(jackdaw_bsn::SceneBsnAst::default());
        register_entities_in_ast(app.world_mut(), &spawned);
        let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
        let node0 = ast.ast_for(spawned[0]).expect("node 42 registered");
        let node1 = ast.ast_for(spawned[1]).expect("node 99 registered");
        assert_eq!(ast.stable_id_of(node0), Some(42));
        assert_eq!(ast.stable_id_of(node1), Some(99));
    }

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
