use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use bevy::{
    asset::AssetPath,
    ecs::reflect::AppTypeRegistry,
    prelude::*,
    reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer},
    tasks::{AsyncComputeTaskPool, IoTaskPool, Task, futures_lite::future},
    window::{PrimaryWindow, RawHandleWrapper},
};
use jackdaw_jsn::format::{
    JsnAssets, JsnEntity, JsnHeader, JsnMaterialDefinition, JsnMetadata, JsnScene,
};
use rfd::{AsyncFileDialog, FileHandle};
use serde::de::DeserializeSeed;

use crate::EditorEntity;

/// Component type path prefixes that should never be saved (runtime-only / internal).
const SKIP_COMPONENT_PREFIXES: &[&str] = &[
    "bevy_render::",
    "bevy_picking::",
    "bevy_window::",
    "bevy_ecs::observer::",
    "bevy_camera::primitives::",
    "bevy_camera::visibility::",
];

/// Specific component type paths that should never be saved.
const SKIP_COMPONENT_PATHS: &[&str] = &[
    "bevy_transform::components::transform::TransformTreeChanged",
    "bevy_light::cascade::Cascades",
];

pub fn should_skip_component(type_path: &str) -> bool {
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
        app.init_resource::<SceneFilePath>().add_systems(
            Update,
            (handle_scene_io_keys, poll_scene_dialog).run_if(in_state(crate::AppState::Editor)),
        );
    }
}

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
    // If no path is set yet, delegate to Save As
    let has_path = world.resource::<SceneFilePath>().path.is_some();
    if !has_path {
        save_scene_as(world);
        return;
    }

    save_scene_inner(world);
}

pub fn save_scene_as(world: &mut World) {
    if world.contains_resource::<SceneDialogTask>() {
        return; // Dialog already open
    }
    spawn_save_dialog(world);
}

fn save_scene_inner(world: &mut World) {
    let entities = build_scene_snapshot(world);

    // Build asset manifest by scanning brush textures and GLTF sources
    let assets = build_asset_manifest(world);

    // Build metadata
    let now = chrono_now();
    let scene_path_res = world.resource::<SceneFilePath>();
    let mut metadata = scene_path_res.metadata.clone();
    metadata.modified = now.clone();
    if metadata.created.is_empty() {
        metadata.created = now;
    }
    if metadata.name.is_empty() {
        metadata.name = "Untitled".to_string();
    }

    let jsn = JsnScene {
        jsn: JsnHeader::default(),
        metadata: metadata.clone(),
        assets,
        editor: None,
        scene: entities,
    };

    let json = match serde_json::to_string_pretty(&jsn) {
        Ok(json) => json,
        Err(err) => {
            warn!("Failed to serialize JSN: {err}");
            return;
        }
    };

    let path = {
        let scene_path = world.resource::<SceneFilePath>();
        scene_path
            .path
            .clone()
            .expect("save_scene_inner called without a path set")
    };

    // Save metadata back
    let mut scene_path = world.resource_mut::<SceneFilePath>();
    scene_path.metadata = metadata;

    // Write to disk on the IO task pool
    let path_clone = path.clone();
    IoTaskPool::get()
        .spawn(async move {
            match std::fs::write(&path_clone, &json) {
                Ok(()) => info!("Scene saved to {path_clone}"),
                Err(err) => warn!("Failed to write scene file: {err}"),
            }
        })
        .detach();
}

pub fn load_scene(world: &mut World) {
    if world.contains_resource::<SceneDialogTask>() {
        return; // Dialog already open
    }
    spawn_open_dialog(world);
}

fn finish_load_scene(world: &mut World, chosen: &std::path::Path) {
    let path = chosen.to_string_lossy().to_string();
    let last_dir = chosen.parent().map(|p| p.to_path_buf());

    // Update last directory
    world.resource_mut::<SceneFilePath>().last_directory = last_dir;

    let json = match std::fs::read_to_string(&path) {
        Ok(json) => json,
        Err(err) => {
            warn!("Failed to read scene file '{path}': {err}");
            return;
        }
    };

    if path.ends_with(".scene.json") {
        // Legacy format: raw DynamicScene JSON
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();

        use bevy::scene::serde::SceneDeserializer;
        let scene_deserializer = SceneDeserializer {
            type_registry: &registry,
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
        // JSN format
        let jsn: JsnScene = match serde_json::from_str(&json) {
            Ok(jsn) => jsn,
            Err(err) => {
                warn!("Failed to parse JSN file: {err}");
                return;
            }
        };

        clear_scene_entities(world);
        load_scene_from_jsn(world, &jsn.scene);

        // Merge embedded material definitions into the library
        if !jsn.assets.material_definitions.is_empty() {
            let mut library =
                world.resource_mut::<crate::material_definition::MaterialLibrary>();
            for jsn_def in &jsn.assets.material_definitions {
                // Don't overwrite existing definitions with the same name
                if library.get_by_name(&jsn_def.name).is_some() {
                    continue;
                }
                library.add(crate::material_definition::MaterialDefinition {
                    name: jsn_def.name.clone(),
                    base_color_texture: jsn_def.base_color_texture.clone(),
                    normal_map_texture: jsn_def.normal_map_texture.clone(),
                    metallic_roughness_texture: jsn_def.metallic_roughness_texture.clone(),
                    emissive_texture: jsn_def.emissive_texture.clone(),
                    occlusion_texture: jsn_def.occlusion_texture.clone(),
                    depth_texture: jsn_def.depth_texture.clone(),
                    base_color: jsn_def.base_color,
                    metallic: jsn_def.metallic,
                    perceptual_roughness: jsn_def.perceptual_roughness,
                    reflectance: jsn_def.reflectance,
                    emissive_intensity: jsn_def.emissive_intensity,
                    double_sided: jsn_def.double_sided,
                    flip_normal_map_y: jsn_def.flip_normal_map_y,
                });
            }
        }

        info!("Scene loaded from {path}");

        // Restore metadata
        let mut scene_path = world.resource_mut::<SceneFilePath>();
        scene_path.metadata = jsn.metadata;
    }

    world.resource_mut::<SceneFilePath>().path = Some(path);
}

pub fn new_scene(world: &mut World) {
    clear_scene_entities(world);
    let mut scene_path = world.resource_mut::<SceneFilePath>();
    scene_path.path = None;
    scene_path.metadata = JsnMetadata::default();
    info!("New scene created");
}

/// Build a `Vec<JsnEntity>` from scene entities (named + descendants) using reflection.
///
/// Only saves entities that have a `Name` component (excluding editor entities)
/// plus all their descendants. This naturally excludes Bevy internal entities,
/// monitors, windows, pointers, etc.
fn build_scene_snapshot(world: &mut World) -> Vec<JsnEntity> {
    let editor_set = collect_editor_entities(world);

    // Collect named non-editor entities as roots
    let roots: Vec<Entity> = world
        .query_filtered::<Entity, With<Name>>()
        .iter(world)
        .filter(|e| !editor_set.contains(e))
        .collect();

    // Expand to include all descendants
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

    let entities: Vec<Entity> = scene_set.into_iter().collect();

    // Build entity → index map for parent references
    let index_map: HashMap<Entity, usize> =
        entities.iter().enumerate().map(|(i, &e)| (e, i)).collect();

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();

    // Component types handled as explicit fields — skip in the generic loop
    let skip_ids: HashSet<TypeId> = HashSet::from([
        TypeId::of::<Name>(),
        TypeId::of::<Transform>(),
        TypeId::of::<GlobalTransform>(),
        TypeId::of::<Visibility>(),
        TypeId::of::<InheritedVisibility>(),
        TypeId::of::<ViewVisibility>(),
        TypeId::of::<ChildOf>(),
        TypeId::of::<Children>(),
    ]);

    entities
        .iter()
        .map(|&entity| {
            let entity_ref = world.entity(entity);

            // Core fields
            let name = entity_ref.get::<Name>().map(|n| n.to_string());
            let transform = entity_ref.get::<Transform>().map(|t| (*t).into());
            let visibility = entity_ref
                .get::<Visibility>()
                .map(|v| (*v).into())
                .unwrap_or_default();
            let parent = entity_ref
                .get::<ChildOf>()
                .and_then(|c| index_map.get(&c.parent()).copied());

            // Extensible components via reflection
            let mut components = HashMap::new();

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

                // Try to serialize — skip on failure (handles unserializable types like Mesh3d)
                let serializer = TypedReflectSerializer::new(component, &registry);
                if let Ok(value) = serde_json::to_value(&serializer) {
                    components.insert(type_path.to_string(), value);
                }
            }

            JsnEntity {
                name,
                transform,
                visibility,
                parent,
                components,
            }
        })
        .collect()
}

/// Spawn entities from a `Vec<JsnEntity>` into the world using reflection.
pub fn load_scene_from_jsn(world: &mut World, entities: &[JsnEntity]) {
    let registry = world.resource::<AppTypeRegistry>().clone();

    // First pass: spawn entities with core fields
    let mut spawned: Vec<Entity> = Vec::new();
    for jsn in entities {
        let mut entity = world.spawn_empty();
        if let Some(name) = &jsn.name {
            entity.insert(Name::new(name.clone()));
        }
        if let Some(t) = &jsn.transform {
            entity.insert(Transform::from(t.clone()));
        }
        let vis: Visibility = jsn.visibility.clone().into();
        entity.insert(vis);
        spawned.push(entity.id());
    }

    // Second pass: set parents (ChildOf)
    for (i, jsn) in entities.iter().enumerate() {
        if let Some(parent_idx) = jsn.parent {
            if let Some(&parent_entity) = spawned.get(parent_idx) {
                world.entity_mut(spawned[i]).insert(ChildOf(parent_entity));
            }
        }
    }

    // Third pass: deserialize extensible components via reflection
    let registry = registry.read();
    for (i, jsn) in entities.iter().enumerate() {
        for (type_path, value) in &jsn.components {
            let Some(registration) = registry.get_with_type_path(type_path) else {
                warn!("Unknown type '{type_path}' — skipping");
                continue;
            };
            let Some(reflect_component) = registration.data::<ReflectComponent>() else {
                warn!("Type '{type_path}' has no ReflectComponent — skipping");
                continue;
            };

            let deserializer = TypedReflectDeserializer::new(registration, &registry);
            let Ok(reflected) = deserializer.deserialize(value) else {
                warn!("Failed to deserialize '{type_path}' — skipping");
                continue;
            };

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                reflect_component.insert(
                    &mut world.entity_mut(spawned[i]),
                    reflected.as_ref(),
                    &registry,
                );
            }));
            if result.is_err() {
                warn!("Panic while inserting component '{type_path}' — skipping");
            }
        }
    }
    drop(registry);

    // Post-load: re-trigger GLTF loading for GltfSource entities
    let gltf_entities: Vec<(Entity, String, usize)> = spawned
        .iter()
        .filter_map(|&e| {
            world
                .get::<jackdaw_jsn::GltfSource>(e)
                .map(|gs| (e, gs.path.clone(), gs.scene_index))
        })
        .collect();
    for (entity, gltf_path, scene_index) in gltf_entities {
        let asset_server = world.resource::<AssetServer>();
        let asset_path: AssetPath<'static> = gltf_path.into();
        let scene = asset_server.load(GltfAssetLabel::Scene(scene_index).from_asset(asset_path));
        world.entity_mut(entity).insert(SceneRoot(scene));
    }
}

/// Collect the set of all editor entities (those with `EditorEntity` and all their descendants).
fn collect_editor_entities(world: &mut World) -> HashSet<Entity> {
    let roots: Vec<Entity> = world
        .query_filtered::<Entity, With<EditorEntity>>()
        .iter(world)
        .collect();

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
///
/// Uses the same logic as `build_scene_snapshot`: only despawns entities that have a
/// `Name` component (excluding editor entities) and all their descendants. This avoids
/// destroying Bevy system entities (Window, Monitor, Pointer, etc.).
fn clear_scene_entities(world: &mut World) {
    let editor_set = collect_editor_entities(world);

    // Collect named non-editor entities as roots
    let roots: Vec<Entity> = world
        .query_filtered::<Entity, With<Name>>()
        .iter(world)
        .filter(|e| !editor_set.contains(e))
        .collect();

    // Expand to include all descendants
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
}

/// Build an asset manifest by scanning entity components.
fn build_asset_manifest(world: &mut World) -> JsnAssets {
    let mut textures = Vec::new();
    let mut models = Vec::new();
    let mut material_names: Vec<String> = Vec::new();

    // Scan brush face textures and material names
    let mut brush_query = world.query::<&jackdaw_jsn::Brush>();
    for brush in brush_query.iter(world) {
        for face in &brush.faces {
            if let Some(ref path) = face.texture_path {
                if !textures.contains(path) {
                    textures.push(path.clone());
                }
            }
            if let Some(ref name) = face.material_name {
                if !material_names.contains(name) {
                    material_names.push(name.clone());
                }
            }
        }
    }

    // Scan GLTF sources
    let mut gltf_query = world.query::<&jackdaw_jsn::GltfSource>();
    for source in gltf_query.iter(world) {
        if !models.contains(&source.path) {
            models.push(source.path.clone());
        }
    }

    // Collect referenced material definitions from the library
    let library = world.resource::<crate::material_definition::MaterialLibrary>();
    let material_definitions: Vec<JsnMaterialDefinition> = material_names
        .iter()
        .filter_map(|name| {
            library.get_by_name(name).map(|def| JsnMaterialDefinition {
                name: def.name.clone(),
                base_color_texture: def.base_color_texture.clone(),
                normal_map_texture: def.normal_map_texture.clone(),
                metallic_roughness_texture: def.metallic_roughness_texture.clone(),
                emissive_texture: def.emissive_texture.clone(),
                occlusion_texture: def.occlusion_texture.clone(),
                depth_texture: def.depth_texture.clone(),
                base_color: def.base_color,
                metallic: def.metallic,
                perceptual_roughness: def.perceptual_roughness,
                reflectance: def.reflectance,
                emissive_intensity: def.emissive_intensity,
                double_sided: def.double_sided,
                flip_normal_map_y: def.flip_normal_map_y,
            })
        })
        .collect();

    textures.sort();
    models.sort();

    JsnAssets {
        textures,
        models,
        material_definitions,
    }
}

/// ISO 8601 timestamp (simplified — no chrono dependency).
fn chrono_now() -> String {
    // Use std::time for a basic timestamp
    let since_epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = since_epoch.as_secs();
    // Basic ISO 8601 approximation
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;
    // Days since 1970-01-01, approximate year/month/day
    let (year, month, day) = days_to_date(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

fn days_to_date(days: u64) -> (u64, u64, u64) {
    // Simplified calendar calculation
    let mut y = 1970;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        y += 1;
    }
    let month_days = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    let mut m = 0;
    for (i, &md) in month_days.iter().enumerate() {
        if remaining < md {
            m = i;
            break;
        }
        remaining -= md;
    }
    (y, m as u64 + 1, remaining + 1)
}

fn is_leap(y: u64) -> bool {
    (y.is_multiple_of(4) && !y.is_multiple_of(100)) || y.is_multiple_of(400)
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
                let last_dir = path.parent().map(|p| p.to_path_buf());

                let mut scene_path = world.resource_mut::<SceneFilePath>();
                scene_path.path = Some(path_str);
                scene_path.last_directory = last_dir;

                save_scene_inner(world);
            }
        }
        SceneDialogTask::Load(t) => {
            let Some(result) = future::block_on(future::poll_once(t)) else {
                world.insert_resource(task);
                return;
            };
            if let Some(file) = result {
                finish_load_scene(world, file.path());
            }
        }
    }
}

fn handle_scene_io_keys(world: &mut World) {
    let keyboard = world.resource::<ButtonInput<KeyCode>>();
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let s_pressed = keyboard.just_pressed(KeyCode::KeyS);
    let o_pressed = keyboard.just_pressed(KeyCode::KeyO);
    let n_pressed = keyboard.just_pressed(KeyCode::KeyN);

    if ctrl && shift && s_pressed {
        save_scene_as(world);
    } else if ctrl && s_pressed {
        save_scene(world);
    } else if ctrl && o_pressed {
        load_scene(world);
    } else if ctrl && shift && n_pressed {
        new_scene(world);
    }
}
