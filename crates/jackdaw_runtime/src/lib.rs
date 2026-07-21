use std::any::TypeId;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::asset::{AssetLoader, LoadContext, ReflectAsset, UntypedHandle, io::Reader};
use bevy::ecs::reflect::AppTypeRegistry;
#[cfg(feature = "render")]
use bevy::image::ImageLoaderSettings;
use bevy::prelude::*;
use bevy::reflect::TypeRegistry;
#[cfg(feature = "render")]
use bevy::world_serialization::{WorldAsset, WorldAssetRoot};
use jackdaw_bsn::{
    BsnApplyAssets, BsnPatch, BsnSceneAssets, BsnValue, SceneBsnAst, apply_component_patch,
    bsn_value_to_reflect, load_bsn_assets, parse_bsn_text,
};

pub use jackdaw_scene_types::{
    Brush, BrushFaceData, CustomProperties, EditorCategory, EditorDescription, EditorHidden,
    GltfSource, PropertyValue, SkipSerialization,
};

#[cfg(feature = "pie")]
mod pie;
#[cfg(feature = "pie")]
mod pie_frames;
#[cfg(feature = "pie")]
mod pie_windowless;
#[cfg(feature = "pie")]
pub use pie_windowless::{maybe_windowless, windowless_requested};

pub mod prelude {
    pub use crate::{
        EditorCategory, EditorDescription, EditorHidden, JackdawCatalog, JackdawCatalogPath,
        JackdawPlugin, JackdawSceneRoot, SkipSerialization,
    };
}

pub struct JackdawPlugin;

impl Plugin for JackdawPlugin {
    fn build(&self, app: &mut App) {
        // Registers every scene type for reflection and installs
        // `MeshRebuildPlugin` (which embeds the bundled grid texture
        // used as the brush fallback material).
        app.add_plugins(jackdaw_scene_types::SceneTypesPlugin {
            runtime_mesh_rebuild: true,
        });

        app.init_asset::<JackdawScene>()
            .init_asset_loader::<JackdawSceneLoader>()
            .init_resource::<JackdawCatalog>();

        app.add_systems(Startup, load_project_catalog);
        app.add_systems(
            Update,
            (clear_modified_scene_roots, spawn_loaded_scenes).chain(),
        );

        // Build avian colliders from authored `AvianCollider` components so
        // brushes collide at runtime. Add `PhysicsPlugins` in your app to run
        // the simulation.
        #[cfg(feature = "physics")]
        app.add_plugins(jackdaw_avian_integration::AvianColliderBridgePlugin);

        // When `JACKDAW_PIE` is set, open the ipc-channel link to the editor
        // and attach the PIE stream / control systems. A connect failure logs
        // and leaves the runtime untouched.
        #[cfg(feature = "pie")]
        if let Some(cfg) = pie::pie_config() {
            match jackdaw_pie_protocol::connect(&cfg.server) {
                Ok(transport) => pie::attach_pie(app, transport),
                Err(err) => bevy::log::error!("PIE connect failed: {err}"),
            }
        }
    }
}

/// Project-wide asset catalog. Maps `@Name` references found in
/// scene files to loaded `UntypedHandle`s.
///
/// Populated at startup from `assets/catalog.bsn` under the Bevy
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
/// `catalog.bsn` path instead of running the default discovery.
#[derive(Resource, Clone, Debug)]
pub struct JackdawCatalogPath(pub PathBuf);

/// A loaded `.bsn` scene, kept as its source text plus the origin metadata
/// the spawn loop needs. The document is parsed on spawn rather than stored,
/// since [`SceneBsnAst`] owns a private [`World`] and cannot be cloned out of
/// the asset store.
#[derive(Asset, TypePath)]
pub struct JackdawScene {
    bsn: String,
    parent_path: PathBuf,
    /// Source file stem (`starter` from `zones/starter.bsn`), captured at
    /// load time. Used to give the spawned scene root a readable `Name` so
    /// the editor's Live tree shows the scene name instead of an entity id.
    /// `None` when the asset was built without a source path.
    stem: Option<String>,
}

impl JackdawScene {
    /// Build a scene asset directly from in-memory `.bsn` text.
    /// Used by integration tests that drive scene-load codepaths
    /// without a real `.bsn` file on disk.
    pub fn new(bsn: String, parent_path: PathBuf) -> Self {
        Self {
            bsn,
            parent_path,
            stem: None,
        }
    }

    /// Like [`JackdawScene::new`] but with an explicit source file stem, so
    /// tests can exercise the root-naming path without a real `.bsn` file.
    pub fn with_stem(bsn: String, parent_path: PathBuf, stem: Option<String>) -> Self {
        Self {
            bsn,
            parent_path,
            stem,
        }
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

#[derive(TypePath, Default)]
struct JackdawSceneLoader;

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

        // Parse once at load so a malformed scene fails here with a clear
        // error rather than silently spawning nothing later. The document is
        // rebuilt from `bsn` on spawn (it owns a `World` and cannot be stored
        // in the asset).
        parse_bsn_text(text).map_err(|e| JackdawLoadError::Parse(e.to_string()))?;

        let source_path = load_context.path().path();
        let parent_path = source_path.parent().unwrap_or(Path::new("")).to_owned();
        let stem = source_path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned());

        Ok(JackdawScene {
            bsn: text.to_owned(),
            parent_path,
            stem,
        })
    }

    fn extensions(&self) -> &[&str] {
        &["bsn"]
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
/// hot reload of `assets/scene.bsn` in the standalone runner.
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
        let (bsn, parent_path, stem) = {
            let scenes = world.resource::<Assets<JackdawScene>>();
            let Some(scene) = scenes.get(&handle) else {
                continue;
            };
            (
                scene.bsn.clone(),
                scene.parent_path.clone(),
                scene.stem.clone(),
            )
        };

        let ast = match parse_bsn_text(&bsn) {
            Ok(ast) => ast,
            Err(err) => {
                warn!("Failed to parse scene .bsn: {err}");
                world.entity_mut(root_entity).insert(SceneSpawned);
                continue;
            }
        };

        spawn_scene_entities(world, root_entity, &ast, &parent_path);

        // Give the container root a readable name from the scene's file
        // stem so the editor's Live tree shows the scene name instead of an
        // entity id. Never overwrite an author-supplied name, and never
        // insert an empty one.
        if world.get::<Name>(root_entity).is_none()
            && let Some(stem) = stem.filter(|s| !s.is_empty())
        {
            world.entity_mut(root_entity).insert(Name::new(stem));
        }

        // Tag the container so the editor's outliner classifies it as a scene
        // root and shows the scene icon. The tag streams in the snapshot.
        if world
            .get::<jackdaw_scene_types::SceneRootTag>(root_entity)
            .is_none()
        {
            world
                .entity_mut(root_entity)
                .insert(jackdaw_scene_types::SceneRootTag);
        }

        world.entity_mut(root_entity).insert(SceneSpawned);
    }
}

/// Spawn a scene document's entities under `root_entity`.
///
/// Embedded named-asset roots load into their `Assets<T>` stores first (so
/// `#Name`/`@Name` references resolve), then entity roots spawn parent-first
/// by walking `roots` and each node's `Children` relation.
fn spawn_scene_entities(
    world: &mut World,
    root_entity: Entity,
    ast: &SceneBsnAst,
    parent_path: &Path,
) {
    let registry = world.resource::<AppTypeRegistry>().clone();

    // Load the linear-space textures a `StandardMaterial` references with
    // `is_srgb = false` before anything resolves their handles, so the
    // asset-server cache hands out the correctly-decoded image. Hold the
    // handles until the materials below take their own strong references.
    #[cfg(feature = "render")]
    let _preloaded_textures = preload_linear_textures(world, ast);

    // Embedded assets keyed as both `#Name` (scene-inline) and `@Name`
    // (catalog spelling), merged with the project catalog. Kept in
    // `BsnSceneAssets` so `apply_component_patch` resolves reference strings.
    let mut local_assets = load_embedded_assets(world, ast, &registry);
    for (name, handle) in world.resource::<JackdawCatalog>().handles.clone() {
        local_assets.entry(name).or_insert(handle);
    }
    let mut scene_assets = bevy::platform::collections::HashMap::default();
    for (name, handle) in &local_assets {
        scene_assets.insert(name.clone(), handle.clone());
    }
    world.insert_resource(BsnSceneAssets(scene_assets));

    let mut spawned: Vec<Entity> = Vec::new();
    for root in ast.roots.clone() {
        let is_asset = {
            let reg = registry.read();
            is_asset_root(ast, root, &reg)
        };
        if is_asset {
            continue;
        }
        spawn_node(world, ast, root, root_entity, &registry, &mut spawned);
    }

    #[cfg(feature = "render")]
    {
        let asset_server = world.resource::<AssetServer>().clone();
        let gltf_entities: Vec<(Entity, String, usize)> = spawned
            .iter()
            .filter_map(|&e| {
                world
                    .get::<jackdaw_scene_types::GltfSource>(e)
                    .map(|gs| (e, gs.path.clone(), gs.scene_index))
            })
            .collect();
        for (entity, gltf_path, scene_index) in gltf_entities {
            let resolved = if Path::new(&gltf_path).is_relative() {
                parent_path.join(&gltf_path).to_string_lossy().into_owned()
            } else {
                gltf_path
            };
            let full_path = format!("{resolved}#Scene{scene_index}");
            let scene_handle: Handle<WorldAsset> = asset_server.load(full_path);
            world
                .entity_mut(entity)
                .insert(WorldAssetRoot(scene_handle));
        }
    }
    #[cfg(not(feature = "render"))]
    let _ = parent_path;
}

/// Spawn one document node and its subtree.
///
/// `Transform` and `Visibility` are pulled into a single `world.spawn` along
/// with a `GlobalTransform`/`InheritedVisibility` computed from the parent's
/// already-final values, so the entity reaches its structural state in one
/// archetype move. `On<Insert, T>` observers for the remaining components then
/// see correct globals. Children are spawned parent-first via recursion.
///
/// Limitation: component fields of type `Entity` that reference another node
/// in the same scene are not remapped to the spawned entity. No built-in
/// component uses such a field today; a cross-entity reference feature must add
/// a post-spawn pass mapping document node order to spawned entities.
fn spawn_node(
    world: &mut World,
    ast: &SceneBsnAst,
    node: Entity,
    parent_entity: Entity,
    registry: &AppTypeRegistry,
    spawned: &mut Vec<Entity>,
) {
    let Some(patches) = ast.get_patches(node).map(|p| p.0.clone()) else {
        return;
    };
    if patches.is_empty() {
        // A document with no content parses to a single empty root; skip it
        // rather than spawn a phantom entity.
        return;
    }

    let mut name: Option<String> = None;
    let mut children: Vec<Entity> = Vec::new();
    let mut transform = Transform::default();
    let mut visibility = Visibility::default();
    let mut deferred: Vec<BsnPatch> = Vec::new();

    {
        let reg = registry.read();
        for &pe in &patches {
            let Some(patch) = ast.get_patch(pe) else {
                continue;
            };
            match patch {
                BsnPatch::Name(n) => name = Some(n.clone()),
                BsnPatch::Children(kids) => children = kids.clone(),
                BsnPatch::Base(_) | BsnPatch::Template(_, _) => {}
                BsnPatch::Type(_) | BsnPatch::Struct(_) | BsnPatch::TupleStruct(_) => {
                    let Some(type_path) = patch_type_path(patch) else {
                        continue;
                    };
                    match resolve_component_type_id(&reg, type_path) {
                        Some(id) if id == TypeId::of::<Transform>() => {
                            if let Some(t) = convert_component::<Transform>(patch, &reg) {
                                transform = t;
                            }
                        }
                        Some(id) if id == TypeId::of::<Visibility>() => {
                            if let Some(v) = convert_component::<Visibility>(patch, &reg) {
                                visibility = v;
                            }
                        }
                        _ => deferred.push(patch.clone()),
                    }
                }
            }
        }
    }

    // GT / IV from the parent's already-final values + local overrides.
    let parent_gt = world
        .get::<GlobalTransform>(parent_entity)
        .copied()
        .unwrap_or(GlobalTransform::IDENTITY);
    let computed_gt = parent_gt.mul_transform(transform);

    let parent_iv = world
        .get::<InheritedVisibility>(parent_entity)
        .copied()
        .unwrap_or(InheritedVisibility::VISIBLE);
    let computed_iv = match visibility {
        Visibility::Hidden => InheritedVisibility::HIDDEN,
        Visibility::Visible => InheritedVisibility::VISIBLE,
        Visibility::Inherited => parent_iv,
    };

    // One archetype move for all structural state.
    let entity = world
        .spawn((
            transform,
            visibility,
            computed_gt,
            computed_iv,
            ChildOf(parent_entity),
        ))
        .id();
    spawned.push(entity);

    if let Some(name) = name {
        world.entity_mut(entity).insert(Name::new(name));
    }

    // User components on top. `On<Insert, T>` fires here with
    // GlobalTransform / InheritedVisibility already correct. `SceneNodeId`
    // rides through this path as a normal registered tuple-struct component.
    for patch in &deferred {
        apply_component_patch(world, entity, patch);
    }

    for child in children {
        spawn_node(world, ast, child, entity, registry, spawned);
    }
}

/// The `BsnValue` form of a component patch (`Type`/`Struct`/`TupleStruct`),
/// or `None` for the relational/name patches that carry no component value.
fn patch_to_bsn_value(patch: &BsnPatch) -> Option<BsnValue> {
    match patch {
        BsnPatch::Type(tp) => Some(BsnValue::Type(tp.clone())),
        BsnPatch::Struct(data) => Some(BsnValue::Struct(data.clone())),
        BsnPatch::TupleStruct(data) => Some(BsnValue::TupleStruct(data.clone())),
        _ => None,
    }
}

/// The authored type path of a component patch, which may be enum-variant
/// qualified (`Enum::Variant`).
fn patch_type_path(patch: &BsnPatch) -> Option<&str> {
    match patch {
        BsnPatch::Type(tp) => Some(tp),
        BsnPatch::Struct(data) => Some(&data.type_path),
        BsnPatch::TupleStruct(data) => Some(&data.type_path),
        _ => None,
    }
}

/// The registered component's `TypeId` for a patch type path, resolving an
/// enum-variant path (`Enum::Variant`) back to its base enum registration.
fn resolve_component_type_id(reg: &TypeRegistry, type_path: &str) -> Option<TypeId> {
    let registration = reg.get_with_type_path(type_path).or_else(|| {
        type_path
            .rfind("::")
            .and_then(|sep| reg.get_with_type_path(&type_path[..sep]))
    })?;
    Some(registration.type_id())
}

/// Convert a component patch to a concrete `T` via the BSN document layer.
/// `assets` is `None`: the structural components this is used for (`Transform`,
/// `Visibility`) carry no asset references.
fn convert_component<T: bevy::reflect::FromReflect>(
    patch: &BsnPatch,
    reg: &TypeRegistry,
) -> Option<T> {
    let value = patch_to_bsn_value(patch)?;
    let reflected = bsn_value_to_reflect(&value, TypeId::of::<T>(), reg, None)?;
    <T as bevy::reflect::FromReflect>::from_reflect(reflected.as_ref())
}

/// The asset type path and value carried by a document root's component patch.
/// Mirrors the private helper in `jackdaw_bsn::catalog`.
fn asset_value_from_root(ast: &SceneBsnAst, root: Entity) -> Option<(String, BsnValue)> {
    let patches = ast.get_patches(root)?;
    for &pe in &patches.0 {
        match ast.get_patch(pe)? {
            BsnPatch::Struct(data) => {
                return Some((data.type_path.clone(), BsnValue::Struct(data.clone())));
            }
            BsnPatch::TupleStruct(data) => {
                return Some((data.type_path.clone(), BsnValue::TupleStruct(data.clone())));
            }
            BsnPatch::Type(tp) => return Some((tp.clone(), BsnValue::Type(tp.clone()))),
            _ => {}
        }
    }
    None
}

/// Whether a document root is a named asset entry (its component patch resolves
/// to a registered `Asset` type). Scene loading routes these into `Assets<T>`
/// stores instead of spawning them as entities.
fn is_asset_root(ast: &SceneBsnAst, root: Entity, reg: &TypeRegistry) -> bool {
    asset_value_from_root(ast, root)
        .and_then(|(type_path, _)| reg.get_with_type_path(&type_path))
        .is_some_and(|registration| registration.data::<ReflectAsset>().is_some())
}

/// Load a scene's embedded named assets into their `Assets<T>` stores.
/// Returns a map of `#Name` and `@Name` reference strings to handles.
fn load_embedded_assets(
    world: &mut World,
    ast: &SceneBsnAst,
    registry: &AppTypeRegistry,
) -> HashMap<String, UntypedHandle> {
    let mut map: HashMap<String, UntypedHandle> = HashMap::new();
    let server = world.resource::<AssetServer>().clone();

    for root in ast.roots.clone() {
        let reg = registry.read();
        let Some((type_path, asset_value)) = asset_value_from_root(ast, root) else {
            continue;
        };
        let Some(registration) = reg.get_with_type_path(&type_path) else {
            warn!(
                "embedded asset type '{type_path}' is not registered in this app; \
                 entities referencing it will fall back to a default handle"
            );
            continue;
        };
        let Some(reflect_asset) = registration.data::<ReflectAsset>() else {
            continue;
        };
        let type_id = registration.type_id();
        let Some(name) = ast.get_name(root).map(str::to_owned) else {
            continue;
        };
        let assets_ctx = BsnApplyAssets {
            server: &server,
            local: None,
        };
        let Some(value) = bsn_value_to_reflect(&asset_value, type_id, &reg, Some(&assets_ctx))
        else {
            continue;
        };
        let handle = reflect_asset.add(world, &*value);
        map.insert(format!("#{name}"), handle.clone());
        map.insert(format!("@{name}"), handle);
    }

    map
}

/// The asset-relative paths of every linear-space texture referenced by a
/// `StandardMaterial` patch in the document. These slots hold non-color data
/// (normals, ORM, height) and must be loaded without sRGB decoding.
#[cfg(feature = "render")]
fn collect_linear_texture_paths(ast: &SceneBsnAst) -> Vec<String> {
    const LINEAR_SLOTS: &[&str] = &[
        "normal_map_texture",
        "metallic_roughness_texture",
        "occlusion_texture",
        "depth_map",
    ];
    const STANDARD_MATERIAL: &str = "bevy_pbr::pbr_material::StandardMaterial";

    let mut paths = Vec::new();
    let mut stack: Vec<Entity> = ast.roots.clone();
    while let Some(node) = stack.pop() {
        let Some(patches) = ast.get_patches(node) else {
            continue;
        };
        for &pe in &patches.0 {
            match ast.get_patch(pe) {
                Some(BsnPatch::Struct(data)) if data.type_path == STANDARD_MATERIAL => {
                    for field in &data.fields.0 {
                        if LINEAR_SLOTS.contains(&field.name.as_str())
                            && let BsnValue::String(path) = &field.value
                            && !path.is_empty()
                        {
                            if path.starts_with('@') || path.starts_with('#') {
                                // A catalog / embedded reference, not a file path.
                                // Its underlying image is loaded elsewhere without
                                // `is_srgb = false`, so the linear-slot decode is
                                // still wrong; loading the ref string as a file
                                // would only add a bogus asset. Skip and flag it.
                                warn!(
                                    "linear-space texture '{path}' in field '{}' is a \
                                     catalog/embedded reference; it will decode as sRGB",
                                    field.name
                                );
                            } else {
                                paths.push(path.clone());
                            }
                        }
                    }
                }
                Some(BsnPatch::Children(kids)) => stack.extend(kids.iter().copied()),
                _ => {}
            }
        }
    }
    paths
}

/// Pre-load the document's linear-space material textures with `is_srgb =
/// false`. The asset server keys handles by path, so a later resolve of the
/// same path returns this correctly-decoded image. The returned handles keep
/// the assets alive until the materials take their own strong references.
#[cfg(feature = "render")]
fn preload_linear_textures(world: &mut World, ast: &SceneBsnAst) -> Vec<UntypedHandle> {
    let paths = collect_linear_texture_paths(ast);
    let mut handles = Vec::new();
    if paths.is_empty() {
        return handles;
    }
    let asset_server = world.resource::<AssetServer>().clone();
    for path in paths {
        let handle = asset_server
            .load_builder()
            .with_settings(|s: &mut ImageLoaderSettings| s.is_srgb = false)
            .load::<Image>(&path);
        handles.push(handle.untyped());
    }
    handles
}

/// Startup system: discover the project catalog and populate
/// [`JackdawCatalog`]. Honours [`JackdawCatalogPath`] if present;
/// otherwise mirrors Bevy's `FileAssetReader::get_base_path()` to
/// look for `assets/catalog.bsn` under the asset root.
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

    let text = match std::fs::read_to_string(&catalog_path) {
        Ok(t) => t,
        Err(err) => {
            warn!("Failed to read catalog {}: {err}", catalog_path.display());
            return;
        }
    };

    // Preload linear-space textures the catalog materials reference before
    // their handles resolve (see `preload_linear_textures`). The handles stay
    // alive until `load_bsn_assets` builds the materials that hold them.
    #[cfg(feature = "render")]
    let _preloaded_textures = parse_bsn_text(&text)
        .ok()
        .map(|ast| preload_linear_textures(world, &ast))
        .unwrap_or_default();

    match load_bsn_assets(world, &text) {
        Ok(entries) => {
            let count = entries.len();
            let mut catalog = world.resource_mut::<JackdawCatalog>();
            for entry in entries {
                // Scenes reference catalog assets as `@Name`.
                catalog
                    .handles
                    .insert(format!("@{}", entry.name), entry.handle);
            }
            info!(
                "Loaded project catalog with {count} entries from {}",
                catalog_path.display()
            );
        }
        Err(err) => warn!("Failed to parse catalog {}: {err}", catalog_path.display()),
    }
}

/// Mirrors `bevy::asset::io::file::FileAssetReader::get_base_path`
/// and returns the candidate catalog path. Falls back through
/// `BEVY_ASSET_ROOT`, `CARGO_MANIFEST_DIR`, and the executable's
/// directory. The catalog lives at `<base>/assets/catalog.bsn`,
/// inside the `assets/` folder Bevy reads from.
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
    Some(base.join("assets").join("catalog.bsn"))
}
