//! Load a scene authored in the editor, in a game that does not have the
//! editor.
//!
//! A game adds [`JackdawPlugin`], points a [`JackdawSceneRoot`] at a `.bsn`
//! file, and the scene's entities spawn under that root: transforms, brushes,
//! lights, cameras, UI canvases, glTF instances, and, under the features
//! below, terrain, colliders and a baked navmesh.
//!
//! ```ignore
//! App::new()
//!     .add_plugins((DefaultPlugins, jackdaw_runtime::JackdawPlugin))
//!     .add_systems(Startup, |mut commands: Commands, assets: Res<AssetServer>| {
//!         commands.spawn(JackdawSceneRoot(assets.load("scene.bsn")));
//!     })
//!     .run();
//! ```
//!
//! # What a game provides
//!
//! - **An asset folder**, as `AssetPlugin` was configured with: the `assets/`
//!   beside the executable unless the game said otherwise. Terrain sidecars
//!   and the catalog are read from it directly rather than through the asset
//!   server. Add [`JackdawPlugin`] *after* `DefaultPlugins` so it can see how
//!   `AssetPlugin` was configured. [`JackdawCatalogPath`] overrides the
//!   location for a game that keeps its project elsewhere.
//! - **A camera.** Scenes usually carry one. A game that spawns its own can
//!   mark it `TerrainViewer` so terrain lays its detail around the camera
//!   the player looks through rather than a UI overlay.
//!
//! No registration call, manifest or export step: the editor and the game
//! read the same files with the same code.
//!
//! # What it reads
//!
//! - `<scene>.bsn`: the scene, its entities and their components, plus inline
//!   `#Name` assets.
//! - `catalog.bsn` and `materials/<name>.material.bsn`: the project's named
//!   assets, loaded at startup into [`JackdawCatalog`], which an `@Name`
//!   reference in a scene resolves against.
//! - `<scene>.terrain-<n>.jdterrain`: one terrain's heights, control map,
//!   colors and material slots, named by the `Terrain` component's `data_path`
//!   relative to the scene. Read under the `terrain` feature.
//! - `<scene>.jdnav`: the navmesh baked for that scene. Read under the
//!   `navmesh` feature onto the scene root as a `JackdawNavmesh`.
//!
//! # Features
//!
//! - `render` (default): draw what the scene authored. A build without it does
//!   not compile, because `jackdaw_scene_types` registers its types through
//!   its own `render` feature.
//! - `terrain`: draw the authored terrain. Implies `render` and `navmesh`.
//! - `navmesh`: load the baked navmesh without drawing anything. A server that
//!   validates moves is `render` plus `navmesh` and no `terrain`, which leaves
//!   out the mesher, the splat material and the texture loads.
//! - `physics`: build avian colliders from authored `AvianCollider`
//!   components. The game adds `PhysicsPlugins` itself to simulate them. With
//!   `terrain` as well, the authored ground gets a heightfield collider built
//!   from the heights it is drawn from.
//! - `pie`: play-in-editor, streaming this world to a running editor.

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
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
use jackdaw_ui::UiCanvas;

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

#[cfg(feature = "terrain")]
mod terrain;
#[cfg(feature = "terrain")]
pub use terrain::TerrainViewer;

#[cfg(feature = "navmesh")]
mod navmesh;
#[cfg(feature = "navmesh")]
pub use navmesh::JackdawNavmesh;

mod schema_cli;
pub use schema_cli::{
    SCHEMA_FLAG, extract_schema_and_exit_if_requested, extract_schema_json,
    schema_extraction_requested,
};

pub mod prelude {
    #[cfg(feature = "navmesh")]
    pub use crate::JackdawNavmesh;
    #[cfg(feature = "terrain")]
    pub use crate::TerrainViewer;
    pub use crate::{
        EditorCategory, EditorDescription, EditorHidden, JackdawCatalog, JackdawCatalogPath,
        JackdawPlugin, JackdawSceneMember, JackdawSceneRoot, SkipSerialization,
    };
}

pub struct JackdawPlugin;

impl Plugin for JackdawPlugin {
    fn build(&self, app: &mut App) {
        // The editor asks for this binary's reflected types by launching
        // it with `--jackdaw-extract-schema`. Handle that here so every
        // game that adds `JackdawPlugin` answers that. Extraction reads
        // the link-time reflect inventory, not `app`, and exits before
        // `App::run` opens a window.
        schema_cli::extract_schema_and_exit_if_requested();

        // Sidecars and the catalog are read from the filesystem rather than
        // through the asset server, so the folder Bevy reads assets from is
        // recovered from the `AssetPlugin` this app added. It is only
        // readable while the app is being built, so it is captured here.
        app.insert_resource(AssetFolder(asset_folder(app)));

        // Registers every scene type for reflection and installs
        // `MeshRebuildPlugin` (which embeds the bundled grid texture
        // used as the brush fallback material).
        app.add_plugins(jackdaw_scene_types::SceneTypesPlugin {
            runtime_mesh_rebuild: true,
        });
        app.add_plugins(jackdaw_ui::JackdawUiPlugin::default());

        app.init_asset::<JackdawScene>()
            .init_asset_loader::<JackdawSceneLoader>()
            .init_resource::<JackdawCatalog>();

        app.add_systems(Startup, load_project_catalog);
        app.add_systems(
            Update,
            (
                clear_modified_scene_roots,
                spawn_loaded_scenes,
                cleanup_orphaned_scene_members,
            )
                .chain(),
        );

        #[cfg(feature = "render")]
        app.add_plugins(MaterialTextureFormatPlugin);

        #[cfg(feature = "terrain")]
        app.add_plugins(terrain::plugin);

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
#[require(Transform, Visibility, SceneInstanceMembers)]
pub struct JackdawSceneRoot(pub Handle<JackdawScene>);

/// Associates a top-level spawned entity with its scene instance independently
/// from the ECS hierarchy.
///
/// UI canvases must remain ECS roots for Bevy layout, so [`ChildOf`] cannot be
/// used as their ownership relation.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct JackdawSceneMember {
    /// The [`JackdawSceneRoot`] that owns this entity.
    pub root: Entity,
}

#[derive(Component, Default)]
struct SceneInstanceMembers(Vec<Entity>);

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
/// hot reload of `assets/scene.bsn` in the standalone game binary.
fn clear_modified_scene_roots(
    mut events: bevy::ecs::message::MessageReader<bevy::asset::AssetEvent<JackdawScene>>,
    roots: Query<(Entity, &JackdawSceneRoot, &SceneInstanceMembers), With<SceneSpawned>>,
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

    for (root_entity, root, members) in &roots {
        if !modified.contains(&root.0.id()) {
            continue;
        }
        for &member in &members.0 {
            commands.entity(member).despawn();
        }
        commands
            .entity(root_entity)
            .remove::<SceneSpawned>()
            .insert(SceneInstanceMembers::default());
    }
}

fn cleanup_orphaned_scene_members(
    members: Query<(Entity, &JackdawSceneMember)>,
    roots: Query<(), With<JackdawSceneRoot>>,
    mut commands: Commands,
) {
    for (entity, member) in &members {
        if roots.get(member.root).is_err() {
            commands.entity(entity).despawn();
        }
    }
}

pub(crate) fn spawn_loaded_scenes(
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

        let members = spawn_scene_entities(world, root_entity, &ast, &parent_path);
        world
            .entity_mut(root_entity)
            .insert(SceneInstanceMembers(members));

        // A bake is saved under the scene's own name, which is known here.
        #[cfg(feature = "navmesh")]
        navmesh::attach_navmesh(world, root_entity, &parent_path, stem.as_deref());

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
) -> Vec<Entity> {
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
    let mut members: Vec<Entity> = Vec::new();
    for root in ast.roots.clone() {
        let is_asset = {
            let reg = registry.read();
            is_asset_root(ast, root, &reg)
        };
        if is_asset {
            continue;
        }
        if let Some(member) = spawn_node(
            world,
            ast,
            root,
            root_entity,
            root_entity,
            true,
            &registry,
            &mut spawned,
        ) {
            members.push(member);
        }
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
    // A terrain's sidecar is named relative to the scene file, whose
    // directory is known here.
    #[cfg(feature = "terrain")]
    terrain::attach_sidecars(world, &spawned, parent_path);

    #[cfg(not(feature = "render"))]
    let _ = parent_path;

    members
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
    scene_root: Entity,
    is_document_root: bool,
    registry: &AppTypeRegistry,
    spawned: &mut Vec<Entity>,
) -> Option<Entity> {
    let patches = ast.get_patches(node).map(|p| p.0.clone())?;
    if patches.is_empty() {
        // A document with no content parses to a single empty root; skip it
        // rather than spawn a phantom entity.
        return None;
    }

    let mut name: Option<String> = None;
    let mut children: Vec<Entity> = Vec::new();
    let mut transform = Transform::default();
    let mut visibility = Visibility::default();
    let mut deferred: Vec<BsnPatch> = Vec::new();
    let mut is_ui_canvas = false;

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
                        Some(id) if id == TypeId::of::<UiCanvas>() => {
                            is_ui_canvas = true;
                            deferred.push(patch.clone());
                        }
                        _ => deferred.push(patch.clone()),
                    }
                }
            }
        }
    }

    // GT / IV from the parent's already-final values + local overrides.
    let unparented_canvas = is_document_root && is_ui_canvas;
    let parent_gt = if unparented_canvas {
        GlobalTransform::IDENTITY
    } else {
        world
            .get::<GlobalTransform>(parent_entity)
            .copied()
            .unwrap_or(GlobalTransform::IDENTITY)
    };
    let computed_gt = parent_gt.mul_transform(transform);

    let parent_iv = if unparented_canvas {
        InheritedVisibility::VISIBLE
    } else {
        world
            .get::<InheritedVisibility>(parent_entity)
            .copied()
            .unwrap_or(InheritedVisibility::VISIBLE)
    };
    let computed_iv = match visibility {
        Visibility::Hidden => InheritedVisibility::HIDDEN,
        Visibility::Visible => InheritedVisibility::VISIBLE,
        Visibility::Inherited => parent_iv,
    };

    // One archetype move for all structural state.
    let entity = world
        .spawn((transform, visibility, computed_gt, computed_iv))
        .id();
    if !unparented_canvas {
        world.entity_mut(entity).insert(ChildOf(parent_entity));
    }
    if is_document_root {
        world
            .entity_mut(entity)
            .insert(JackdawSceneMember { root: scene_root });
    }
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
        spawn_node(
            world, ast, child, entity, scene_root, false, registry, spawned,
        );
    }

    Some(entity)
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

/// The `Unorm` twin of a 16-bit `Uint` texture format: same bytes per texel,
/// same channel order, but float-filterable where the `Uint` side is not.
///
/// Bevy decodes 16-bit grayscale PNGs as `R16Uint` and grayscale+alpha as
/// `Rg16Uint`. A `StandardMaterial` slot demands a filterable float sampler,
/// so binding either one fails the whole bind group.
#[cfg(feature = "render")]
fn filterable_twin(
    format: bevy::render::render_resource::TextureFormat,
) -> Option<bevy::render::render_resource::TextureFormat> {
    use bevy::render::render_resource::TextureFormat;
    match format {
        TextureFormat::R16Uint => Some(TextureFormat::R16Unorm),
        TextureFormat::Rg16Uint => Some(TextureFormat::Rg16Unorm),
        TextureFormat::Rgba16Uint => Some(TextureFormat::Rgba16Unorm),
        _ => None,
    }
}

/// The images a `StandardMaterial` binds, across all of its texture slots.
#[cfg(feature = "render")]
fn material_texture_ids(
    material: &StandardMaterial,
) -> impl Iterator<Item = bevy::asset::AssetId<Image>> {
    [
        material.base_color_texture.as_ref(),
        material.emissive_texture.as_ref(),
        material.metallic_roughness_texture.as_ref(),
        material.normal_map_texture.as_ref(),
        material.occlusion_texture.as_ref(),
        material.depth_map.as_ref(),
    ]
    .into_iter()
    .flatten()
    .map(Handle::id)
}

/// Retags 16-bit `Uint` material textures as their filterable `Unorm` twins.
///
/// The editor adds this plugin as well, so a pack of 16-bit maps renders the
/// same in the editor and in the built game.
#[cfg(feature = "render")]
pub struct MaterialTextureFormatPlugin;

#[cfg(feature = "render")]
impl Plugin for MaterialTextureFormatPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            promote_material_texture_formats.after(bevy::asset::AssetEventSystems),
        );
    }
}

/// Retag every 16-bit `Uint` image a material binds as its `Unorm` twin.
///
/// Rewrites the descriptor only: the texels already have the layout the twin
/// declares. Runs after the asset events are published and before the render
/// world extracts, so a depth map is never bound in the failing format. Both
/// event streams are read, since an image may decode after the material naming
/// it or before it.
#[cfg(feature = "render")]
fn promote_material_texture_formats(
    mut image_events: MessageReader<AssetEvent<Image>>,
    mut material_events: MessageReader<AssetEvent<StandardMaterial>>,
    materials: Res<Assets<StandardMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    use bevy::asset::AssetId;

    let touched: Vec<AssetId<Image>> = image_events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Added { id }
            | AssetEvent::Modified { id }
            | AssetEvent::LoadedWithDependencies { id } => Some(*id),
            _ => None,
        })
        .collect();
    let materials_changed = material_events.read().any(|event| {
        matches!(
            event,
            AssetEvent::Added { .. } | AssetEvent::Modified { .. }
        )
    });
    if touched.is_empty() && !materials_changed {
        return;
    }

    let bound: HashSet<AssetId<Image>> = materials
        .iter()
        .flat_map(|(_, material)| material_texture_ids(material))
        .collect();
    let candidates: Vec<AssetId<Image>> = if materials_changed {
        bound.into_iter().collect()
    } else {
        touched
            .into_iter()
            .filter(|id| bound.contains(id))
            .collect()
    };

    for id in candidates {
        let Some(twin) = images
            .get(id)
            .and_then(|image| filterable_twin(image.texture_descriptor.format))
        else {
            continue;
        };
        if let Some(mut image) = images.get_mut(id) {
            image.texture_descriptor.format = twin;
        }
    }
}

/// Suffix of a saved material file under `assets/materials`. The stem before
/// it is the material's `@Name`.
const MATERIAL_FILE_SUFFIX: &str = ".material.bsn";

/// Startup system: discover the project catalog and populate
/// [`JackdawCatalog`]. Honours [`JackdawCatalogPath`] if present;
/// otherwise mirrors Bevy's `FileAssetReader::get_base_path()` to
/// look for `assets/catalog.bsn` under the asset root.
///
/// Materials are one file each under `assets/materials`; the catalog file
/// holds the remaining named assets. Both feed the same `@Name` map.
fn load_project_catalog(world: &mut World) {
    let root = assets_root(world);
    let Some(catalog_path) = world
        .get_resource::<JackdawCatalogPath>()
        .map(|p| p.0.clone())
        .or_else(|| root.as_ref().map(|root| root.join("catalog.bsn")))
    else {
        return;
    };

    // Materials first, so their linear-space textures claim their paths at the
    // right color space and a saved material wins its name over any inline
    // entry in the catalog file.
    if let Some(root) = &root {
        load_material_files(world, &root.join("materials"));
    }

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
                // Scenes reference catalog assets as `@Name`. A material
                // file already holding the name wins.
                catalog
                    .handles
                    .entry(format!("@{}", entry.name))
                    .or_insert(entry.handle);
            }
            info!(
                "Loaded project catalog with {count} entries from {}",
                catalog_path.display()
            );
        }
        Err(err) => warn!("Failed to parse catalog {}: {err}", catalog_path.display()),
    }
}

/// Load every `assets/materials/*.material.bsn` into [`JackdawCatalog`] under
/// its file stem. A file that fails to read or parse is logged and skipped.
fn load_material_files(world: &mut World, dir: &Path) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = read_dir
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(MATERIAL_FILE_SUFFIX))
        })
        .collect();
    files.sort();

    let mut count = 0usize;
    for path in files {
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(MATERIAL_FILE_SUFFIX))
            .map(str::to_owned)
        else {
            continue;
        };
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                warn!("Failed to read material {}: {err}", path.display());
                continue;
            }
        };

        #[cfg(feature = "render")]
        let _preloaded_textures = parse_bsn_text(&text)
            .ok()
            .map(|ast| preload_linear_textures(world, &ast))
            .unwrap_or_default();

        match load_bsn_assets(world, &text) {
            Ok(entries) => {
                if entries.len() > 1 {
                    warn!(
                        "{} holds {} assets; only the first is used",
                        path.display(),
                        entries.len()
                    );
                }
                let Some(entry) = entries.into_iter().next() else {
                    continue;
                };
                if entry.handle.type_id() != TypeId::of::<StandardMaterial>() {
                    warn!("{} is not a StandardMaterial", path.display());
                    continue;
                }
                world
                    .resource_mut::<JackdawCatalog>()
                    .handles
                    .insert(format!("@{name}"), entry.handle);
                count += 1;
            }
            Err(err) => warn!("Failed to parse material {}: {err}", path.display()),
        }
    }
    if count > 0 {
        info!("Loaded {count} saved materials from {}", dir.display());
    }
}

/// The folder Bevy reads assets from, as this app's [`AssetPlugin`] was
/// configured. Captured when [`JackdawPlugin`] is built.
#[derive(Resource, Clone, Debug)]
struct AssetFolder(Option<PathBuf>);

/// The asset root every sidecar and catalog path is resolved against: the
/// directory [`JackdawCatalogPath`] points into when it is set, else the
/// folder the app's [`AssetPlugin`] reads from.
pub(crate) fn assets_root(world: &World) -> Option<PathBuf> {
    if let Some(path) = world.get_resource::<JackdawCatalogPath>() {
        return path.0.parent().map(ToOwned::to_owned);
    }
    world
        .get_resource::<AssetFolder>()
        .and_then(|f| f.0.clone())
}

/// Where the app reads assets from: [`AssetPlugin::file_path`] resolved the
/// way `bevy::asset::io::file::FileAssetReader` resolves it, against
/// `BEVY_ASSET_ROOT`, `CARGO_MANIFEST_DIR`, or the executable's directory.
///
/// With no `AssetPlugin` added yet, the default folder stands in.
fn asset_folder(app: &App) -> Option<PathBuf> {
    let file_path = app
        .get_added_plugins::<AssetPlugin>()
        .first()
        .map_or_else(|| AssetPlugin::default().file_path, |p| p.file_path.clone());
    let base = if let Ok(p) = std::env::var("BEVY_ASSET_ROOT") {
        PathBuf::from(p)
    } else if let Ok(p) = std::env::var("CARGO_MANIFEST_DIR") {
        PathBuf::from(p)
    } else {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(ToOwned::to_owned))?
    };
    Some(base.join(file_path))
}

#[cfg(test)]
mod material_file_tests {
    use super::*;
    use bevy::app::App;
    use bevy::asset::{AssetApp, AssetPlugin};

    fn runtime_app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_asset::<StandardMaterial>();
        app.register_asset_reflect::<Image>();
        app.register_asset_reflect::<StandardMaterial>();
        app.init_resource::<JackdawCatalog>();
        app
    }

    const GRASS: &str =
        "#grass\nbevy_pbr::pbr_material::StandardMaterial {\n    perceptual_roughness: 0.25,\n}\n";

    #[test]
    fn a_missing_materials_directory_is_not_an_error() {
        let mut app = runtime_app();
        let tmp = tempfile::tempdir().expect("tempdir");
        load_material_files(app.world_mut(), &tmp.path().join("materials"));
        assert!(app.world().resource::<JackdawCatalog>().is_empty());
    }

    #[test]
    fn material_files_populate_the_catalog_under_their_stem() {
        let mut app = runtime_app();
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("materials");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("grass.material.bsn"), GRASS).expect("write");
        std::fs::write(dir.join("notes.txt"), "ignored").expect("write");
        // A material file must hold a material; anything else is rejected.
        std::fs::write(
            dir.join("wrong.material.bsn"),
            "#wrong\nbevy_image::image::Image\n",
        )
        .expect("write");

        load_material_files(app.world_mut(), &dir);

        assert_eq!(
            app.world().resource::<Assets<Image>>().len(),
            1,
            "the non-material file must have parsed, so the rejection is the type check"
        );
        let catalog = app.world().resource::<JackdawCatalog>();
        assert_eq!(catalog.len(), 1);
        assert!(catalog.get("@grass").is_some());
        assert!(catalog.get("@wrong").is_none());
    }

    #[test]
    fn a_material_file_outranks_an_inline_catalog_entry_of_the_same_name() {
        let mut app = runtime_app();
        let tmp = tempfile::tempdir().expect("tempdir");
        let dir = tmp.path().join("materials");
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join("grass.material.bsn"), GRASS).expect("write");

        load_material_files(app.world_mut(), &dir);
        let from_file = app
            .world()
            .resource::<JackdawCatalog>()
            .get("@grass")
            .expect("file entry")
            .clone();

        // The catalog file is read second and must not displace it.
        let inline = "#grass\nbevy_pbr::pbr_material::StandardMaterial {\n    perceptual_roughness: 0.9,\n}\n";
        let entries = load_bsn_assets(app.world_mut(), inline).expect("parse");
        let mut catalog = app.world_mut().resource_mut::<JackdawCatalog>();
        for entry in entries {
            catalog
                .handles
                .entry(format!("@{}", entry.name))
                .or_insert(entry.handle);
        }

        assert_eq!(
            app.world()
                .resource::<JackdawCatalog>()
                .get("@grass")
                .map(UntypedHandle::id),
            Some(from_file.id())
        );
    }
}

/// The `Uint` retag, exercised through the plugin the editor and a built game
/// both add.
#[cfg(all(test, feature = "render"))]
mod material_texture_format_tests {
    use super::*;
    use bevy::app::App;
    use bevy::asset::{AssetApp, AssetPlugin, RenderAssetUsages};
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    fn promotion_app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_asset::<StandardMaterial>();
        app.add_plugins(MaterialTextureFormatPlugin);
        app
    }

    /// A one-texel image in `format`, sized from the texel it is given.
    fn raw_image(app: &mut App, texel: &[u8], format: TextureFormat) -> Handle<Image> {
        let image = Image::new(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            texel.to_vec(),
            format,
            RenderAssetUsages::default(),
        );
        app.world_mut().resource_mut::<Assets<Image>>().add(image)
    }

    fn format_of(app: &App, handle: &Handle<Image>) -> TextureFormat {
        app.world()
            .resource::<Assets<Image>>()
            .get(handle)
            .expect("image")
            .texture_descriptor
            .format
    }

    /// A 16-bit grayscale PNG decodes as `R16Uint`, which has no filterable
    /// sampler; bound to a material slot it fails the whole bind group.
    #[test]
    fn a_sixteen_bit_uint_image_a_material_binds_is_retagged_as_its_unorm_twin() {
        let mut app = promotion_app();
        let occlusion = raw_image(&mut app, &[0x00, 0x80], TextureFormat::R16Uint);
        let gray_alpha = raw_image(&mut app, &[0x00, 0x80, 0x00, 0xff], TextureFormat::Rg16Uint);
        let _material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                occlusion_texture: Some(occlusion.clone()),
                depth_map: Some(gray_alpha.clone()),
                ..default()
            });

        app.update();

        assert_eq!(format_of(&app, &occlusion), TextureFormat::R16Unorm);
        assert_eq!(format_of(&app, &gray_alpha), TextureFormat::Rg16Unorm);
    }

    /// The texels are untouched; only the descriptor is rewritten.
    #[test]
    fn promotion_leaves_the_texels_alone() {
        let mut app = promotion_app();
        let image = raw_image(&mut app, &[0x34, 0x12], TextureFormat::R16Uint);
        let _material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                depth_map: Some(image.clone()),
                ..default()
            });

        app.update();

        let images = app.world().resource::<Assets<Image>>();
        assert_eq!(
            images.get(&image).unwrap().data.as_deref(),
            Some(&[0x34u8, 0x12][..])
        );
    }

    /// An image no material binds keeps its format: an integer texture read
    /// with `textureLoad` stays integer.
    #[test]
    fn an_image_no_material_binds_keeps_its_uint_format() {
        let mut app = promotion_app();
        let unbound = raw_image(&mut app, &[0x00, 0x80], TextureFormat::R16Uint);

        app.update();

        assert_eq!(format_of(&app, &unbound), TextureFormat::R16Uint);
    }

    /// The image may decode before the material that names it, so the
    /// material's own event sweeps the slots it claims.
    #[test]
    fn a_material_added_after_its_image_still_gets_it_promoted() {
        let mut app = promotion_app();
        let image = raw_image(&mut app, &[0x00, 0x80], TextureFormat::R16Uint);
        app.update();
        assert_eq!(format_of(&app, &image), TextureFormat::R16Uint);

        let _material = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                normal_map_texture: Some(image.clone()),
                ..default()
            });
        app.update();

        assert_eq!(format_of(&app, &image), TextureFormat::R16Unorm);
    }
}
