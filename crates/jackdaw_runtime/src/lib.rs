use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::fmt::{self, Formatter};
use std::path::{Path, PathBuf};

use bevy::asset::{
    AssetLoader, LoadContext, ReflectAsset, ReflectHandle, UntypedHandle, io::Reader,
};
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::image::ImageLoaderSettings;
use bevy::prelude::*;
use bevy::reflect::serde::{ReflectDeserializerProcessor, TypedReflectDeserializer};
use bevy::reflect::{TypeRegistration, TypeRegistry};
use jackdaw_jsn::JsnPlugin;
use jackdaw_jsn::format::{JsnAssets, JsnCatalog, JsnScene, JsnSceneV2};
use serde::Deserializer;
use serde::de::{DeserializeSeed, Visitor};

pub use jackdaw_jsn::{
    Brush, BrushFaceData, CustomProperties, EditorCategory, EditorDescription, EditorHidden,
    GltfSource, PropertyValue, SkipSerialization,
};

pub mod prelude {
    pub use crate::{
        EditorCategory, EditorDescription, EditorHidden, JackdawCatalog, JackdawCatalogPath,
        JackdawPlugin, JackdawSceneRoot, SkipSerialization,
    };
}

pub struct JackdawPlugin;

impl Plugin for JackdawPlugin {
    fn build(&self, app: &mut App) {
        // `JsnPlugin` registers every scene type for reflection
        // and installs `MeshRebuildPlugin` (which embeds the
        // bundled grid texture used as the brush fallback
        // material).
        app.add_plugins(JsnPlugin::default());

        app.init_asset::<JackdawScene>()
            .init_asset_loader::<JackdawSceneLoader>()
            .init_resource::<JackdawCatalog>();

        app.add_systems(Startup, load_project_catalog);
        app.add_systems(
            Update,
            (clear_modified_scene_roots, spawn_loaded_scenes).chain(),
        );
    }
}

/// Project-wide asset catalog. Maps `@Name` references found in
/// scene files to loaded `UntypedHandle`s.
///
/// Populated at startup from `.jsn/catalog.jsn` next to the Bevy
/// asset root (mirrors `FileAssetReader::get_base_path()`). To
/// load from a different location, insert a [`JackdawCatalogPath`]
/// resource before [`JackdawPlugin`] is built.
#[derive(Resource, Default)]
pub struct JackdawCatalog {
    handles: HashMap<String, UntypedHandle>,
}

impl JackdawCatalog {
    /// Look up a catalog handle by its `@Name` reference.
    pub fn get(&self, name: &str) -> Option<&UntypedHandle> {
        self.handles.get(name)
    }

    /// Number of catalog entries (each `@Name`).
    pub fn len(&self) -> usize {
        self.handles.len()
    }

    /// True when no catalog has been loaded.
    pub fn is_empty(&self) -> bool {
        self.handles.is_empty()
    }
}

/// Optional override for catalog discovery. Insert this resource
/// before [`JackdawPlugin`] to point the loader at an explicit
/// `catalog.jsn` path instead of running the default discovery.
#[derive(Resource, Clone, Debug)]
pub struct JackdawCatalogPath(pub PathBuf);

#[derive(Asset, TypePath)]
pub struct JackdawScene {
    jsn: JsnScene,
    parent_path: PathBuf,
}

impl JackdawScene {
    /// Build a scene asset directly from in-memory JSN data.
    /// Used by integration tests that drive scene-load codepaths
    /// without a real `.jsn` file on disk.
    pub fn new(jsn: JsnScene, parent_path: PathBuf) -> Self {
        Self { jsn, parent_path }
    }
}

/// Scene entities spawn as children of this root.
///
/// Requires `Transform` and `Visibility` so the hierarchy has a
/// propagation backbone (otherwise every child would have
/// `GlobalTransform`/`InheritedVisibility` but no upstream
/// chain, triggering Bevy B0004 warnings and silently breaking
/// rendering). Callers can spawn `JackdawSceneRoot(handle)` by
/// itself; Bevy fills in the requires.
#[derive(Component, Deref)]
#[require(Transform, Visibility)]
pub struct JackdawSceneRoot(pub Handle<JackdawScene>);

#[derive(Component)]
struct SceneSpawned;

#[derive(Debug, TypePath)]
struct JackdawSceneLoader;

impl FromWorld for JackdawSceneLoader {
    fn from_world(_world: &mut World) -> Self {
        Self
    }
}

impl AssetLoader for JackdawSceneLoader {
    type Asset = JackdawScene;
    type Settings = ();
    type Error = JackdawLoadError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| JackdawLoadError::Io(e.to_string()))?;

        let text =
            std::str::from_utf8(&bytes).map_err(|e| JackdawLoadError::Parse(e.to_string()))?;

        let jsn: JsnScene = match serde_json::from_str(text) {
            Ok(jsn) => jsn,
            Err(v3_err) => match serde_json::from_str::<JsnSceneV2>(text) {
                Ok(v2) => v2.migrate_to_v3(),
                Err(_) => return Err(JackdawLoadError::Parse(v3_err.to_string())),
            },
        };

        let parent_path = load_context
            .path()
            .path()
            .parent()
            .unwrap_or(Path::new(""))
            .to_owned();

        Ok(JackdawScene { jsn, parent_path })
    }

    fn extensions(&self) -> &[&str] {
        &["jsn"]
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JackdawLoadError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Parse error: {0}")]
    Parse(String),
}

/// On `JackdawScene` change, despawn the previously-spawned
/// children and clear `SceneSpawned` so the next
/// `spawn_loaded_scenes` tick re-instantiates from the new
/// asset content. Pair with Bevy's `file_watcher` feature to get
/// hot reload of `assets/scene.jsn` in the standalone runner.
fn clear_modified_scene_roots(
    mut events: bevy::ecs::message::MessageReader<bevy::asset::AssetEvent<JackdawScene>>,
    roots: Query<(Entity, &JackdawSceneRoot, Option<&Children>), With<SceneSpawned>>,
    mut commands: Commands,
) {
    use bevy::asset::AssetEvent;

    let modified: Vec<bevy::asset::AssetId<JackdawScene>> = events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Modified { id } | AssetEvent::LoadedWithDependencies { id } => Some(*id),
            _ => None,
        })
        .collect();
    if modified.is_empty() {
        return;
    }

    for (root_entity, root, kids) in &roots {
        if !modified.contains(&root.0.id()) {
            continue;
        }
        if let Some(kids) = kids {
            for child in kids.iter() {
                commands.entity(child).despawn();
            }
        }
        commands.entity(root_entity).remove::<SceneSpawned>();
    }
}

fn spawn_loaded_scenes(
    world: &mut World,
    scene_roots: &mut QueryState<(Entity, &JackdawSceneRoot), Without<SceneSpawned>>,
) {
    let to_spawn: Vec<(Entity, Handle<JackdawScene>)> = scene_roots
        .iter(world)
        .map(|(e, root)| (e, root.0.clone()))
        .collect();

    for (root_entity, handle) in to_spawn {
        let scenes = world.resource::<Assets<JackdawScene>>();
        let Some(scene) = scenes.get(&handle) else {
            continue;
        };
        let jsn = scene.jsn.clone();
        let parent_path = scene.parent_path.clone();

        let catalog_assets = world.resource::<JackdawCatalog>().handles.clone();
        let local_assets = load_inline_assets(world, &jsn.assets, &parent_path, &catalog_assets);
        spawn_scene_entities(
            world,
            root_entity,
            &jsn.scene,
            &parent_path,
            &local_assets,
            &catalog_assets,
        );

        world.entity_mut(root_entity).insert(SceneSpawned);
    }
}

/// Type paths the loader pulls out of the JSN reflected-component
/// stream and bundles into the per-entity `world.spawn`, so the
/// entity reaches its final structural state in one table move.
const TRANSFORM_TYPE_PATH: &str = "bevy_transform::components::transform::Transform";
const VISIBILITY_TYPE_PATH: &str = "bevy_render::view::visibility::Visibility";

fn spawn_scene_entities(
    world: &mut World,
    root_entity: Entity,
    entities: &[jackdaw_jsn::format::JsnEntity],
    parent_path: &Path,
    local_assets: &HashMap<String, UntypedHandle>,
    catalog_assets: &HashMap<String, UntypedHandle>,
) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let asset_server = world.resource::<AssetServer>().clone();

    // Process parents before children. JSN array order isn't
    // guaranteed parent-first (the save path reads from a HashSet),
    // so sort here. `spawned[i]` holds the resulting `Entity`,
    // reused for parent resolution and the gltf post-pass.
    let topo: Vec<usize> = topological_index_order(entities);
    let mut spawned: Vec<Entity> = vec![root_entity; entities.len()];

    let registry_guard = registry.read();

    for &i in &topo {
        let jsn = &entities[i];

        let parent_entity = match jsn.parent {
            Some(p_idx) if p_idx < entities.len() => spawned[p_idx],
            _ => root_entity,
        };

        // Pull Transform / Visibility into typed locals; defer the
        // rest for the post-spawn reflect-insert.
        let mut local_transform = Transform::default();
        let mut local_visibility = Visibility::default();
        let mut deferred: Vec<Box<dyn PartialReflect>> = Vec::new();

        for (type_path, value) in &jsn.components {
            let Some(registration) = registry_guard.get_with_type_path(type_path) else {
                // `jackdaw::*` types are editor-only; skip them
                // silently. Other unknowns log once.
                if !type_path.starts_with("jackdaw::") {
                    info!("Skipping unknown type '{type_path}' (not registered in runtime)");
                }
                continue;
            };
            if registration.data::<ReflectComponent>().is_none() {
                continue;
            }
            let mut processor = RuntimeDeserializerProcessor {
                asset_server: &asset_server,
                parent_path,
                local_assets,
                catalog_assets,
                entity_map: &spawned,
            };
            let deserializer = TypedReflectDeserializer::with_processor(
                registration,
                &registry_guard,
                &mut processor,
            );
            let Ok(reflected) = deserializer.deserialize(value) else {
                warn!("Failed to deserialize '{type_path}'; skipping");
                continue;
            };

            if type_path == TRANSFORM_TYPE_PATH {
                if let Some(t) =
                    <Transform as bevy::reflect::FromReflect>::from_reflect(reflected.as_ref())
                {
                    local_transform = t;
                }
            } else if type_path == VISIBILITY_TYPE_PATH {
                if let Some(v) =
                    <Visibility as bevy::reflect::FromReflect>::from_reflect(reflected.as_ref())
                {
                    local_visibility = v;
                }
            } else {
                deferred.push(reflected);
            }
        }

        // GT / IV from parent's already-final values + local overrides.
        let parent_gt = world
            .get::<GlobalTransform>(parent_entity)
            .copied()
            .unwrap_or(GlobalTransform::IDENTITY);
        let computed_gt = parent_gt.mul_transform(local_transform);

        let parent_iv = world
            .get::<InheritedVisibility>(parent_entity)
            .copied()
            .unwrap_or(InheritedVisibility::VISIBLE);
        let computed_iv = match local_visibility {
            Visibility::Hidden => InheritedVisibility::HIDDEN,
            Visibility::Visible => InheritedVisibility::VISIBLE,
            Visibility::Inherited => parent_iv,
        };

        // One archetype move for all structural state.
        let entity = world
            .spawn((
                local_transform,
                local_visibility,
                computed_gt,
                computed_iv,
                ChildOf(parent_entity),
            ))
            .id();
        spawned[i] = entity;

        // User components on top. `On<Insert, T>` fires here with
        // GlobalTransform / InheritedVisibility already correct.
        for reflected in deferred {
            world.entity_mut(entity).insert_reflect(reflected);
        }
    }
    drop(registry_guard);

    let gltf_entities: Vec<(Entity, String, usize)> = spawned
        .iter()
        .filter_map(|&e| {
            world
                .get::<jackdaw_jsn::GltfSource>(e)
                .map(|gs| (e, gs.path.clone(), gs.scene_index))
        })
        .collect();
    for (entity, gltf_path, scene_index) in gltf_entities {
        let resolved = if Path::new(&gltf_path).is_relative() {
            parent_path.join(&gltf_path).to_string_lossy().into_owned()
        } else {
            gltf_path
        };
        let label = format!("Scene{scene_index}");
        let full_path = format!("{resolved}#{label}");
        let scene_handle: Handle<Scene> = asset_server.load(full_path);
        world.entity_mut(entity).insert(SceneRoot(scene_handle));
    }
}

/// Returns indices such that every parent appears before its
/// children. JSN order isn't guaranteed parent-first because the
/// save side reads from a `HashSet`. O(N); cycle-tolerant by
/// treating the offending entry as a root.
fn topological_index_order(entities: &[jackdaw_jsn::format::JsnEntity]) -> Vec<usize> {
    let n = entities.len();
    let mut order = Vec::with_capacity(n);
    let mut emitted = vec![false; n];

    for start in 0..n {
        if emitted[start] {
            continue;
        }
        let mut chain: Vec<usize> = Vec::new();
        let mut cursor = start;
        let mut steps = 0usize;
        loop {
            if emitted[cursor] {
                break;
            }
            // Cycle guard: chain longer than `n` means a cycle.
            if steps > n {
                warn!(
                    "Topological order: cycle detected at JSN entity index {cursor}; \
                     remaining chain treated as roots"
                );
                break;
            }
            steps += 1;
            chain.push(cursor);
            match entities[cursor].parent {
                Some(p) if p < n && !emitted[p] => cursor = p,
                _ => break,
            }
        }
        for &i in chain.iter().rev() {
            order.push(i);
            emitted[i] = true;
        }
    }

    order
}

fn load_inline_assets(
    world: &mut World,
    assets: &JsnAssets,
    parent_path: &Path,
    catalog_assets: &HashMap<String, UntypedHandle>,
) -> HashMap<String, UntypedHandle> {
    let mut local_assets: HashMap<String, UntypedHandle> = HashMap::new();
    let linear_image_names = collect_linear_image_names(assets);
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry_guard = registry.read();
    let asset_server = world.resource::<AssetServer>().clone();

    for (type_path, named_entries) in &assets.0 {
        for (name, json_value) in named_entries {
            let serde_json::Value::String(rel_path) = json_value else {
                continue;
            };
            if rel_path.starts_with('@') {
                if let Some(handle) = catalog_assets.get(rel_path) {
                    local_assets.insert(name.clone(), handle.clone());
                } else {
                    warn!(
                        "Catalog asset '{rel_path}' referenced by '{name}' not found in project catalog"
                    );
                }
                continue;
            }

            let resolved = if Path::new(rel_path.as_str()).is_relative() {
                parent_path.join(rel_path).to_string_lossy().into_owned()
            } else {
                rel_path.clone()
            };

            let handle = if type_path == "bevy_image::image::Image" {
                if linear_image_names.contains(name) {
                    asset_server
                        .load_with_settings::<Image, ImageLoaderSettings>(
                            &resolved,
                            |s: &mut ImageLoaderSettings| s.is_srgb = false,
                        )
                        .untyped()
                } else {
                    asset_server.load::<Image>(&resolved).untyped()
                }
            } else {
                asset_server
                    .load::<bevy::asset::LoadedUntypedAsset>(&resolved)
                    .untyped()
            };
            local_assets.insert(name.clone(), handle);
        }
    }

    for (type_path, named_entries) in &assets.0 {
        let Some(registration) = registry_guard.get_with_type_path(type_path) else {
            warn!("Unknown asset type '{type_path}' in inline assets; skipping");
            continue;
        };
        let Some(reflect_asset) = registration.data::<ReflectAsset>() else {
            continue;
        };

        for (name, json_value) in named_entries {
            if json_value.is_string() {
                continue;
            }

            let mut processor = RuntimeDeserializerProcessor {
                asset_server: &asset_server,
                parent_path,
                local_assets: &local_assets,
                catalog_assets,
                entity_map: &[],
            };
            let deserializer = TypedReflectDeserializer::with_processor(
                registration,
                &registry_guard,
                &mut processor,
            );
            let Ok(reflected) = deserializer.deserialize(json_value) else {
                warn!("Failed to deserialize inline asset '{name}' of type '{type_path}'");
                continue;
            };

            let handle = reflect_asset.add(world, reflected.as_ref());
            local_assets.insert(name.clone(), handle);
        }
    }

    local_assets
}

/// Startup system: discover the project catalog and populate
/// [`JackdawCatalog`]. Honours [`JackdawCatalogPath`] if present;
/// otherwise mirrors Bevy's `FileAssetReader::get_base_path()` to
/// look for `.jsn/catalog.jsn` at the asset-root sibling.
fn load_project_catalog(world: &mut World) {
    let Some(catalog_path) = world
        .get_resource::<JackdawCatalogPath>()
        .map(|p| p.0.clone())
        .or_else(discover_catalog_path)
    else {
        return;
    };

    if !catalog_path.is_file() {
        info!(
            "No catalog at {}, skipping catalog load",
            catalog_path.display()
        );
        return;
    }

    let bytes = match std::fs::read(&catalog_path) {
        Ok(b) => b,
        Err(err) => {
            warn!("Failed to read catalog {}: {err}", catalog_path.display());
            return;
        }
    };
    let jsn_catalog: JsnCatalog = match serde_json::from_slice(&bytes) {
        Ok(c) => c,
        Err(err) => {
            warn!("Failed to parse catalog {}: {err}", catalog_path.display());
            return;
        }
    };

    apply_catalog(world, &jsn_catalog);
    let count = world.resource::<JackdawCatalog>().handles.len();
    info!(
        "Loaded project catalog with {count} entries from {}",
        catalog_path.display()
    );
}

/// Loads a `JsnCatalog`'s assets into the world and stores the
/// `@Name` handles in [`JackdawCatalog`]. Texture paths inside the
/// catalog are treated as asset-server-relative (same convention
/// as a scene file at the asset root).
fn apply_catalog(world: &mut World, jsn: &JsnCatalog) {
    // Catalog materials reference their own inline `#Name` images
    // only; no nested `@Name` lookups, so an empty external map
    // is correct here.
    let no_external = HashMap::new();
    let loaded = load_inline_assets(world, &jsn.assets, Path::new(""), &no_external);
    let handles: HashMap<String, UntypedHandle> = loaded
        .into_iter()
        .filter(|(name, _)| name.starts_with('@'))
        .collect();
    world.resource_mut::<JackdawCatalog>().handles = handles;
}

/// Mirrors `bevy::asset::io::file::FileAssetReader::get_base_path`
/// and returns the candidate catalog path. Falls back through
/// `BEVY_ASSET_ROOT`, `CARGO_MANIFEST_DIR`, and the executable's
/// directory. The catalog itself lives at `<base>/.jsn/catalog.jsn`,
/// sibling to the `assets/` folder Bevy reads from.
fn discover_catalog_path() -> Option<PathBuf> {
    let base = if let Ok(p) = std::env::var("BEVY_ASSET_ROOT") {
        PathBuf::from(p)
    } else if let Ok(p) = std::env::var("CARGO_MANIFEST_DIR") {
        PathBuf::from(p)
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(ToOwned::to_owned))?
    };
    Some(base.join(".jsn").join("catalog.jsn"))
}

fn collect_linear_image_names(assets: &JsnAssets) -> HashSet<String> {
    const LINEAR_SLOTS: &[&str] = &[
        "normal_map_texture",
        "metallic_roughness_texture",
        "occlusion_texture",
        "depth_map",
    ];
    let mut linear_names = HashSet::new();
    if let Some(materials) = assets.0.get("bevy_pbr::pbr_material::StandardMaterial") {
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

struct RuntimeDeserializerProcessor<'a> {
    asset_server: &'a AssetServer,
    parent_path: &'a Path,
    local_assets: &'a HashMap<String, UntypedHandle>,
    catalog_assets: &'a HashMap<String, UntypedHandle>,
    entity_map: &'a [Entity],
}

impl ReflectDeserializerProcessor for RuntimeDeserializerProcessor<'_> {
    fn try_deserialize<'de, D>(
        &mut self,
        registration: &TypeRegistration,
        _registry: &TypeRegistry,
        deserializer: D,
    ) -> Result<Result<Box<dyn PartialReflect>, D>, D::Error>
    where
        D: Deserializer<'de>,
    {
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

        if registration.data::<ReflectHandle>().is_some() {
            let path_str = deserializer.deserialize_any(StringOrNullVisitor)?;

            if path_str.is_empty()
                && let Some(rd) = registration.data::<ReflectDefault>()
            {
                return Ok(Ok(rd.default().into_partial_reflect()));
            }

            if path_str.starts_with('@') {
                if let Some(handle) = self.catalog_assets.get(&path_str) {
                    return Ok(Ok(Box::new(handle.clone()).into_partial_reflect()));
                }
                warn!("Catalog asset '{path_str}' not found in project catalog -- using default");
                if let Some(rd) = registration.data::<ReflectDefault>() {
                    return Ok(Ok(rd.default().into_partial_reflect()));
                }
            }

            if let Some(handle) = self.local_assets.get(&path_str) {
                return Ok(Ok(Box::new(handle.clone()).into_partial_reflect()));
            }

            let label_pos = path_str.find('#').unwrap_or(path_str.len());
            let file_part = &path_str[..label_pos];
            let label_part = &path_str[label_pos..];
            let resolved = if Path::new(file_part).is_relative() && !file_part.is_empty() {
                self.parent_path
                    .join(file_part)
                    .to_string_lossy()
                    .into_owned()
            } else {
                file_part.to_owned()
            };
            let handle = self
                .asset_server
                .load_untyped(format!("{resolved}{label_part}"));
            return Ok(Ok(Box::new(handle).into_partial_reflect()));
        }

        if registration.type_id() == TypeId::of::<Entity>() {
            let Ok(idx_str) = deserializer.deserialize_any(StringOrNullVisitor) else {
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

struct StringOrNullVisitor;

impl Visitor<'_> for StringOrNullVisitor {
    type Value = String;

    fn expecting(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "a string, integer, or null")
    }

    fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
        Ok(String::new())
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(v.to_owned())
    }

    fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
        Ok(v.to_string())
    }
}

struct F32Visitor;

impl Visitor<'_> for F32Visitor {
    type Value = f32;

    fn expecting(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "a number or float string (inf, -inf, NaN)")
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
        Ok(0.0)
    }
}

struct F64Visitor;

impl Visitor<'_> for F64Visitor {
    type Value = f64;

    fn expecting(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "a number or float string (inf, -inf, NaN)")
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
        Ok(0.0)
    }
}
