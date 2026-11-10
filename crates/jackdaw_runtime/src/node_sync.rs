//! Applies authored component values from the editor's document to the
//! entities spawned from each scene node.
//!
//! [`SceneNodeIndex`] maps every [`SceneNodeId`] to the entities that carry
//! it, kept current by observers on the component. [`apply_node_components`]
//! writes a node's changed components to each of those entities through
//! reflection.
//!
//! While a scene root has yet to spawn, every change is also held in
//! [`PendingNodeChanges`] and applied to the entities that spawn, after the
//! scene has finished inserting its components. The document a root spawns from
//! may be older than a change that reached the game first. A value naming an
//! asset nothing has loaded yet is tried again on the entities it missed, alone,
//! since the root may bring the asset. With no root waiting the held changes are
//! dropped: a scene that spawns later asks for the document as it stands then.
//!
//! [`load_scene_assets`] takes the asset definitions the editor sends ahead of an
//! edit naming an asset this game has not been given, and [`load_asset_files`]
//! the asset files the editor sends by path. Either writes a new definition into
//! the handle already held, so everything using the asset updates.

use bevy::asset::{AssetServer, ReflectHandle, UntypedHandle};
use bevy::ecs::component::ComponentId;
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::serde::{ReflectDeserializerProcessor, TypedReflectDeserializer};
use bevy::reflect::{PartialReflect, ReflectFromReflect, TypeRegistration, TypeRegistry};
use jackdaw_bsn::BsnSceneAssets;
use jackdaw_scene_types::SceneNodeId;

use crate::JackdawCatalog;
use serde::Deserialize;
use serde::de::DeserializeSeed;

/// The entities spawned from each scene node.
#[derive(Resource, Default, Debug)]
pub struct SceneNodeIndex(HashMap<SceneNodeId, Vec<Entity>>);

impl SceneNodeIndex {
    /// The entities carrying `node`, in the order they gained it.
    pub fn entities(&self, node: SceneNodeId) -> &[Entity] {
        self.0.get(&node).map_or(&[], Vec::as_slice)
    }

    fn add(&mut self, node: SceneNodeId, entity: Entity) {
        let entities = self.0.entry(node).or_default();
        if !entities.contains(&entity) {
            entities.push(entity);
        }
    }

    fn remove(&mut self, node: SceneNodeId, entity: Entity) {
        if let Some(entities) = self.0.get_mut(&node) {
            entities.retain(|&held| held != entity);
            if entities.is_empty() {
                self.0.remove(&node);
            }
        }
    }
}

/// Type paths a node sync already warned about, so each warns once.
#[derive(Resource, Default)]
struct NodeSyncWarnings(HashSet<String>);

/// Node changes that arrived while a scene root had yet to spawn, by node.
#[derive(Resource, Default)]
pub struct PendingNodeChanges {
    nodes: HashMap<u64, PendingNode>,
}

impl PendingNodeChanges {
    /// Drop what is held for `node`: text newer than it is on its way.
    pub(crate) fn forget(&mut self, node: u64) {
        self.nodes.remove(&node);
    }

    /// Whether a change for `node` is held.
    pub fn contains(&self, node: u64) -> bool {
        self.nodes.contains_key(&node)
    }

    /// How many nodes have a change held.
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// Whether no change is held.
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// Merge a change into what `node` holds: a value replaces an earlier
    /// value or removal of its type, and a removal replaces an earlier value.
    /// `delivered` are the entities the change is being applied to directly.
    /// `missed` are the types in `set` whose value named an asset nothing has
    /// loaded yet, so those entities take that value again once something has.
    fn hold(
        &mut self,
        node: u64,
        set: &[(String, serde_json::Value)],
        removed: &[String],
        delivered: &[Entity],
        missed: &[String],
    ) {
        let held = self.nodes.entry(node).or_default();
        held.delivered.extend(delivered.iter().copied());
        for (type_path, value) in set {
            held.missed.remove(type_path);
            held.removed.retain(|path| path != type_path);
            match held.set.iter_mut().find(|(path, _)| path == type_path) {
                Some((_, held_value)) => *held_value = value.clone(),
                None => held.set.push((type_path.clone(), value.clone())),
            }
        }
        for type_path in missed {
            held.missed
                .entry(type_path.clone())
                .or_default()
                .extend(delivered.iter().copied());
        }
        for type_path in removed {
            held.missed.remove(type_path);
            held.set.retain(|(path, _)| path != type_path);
            if !held.removed.contains(type_path) {
                held.removed.push(type_path.clone());
            }
        }
    }
}

#[derive(Default)]
struct PendingNode {
    set: Vec<(String, serde_json::Value)>,
    removed: Vec<String>,
    /// Entities that already have what is held. A root spawned earlier keeps
    /// whatever the game has done to them since.
    delivered: HashSet<Entity>,
    /// The entities still owed the held value of a type, which named an asset
    /// nothing has loaded yet.
    missed: HashMap<String, HashSet<Entity>>,
}

/// Install [`SceneNodeIndex`] and the observers that keep it current, indexing
/// the entities that already carry a [`SceneNodeId`].
pub fn install_node_index(app: &mut App) {
    app.init_resource::<SceneNodeIndex>();
    app.init_resource::<NodeSyncWarnings>();
    app.init_resource::<PendingNodeChanges>();
    app.add_observer(index_node_insert);
    app.add_observer(index_node_replace);
    app.init_resource::<crate::node_structure::HeldStructure>();
    app.add_systems(
        Update,
        (
            crate::node_structure::replay_held_structure,
            apply_pending_node_changes,
        )
            .chain()
            .after(crate::spawn_loaded_scenes),
    );
    let world = app.world_mut();
    let present: Vec<(Entity, SceneNodeId)> = world
        .query::<(Entity, &SceneNodeId)>()
        .iter(world)
        .map(|(entity, node)| (entity, *node))
        .collect();
    let mut index = world.resource_mut::<SceneNodeIndex>();
    for (entity, node) in present {
        index.add(node, entity);
    }
}

fn index_node_insert(
    insert: On<Insert<SceneNodeId>>,
    nodes: Query<&SceneNodeId>,
    mut index: ResMut<SceneNodeIndex>,
) {
    if let Ok(node) = nodes.get(insert.entity) {
        index.add(*node, insert.entity);
    }
}

/// Apply held changes to the entities that spawned since they were held, and
/// drop them once no scene root is waiting. Runs after scenes spawn, so a held
/// value is not overwritten by a component the spawn inserts later.
fn apply_pending_node_changes(world: &mut World) {
    if world.resource::<PendingNodeChanges>().is_empty() {
        return;
    }
    let nodes: Vec<u64> = world
        .resource::<PendingNodeChanges>()
        .nodes
        .keys()
        .copied()
        .collect();
    for node in nodes {
        let entities: Vec<Entity> = world
            .resource::<SceneNodeIndex>()
            .entities(SceneNodeId(node))
            .to_vec();
        let mut pending = world.resource_mut::<PendingNodeChanges>();
        let Some(held) = pending.nodes.get_mut(&node) else {
            continue;
        };
        let fresh: Vec<Entity> = entities
            .into_iter()
            .filter(|entity| !held.delivered.contains(entity))
            .collect();
        held.delivered.extend(fresh.iter().copied());
        let (set, removed) = (held.set.clone(), held.removed.clone());
        retry_missed_values(world, node);
        let missed = apply_to_entities(world, &fresh, &set, &removed, true);
        if let Some(held) = world
            .resource_mut::<PendingNodeChanges>()
            .nodes
            .get_mut(&node)
        {
            for type_path in missed {
                held.missed
                    .entry(type_path)
                    .or_default()
                    .extend(fresh.iter().copied());
            }
        }
    }
    if !crate::scene_root_awaiting_spawn(world) {
        let nodes = std::mem::take(&mut world.resource_mut::<PendingNodeChanges>().nodes);
        for held in nodes.into_values() {
            for type_path in held.missed.into_keys() {
                warn_once(
                    world,
                    &type_path,
                    &format!("`{type_path}`: it names an asset this game never loaded"),
                );
            }
        }
    }
}

/// Write each value `node` still owes an entity if the asset it names has loaded.
fn retry_missed_values(world: &mut World, node: u64) {
    let Some(held) = world.resource::<PendingNodeChanges>().nodes.get(&node) else {
        return;
    };
    let owed: Vec<(String, serde_json::Value, Vec<Entity>)> = held
        .missed
        .iter()
        .filter_map(|(type_path, entities)| {
            let (_, value) = held.set.iter().find(|(path, _)| path == type_path)?;
            Some((
                type_path.clone(),
                value.clone(),
                entities.iter().copied().collect(),
            ))
        })
        .collect();
    for (type_path, value, entities) in owed {
        match decode_component(world, &type_path, &value) {
            Err(DecodeFailure::UnresolvedAsset(_)) => continue,
            Err(DecodeFailure::Other(reason)) => warn_once(world, &type_path, &reason),
            Ok(decoded) => {
                for entity in entities {
                    write_component(world, entity, &decoded);
                }
            }
        }
        if let Some(held) = world
            .resource_mut::<PendingNodeChanges>()
            .nodes
            .get_mut(&node)
        {
            held.missed.remove(&type_path);
        }
    }
}

fn index_node_replace(
    replace: On<Discard<SceneNodeId>>,
    nodes: Query<&SceneNodeId>,
    mut index: ResMut<SceneNodeIndex>,
) {
    if let Ok(node) = nodes.get(replace.entity) {
        index.remove(*node, replace.entity);
    }
}

/// Reads an asset handle as the reference the editor sent: a `#Name` or
/// `@Name` the loaded scene or the project catalog answers to, else an asset
/// path loaded through the asset server. `null` reads as the handle type's
/// default. A `#Name` or `@Name` nothing answers to is refused, and so is a path
/// to a document the catalog does not hold, since only a scene has a loader for
/// one and the editor sends any other by path ahead of the edit naming it.
struct HandleReferenceProcessor<'a> {
    server: Option<&'a AssetServer>,
    scene: Option<&'a BsnSceneAssets>,
    catalog: Option<&'a JackdawCatalog>,
    /// Whether a reference was refused.
    refused: bool,
}

impl HandleReferenceProcessor<'_> {
    fn named(&self, reference: &str, asset_type: std::any::TypeId) -> Option<UntypedHandle> {
        self.scene
            .and_then(|scene| scene.0.get(reference))
            .or_else(|| self.catalog.and_then(|catalog| catalog.get(reference)))
            .filter(|handle| handle.type_id() == asset_type)
            .cloned()
    }
}

impl ReflectDeserializerProcessor for HandleReferenceProcessor<'_> {
    fn try_deserialize<'de, D>(
        &mut self,
        registration: &TypeRegistration,
        _registry: &TypeRegistry,
        deserializer: D,
    ) -> Result<Result<Box<dyn PartialReflect>, D>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let Some(reflect_handle) = registration.data::<ReflectHandle>() else {
            return Ok(Err(deserializer));
        };
        let asset_type = reflect_handle.asset_type_id();
        let untyped = match Option::<String>::deserialize(deserializer)? {
            None => UntypedHandle::default_for_type(asset_type),
            Some(reference) => match self.named(&reference, asset_type) {
                Some(handle) => handle,
                None if refused_when_unresolved(&reference, asset_type) => {
                    self.refused = true;
                    return Err(serde::de::Error::custom(format!(
                        "the asset reference `{reference}` names nothing this game loaded"
                    )));
                }
                None => match self.server {
                    Some(server) => server.load_builder().load_erased(asset_type, reference),
                    None => UntypedHandle::default_for_type(asset_type),
                },
            },
        };
        Ok(Ok(reflect_handle.typed(untyped).into_partial_reflect()))
    }
}

/// Whether nothing but a loaded asset can answer to `reference`.
fn refused_when_unresolved(reference: &str, asset_type: std::any::TypeId) -> bool {
    reference.starts_with('#')
        || reference.starts_with('@')
        || (jackdaw_bsn::is_document_path(std::path::Path::new(reference))
            && asset_type != std::any::TypeId::of::<crate::JackdawScene>())
}

/// Why a component value could not be read.
pub(crate) enum DecodeFailure {
    /// It names an asset nothing has loaded, which a later load can fix.
    UnresolvedAsset(String),
    /// Anything else, which trying again cannot fix.
    Other(String),
}

impl std::fmt::Display for DecodeFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnresolvedAsset(reason) | Self::Other(reason) => formatter.write_str(reason),
        }
    }
}

/// A component value read from JSON, with the reflection needed to write it.
pub(crate) struct DecodedComponent {
    pub(crate) reflect: ReflectComponent,
    pub(crate) value: Box<dyn PartialReflect>,
    type_id: std::any::TypeId,
    from_reflect: Option<ReflectFromReflect>,
}

/// Deserialize `value` as the component `type_path` names. Asset handles are
/// read as the references [`HandleReferenceProcessor`] resolves.
pub(crate) fn decode_component(
    world: &World,
    type_path: &str,
    value: &serde_json::Value,
) -> Result<DecodedComponent, DecodeFailure> {
    let registry = world.resource::<AppTypeRegistry>().read();
    let registration = registry
        .get_with_type_path(type_path)
        .ok_or_else(|| DecodeFailure::Other(format!("unknown type path `{type_path}`")))?;
    let reflect = registration
        .data::<ReflectComponent>()
        .ok_or_else(|| DecodeFailure::Other(format!("`{type_path}` has no ReflectComponent")))?
        .clone();
    let mut processor = HandleReferenceProcessor {
        server: world.get_resource::<AssetServer>(),
        scene: world.get_resource::<BsnSceneAssets>(),
        catalog: world.get_resource::<JackdawCatalog>(),
        refused: false,
    };
    let value = TypedReflectDeserializer::with_processor(registration, &registry, &mut processor)
        .deserialize(value)
        .map_err(|err| {
            let reason = format!("failed to deserialize `{type_path}`: {err}");
            if processor.refused {
                DecodeFailure::UnresolvedAsset(reason)
            } else {
                DecodeFailure::Other(reason)
            }
        })?;
    let from_reflect = registration.data::<ReflectFromReflect>().cloned();
    let value = match &from_reflect {
        Some(from_reflect) => from_reflect
            .from_reflect(value.as_ref())
            .ok_or_else(|| {
                DecodeFailure::Other(format!("`{type_path}` cannot be built from the value sent"))
            })?
            .into_partial_reflect(),
        None => value,
    };
    Ok(DecodedComponent {
        reflect,
        value,
        type_id: registration.type_id(),
        from_reflect,
    })
}

/// The reflection and component id of a removed type.
struct Removal {
    type_path: String,
    reflect: ReflectComponent,
    id: Option<ComponentId>,
}

fn removal_for(world: &World, type_path: &str) -> Result<Removal, String> {
    let registry = world.resource::<AppTypeRegistry>().read();
    let registration = registry
        .get_with_type_path(type_path)
        .ok_or_else(|| format!("unknown type path `{type_path}`"))?;
    let reflect = registration
        .data::<ReflectComponent>()
        .cloned()
        .ok_or_else(|| format!("`{type_path}` has no ReflectComponent"))?;
    Ok(Removal {
        type_path: type_path.to_string(),
        reflect,
        id: world.components().get_id(registration.type_id()),
    })
}

/// Whether a component `entity` holds, other than those in `leaving`,
/// requires the component `required`.
fn required_on(
    world: &World,
    entity: Entity,
    required: ComponentId,
    leaving: &[ComponentId],
) -> bool {
    let components = world.components();
    let Ok(entity_ref) = world.get_entity(entity) else {
        return false;
    };
    entity_ref.archetype().components().iter().any(|&held| {
        held != required
            && !leaving.contains(&held)
            && components.get_info(held).is_some_and(|info| {
                info.required_components()
                    .iter_ids()
                    .any(|id| id == required)
            })
    })
}

/// Write `decoded` onto `entity` as its whole new value.
///
/// A mutable component the entity holds has its value replaced in place: no
/// hooks or observers run, and the component reads as changed. A reflective
/// apply is not used because it only overwrites and appends list elements, so
/// a shorter list would keep its stale tail. An immutable component cannot
/// be changed in place and is re-inserted, which runs its replace and insert
/// hooks and observers. A component the entity lacks is inserted.
pub(crate) fn write_component(world: &mut World, entity: Entity, decoded: &DecodedComponent) {
    let Ok(entity_ref) = world.get_entity(entity) else {
        return;
    };
    let present = decoded.reflect.contains(entity_ref);
    let mutable = world
        .components()
        .get_id(decoded.type_id)
        .and_then(|id| world.components().get_info(id))
        .is_none_or(bevy::ecs::component::ComponentInfo::mutable);
    let replacement = decoded
        .from_reflect
        .as_ref()
        .and_then(|from_reflect| from_reflect.from_reflect(decoded.value.as_ref()));
    if present
        && mutable
        && let Some(replacement) = replacement
        && let Some(mut current) = decoded.reflect.reflect_mut(world.entity_mut(entity))
    {
        if let Err(refused) = current.set(replacement) {
            warn!(
                "PIE: node sync could not replace `{}`",
                refused.reflect_type_path()
            );
        }
        return;
    }
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let mut entity_mut = world.entity_mut(entity);
    decoded
        .reflect
        .insert(&mut entity_mut, decoded.value.as_ref(), &registry);
}

fn warn_once(world: &mut World, type_path: &str, reason: &str) {
    let mut warnings = world.get_resource_or_init::<NodeSyncWarnings>();
    if warnings.0.insert(type_path.to_string()) {
        warn!("PIE: node sync skipped {reason}");
    }
}

fn warn_kept_once(world: &mut World, type_path: &str) {
    let mut warnings = world.get_resource_or_init::<NodeSyncWarnings>();
    if warnings.0.insert(format!("kept {type_path}")) {
        warn!(
            "PIE: node sync kept `{type_path}`: another component on the entity requires it, \
             so the running game keeps its last value until it restarts"
        );
    }
}

/// Apply one node's changed components to every entity spawned from it: each
/// value in `set` replaces (or adds) that component, and each type path in
/// `removed` is removed. A removed component that a component staying on the
/// entity requires is left as it is, with a warning once per type path. A
/// type the game does not know, or a value that does not deserialize, is
/// skipped with a warning once per type path. While a scene root has yet to
/// spawn the change is also held for the entities it brings; see
/// [`PendingNodeChanges`].
pub fn apply_node_components(
    world: &mut World,
    node: u64,
    set: &[(String, serde_json::Value)],
    removed: &[String],
) {
    let entities: Vec<Entity> = world
        .get_resource::<SceneNodeIndex>()
        .map(|index| index.entities(SceneNodeId(node)).to_vec())
        .unwrap_or_default();
    let awaiting = crate::scene_root_awaiting_spawn(world);
    let missed = apply_to_entities(world, &entities, set, removed, awaiting);
    if awaiting && let Some(mut pending) = world.get_resource_mut::<PendingNodeChanges>() {
        pending.hold(node, set, removed, &entities, &missed);
    }
}

/// The digest of the definition each `#Name` last loaded from the editor.
#[derive(Resource, Default)]
struct EditorDefinitions(HashMap<String, u64>);

/// Load the named asset definitions in `text` into their asset stores and
/// [`BsnSceneAssets`], under both reference spellings. A name that already answers
/// to a live asset of its type has the definition written into that asset, so
/// every entity holding it updates. A definition the name last loaded leaves the
/// asset alone, with whatever the game has done to it since.
pub fn load_scene_assets(world: &mut World, text: &str) {
    let mut ast = match jackdaw_bsn::parse_bsn_text(text) {
        Ok(ast) => ast,
        Err(err) => {
            warn!("PIE: could not read the asset definitions the editor sent: {err}");
            return;
        }
    };
    let held = world
        .get_resource::<BsnSceneAssets>()
        .map(|assets| assets.0.clone())
        .unwrap_or_default();
    let mut loaded = world
        .remove_resource::<EditorDefinitions>()
        .unwrap_or_default();
    let mut changed = HashMap::new();
    let mut unchanged = Vec::new();
    for &root in &ast.roots {
        let Some((reference, digest)) = definition_of(&ast, root) else {
            continue;
        };
        if held.contains_key(&reference) && loaded.0.get(&reference) == Some(&digest) {
            unchanged.push(root);
        } else {
            changed.insert(reference, digest);
        }
    }
    ast.roots.retain(|root| !unchanged.contains(root));
    let entries = load_with_textures(world, &ast, |name| held.get(&format!("#{name}")).cloned());
    let mut assets = world.get_resource_or_init::<BsnSceneAssets>();
    for entry in entries {
        let reference = format!("#{}", entry.name);
        if let Some(digest) = changed.remove(&reference) {
            loaded.0.insert(reference.clone(), digest);
        }
        assets.0.insert(reference, entry.handle.clone());
        assets.0.insert(format!("@{}", entry.name), entry.handle);
    }
    world.insert_resource(loaded);
}

/// The `#Name` of the asset root `root` and a digest of what it defines.
fn definition_of(ast: &jackdaw_bsn::SceneBsnAst, root: Entity) -> Option<(String, u64)> {
    use std::hash::{Hash, Hasher};
    let name = ast.get_name(root)?;
    let (type_path, value) = jackdaw_bsn::asset_value_from_root(ast, root)?;
    let mut hasher = std::hash::DefaultHasher::new();
    (type_path, jackdaw_bsn::emit_value_text(&value)).hash(&mut hasher);
    Some((format!("#{name}"), hasher.finish()))
}

/// Load each asset file the editor sent, as `(path, text)`, into
/// [`JackdawCatalog`]: a file the catalog holds has the definition written into its
/// asset, so every entity holding it updates, and a new one joins under its path.
pub fn load_asset_files(world: &mut World, files: &[(String, String)]) {
    for (path, text) in files {
        let held = world
            .get_resource::<JackdawCatalog>()
            .and_then(|catalog| catalog.files.get(path).cloned());
        match jackdaw_bsn::parse_bsn_text(text) {
            Ok(ast) => {
                let entries = load_with_textures(world, &ast, |_| held.clone());
                let (Some(entry), Some(mut catalog)) = (
                    entries.into_iter().next(),
                    world.get_resource_mut::<JackdawCatalog>(),
                ) else {
                    warn!("PIE: the editor sent no asset this game can hold for {path}");
                    continue;
                };
                catalog.files.insert(path.clone(), entry.handle.clone());
                world
                    .get_resource_or_init::<BsnSceneAssets>()
                    .0
                    .insert(path.clone(), entry.handle);
            }
            Err(err) => warn!("PIE: could not read the asset file {path} the editor sent: {err}"),
        }
    }
}

/// Load definitions as a scene spawn does: each texture with its slot's colour
/// space, ahead of the generic resolve reaching the same path.
fn load_with_textures(
    world: &mut World,
    ast: &jackdaw_bsn::SceneBsnAst,
    held: impl Fn(&str) -> Option<UntypedHandle>,
) -> Vec<jackdaw_bsn::CatalogEntry> {
    #[cfg(feature = "render")]
    let _preloaded_textures = crate::preload_textures(world, ast);
    jackdaw_bsn::load_document_assets_onto(world, ast, held)
}

/// Write a node's change to `entities`. Returns the types whose value named an
/// asset nothing has loaded yet. With `retry`, such a value is left to be tried
/// again rather than warned about. Any other value that fails is warned about.
fn apply_to_entities(
    world: &mut World,
    entities: &[Entity],
    set: &[(String, serde_json::Value)],
    removed: &[String],
    retry: bool,
) -> Vec<String> {
    let mut missed = Vec::new();
    if entities.is_empty() {
        return missed;
    }
    for (type_path, value) in set {
        match decode_component(world, type_path, value) {
            Ok(decoded) => {
                for &entity in entities {
                    write_component(world, entity, &decoded);
                }
            }
            Err(DecodeFailure::UnresolvedAsset(_)) if retry => missed.push(type_path.clone()),
            Err(failure) => warn_once(world, type_path, &failure.to_string()),
        }
    }
    let mut removals = Vec::new();
    for type_path in removed {
        match removal_for(world, type_path) {
            Ok(removal) => removals.push(removal),
            Err(reason) => warn_once(world, type_path, &reason),
        }
    }
    let leaving: Vec<ComponentId> = removals.iter().filter_map(|removal| removal.id).collect();
    for removal in &removals {
        for &entity in entities {
            if let Some(id) = removal.id
                && required_on(world, entity, id, &leaving)
            {
                warn_kept_once(world, &removal.type_path);
                continue;
            }
            if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
                removal.reflect.remove(&mut entity_mut);
            }
        }
    }
    missed
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
    const NAME: &str = "bevy_ecs::name::Name";

    #[derive(Component, Reflect, Default, Debug, PartialEq)]
    #[component(immutable)]
    #[reflect(Component, Default)]
    struct Tint {
        level: u32,
    }

    #[derive(Component, Reflect, Default, Debug, PartialEq)]
    #[reflect(Component, Default)]
    struct Path {
        points: Vec<f32>,
    }

    #[derive(Asset, TypePath, Default)]
    struct Swatch;

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    struct Painted {
        swatch: Handle<Swatch>,
    }

    #[derive(Component, Reflect, Default, Debug, PartialEq)]
    #[reflect(Component, Default)]
    struct Health(u32);

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    #[require(Health)]
    struct Player;

    const HEALTH: &str = "jackdaw_runtime::node_sync::tests::Health";
    const PLAYER: &str = "jackdaw_runtime::node_sync::tests::Player";
    const TINT: &str = "jackdaw_runtime::node_sync::tests::Tint";
    const PATH: &str = "jackdaw_runtime::node_sync::tests::Path";
    const PAINTED: &str = "jackdaw_runtime::node_sync::tests::Painted";

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.register_type::<Name>();
        app.register_type::<Transform>();
        app.register_type::<Tint>();
        app.register_type::<Path>();
        app.register_type::<SceneNodeId>();
        install_node_index(&mut app);
        app
    }

    fn transform_json(x: f32) -> serde_json::Value {
        serde_json::json!({
            "translation": [x, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0]
        })
    }

    #[test]
    fn the_index_follows_spawns_despawns_and_shared_nodes() {
        let mut app = app();
        let early = app.world_mut().spawn(SceneNodeId(5)).id();
        let mut late = App::new();
        late.world_mut().spawn(SceneNodeId(9));
        install_node_index(&mut late);
        assert_eq!(
            late.world()
                .resource::<SceneNodeIndex>()
                .entities(SceneNodeId(9))
                .len(),
            1,
            "entities spawned before the index is installed are indexed"
        );

        let twin = app.world_mut().spawn(SceneNodeId(5)).id();
        let index = app.world().resource::<SceneNodeIndex>();
        assert_eq!(index.entities(SceneNodeId(5)), &[early, twin]);

        app.world_mut().despawn(early);
        let index = app.world().resource::<SceneNodeIndex>();
        assert_eq!(index.entities(SceneNodeId(5)), &[twin]);

        app.world_mut().entity_mut(twin).insert(SceneNodeId(6));
        let index = app.world().resource::<SceneNodeIndex>();
        assert!(index.entities(SceneNodeId(5)).is_empty());
        assert_eq!(index.entities(SceneNodeId(6)), &[twin]);

        app.world_mut().entity_mut(twin).remove::<SceneNodeId>();
        let index = app.world().resource::<SceneNodeIndex>();
        assert!(index.entities(SceneNodeId(6)).is_empty());
    }

    #[test]
    fn a_set_reaches_every_entity_of_the_node_and_no_other() {
        let mut app = app();
        let a = app
            .world_mut()
            .spawn((SceneNodeId(1), Transform::default()))
            .id();
        let b = app.world_mut().spawn(SceneNodeId(1)).id();
        let other = app
            .world_mut()
            .spawn((SceneNodeId(2), Transform::default()))
            .id();

        apply_node_components(
            app.world_mut(),
            1,
            &[
                (TRANSFORM.to_string(), transform_json(4.0)),
                (NAME.to_string(), serde_json::json!("Rock")),
            ],
            &[],
        );

        let world = app.world();
        for entity in [a, b] {
            assert_eq!(world.get::<Transform>(entity).unwrap().translation.x, 4.0);
            assert_eq!(world.get::<Name>(entity).unwrap().as_str(), "Rock");
        }
        assert_eq!(world.get::<Transform>(other).unwrap().translation.x, 0.0);
        assert!(world.get::<Name>(other).is_none());
    }

    #[test]
    fn removed_components_are_removed() {
        let mut app = app();
        let entity = app
            .world_mut()
            .spawn((SceneNodeId(3), Name::new("Gone"), Transform::default()))
            .id();
        apply_node_components(app.world_mut(), 3, &[], &[NAME.to_string()]);
        assert!(app.world().get::<Name>(entity).is_none());
        assert!(app.world().get::<Transform>(entity).is_some());
    }

    /// Undoing an override of a required component removes it from the node,
    /// but an entity whose other components require it keeps it as it is.
    #[test]
    fn a_required_component_is_left_unchanged() {
        let mut app = app();
        app.register_type::<Health>().register_type::<Player>();
        let player = app
            .world_mut()
            .spawn((SceneNodeId(4), Player, Health(30)))
            .id();
        let loose = app.world_mut().spawn((SceneNodeId(4), Health(30))).id();
        apply_node_components(app.world_mut(), 4, &[], &[HEALTH.to_string()]);
        assert_eq!(app.world().get::<Health>(player), Some(&Health(30)));
        assert!(app.world().get::<Health>(loose).is_none());
    }

    /// A requirer leaving in the same change does not keep what it requires,
    /// whichever order the removals come in.
    #[test]
    fn a_requirer_removed_alongside_does_not_keep_its_requirement() {
        let mut app = app();
        app.register_type::<Health>().register_type::<Player>();
        for (node, removed) in [(5, [HEALTH, PLAYER]), (6, [PLAYER, HEALTH])] {
            let entity = app
                .world_mut()
                .spawn((SceneNodeId(node), Player, Health(30)))
                .id();
            let removed: Vec<String> = removed.iter().map(ToString::to_string).collect();
            apply_node_components(app.world_mut(), node, &[], &removed);
            assert!(app.world().get::<Health>(entity).is_none(), "{removed:?}");
            assert!(app.world().get::<Player>(entity).is_none(), "{removed:?}");
        }
    }

    fn x_of(app: &App, entity: Entity) -> f32 {
        app.world()
            .get::<Transform>(entity)
            .expect("a transform")
            .translation
            .x
    }

    /// A headless game that loads scenes, with the node index installed.
    fn scene_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::transform::TransformPlugin);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
        install_node_index(&mut app);
        app.add_plugins(crate::JackdawPlugin);
        app
    }

    fn scene_with_node(node: u64, x: f32) -> crate::JackdawScene {
        let text = format!(
            "bevy_ecs::hierarchy::Children [\n    \
             jackdaw_scene_types::node_id::SceneNodeId({node})\n    \
             bevy_transform::components::transform::Transform {{\n        \
             translation: glam::Vec3 {{ x: {x:?}, y: 0.0, z: 0.0 }},\n    }}\n]\n"
        );
        crate::JackdawScene::new(text, std::path::PathBuf::new())
    }

    /// A root whose scene has yet to arrive, and the handle to deliver it on.
    fn waiting_root(app: &mut App) -> Handle<crate::JackdawScene> {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<crate::JackdawScene>>()
            .reserve_handle();
        app.world_mut()
            .spawn(crate::JackdawSceneRoot(handle.clone()));
        handle
    }

    fn deliver(app: &mut App, handle: &Handle<crate::JackdawScene>, scene: crate::JackdawScene) {
        app.world_mut()
            .resource_mut::<Assets<crate::JackdawScene>>()
            .insert(handle.id(), scene)
            .expect("a live handle");
    }

    fn node_xs(app: &mut App, node: u64) -> Vec<f32> {
        let mut query = app.world_mut().query::<(&SceneNodeId, &Transform)>();
        query
            .iter(app.world())
            .filter(|(id, _)| id.0 == node)
            .map(|(_, transform)| transform.translation.x)
            .collect()
    }

    #[test]
    fn an_edit_made_while_a_root_waits_lands_after_the_scene_spawns() {
        let mut app = scene_app();
        let handle = waiting_root(&mut app);
        apply_node_components(
            app.world_mut(),
            21,
            &[(TRANSFORM.to_string(), transform_json(9.0))],
            &[],
        );
        assert!(app.world().resource::<PendingNodeChanges>().contains(21));

        deliver(&mut app, &handle, scene_with_node(21, 1.0));
        app.update();
        assert_eq!(
            node_xs(&mut app, 21),
            vec![9.0],
            "the held edit wins over the scene value"
        );
        assert!(
            app.world().resource::<PendingNodeChanges>().is_empty(),
            "nothing is held once no root waits"
        );
    }

    /// The document a later root spawns from already holds the edit, so with no
    /// root waiting there is nothing to keep.
    #[test]
    fn an_edit_with_no_root_waiting_is_not_held() {
        let mut app = app();
        apply_node_components(
            app.world_mut(),
            8,
            &[(TRANSFORM.to_string(), transform_json(3.0))],
            &[],
        );
        assert!(app.world().resource::<PendingNodeChanges>().is_empty());
    }

    /// A removal cancels a held value of its type, and a later value cancels
    /// a held removal.
    #[test]
    fn held_changes_merge_per_type() {
        let mut app = scene_app();
        let handle = waiting_root(&mut app);
        let name = |text: &str| (NAME.to_string(), serde_json::json!(text));
        let world = app.world_mut();
        apply_node_components(
            world,
            8,
            &[name("Early"), (TRANSFORM.to_string(), transform_json(1.0))],
            &[],
        );
        apply_node_components(world, 8, &[], &[NAME.to_string()]);
        apply_node_components(world, 9, &[], &[NAME.to_string()]);
        apply_node_components(world, 9, &[name("Late")], &[]);

        let removed = world
            .spawn((SceneNodeId(8), Name::new("Spawned"), Transform::default()))
            .id();
        let renamed = world
            .spawn((SceneNodeId(9), Name::new("Spawned"), Transform::default()))
            .id();
        deliver(&mut app, &handle, scene_with_node(30, 0.0));
        app.update();
        assert!(app.world().get::<Name>(removed).is_none());
        assert_eq!(x_of(&app, removed), 1.0);
        assert_eq!(
            app.world().get::<Name>(renamed).map(Name::as_str),
            Some("Late")
        );
    }

    /// Nothing about how long a root waits drops what is held for it, however
    /// long ago the first scene spawned.
    #[test]
    fn a_held_edit_waits_as_long_as_the_root_does() {
        let mut app = scene_app();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs(31),
        ));
        let first = waiting_root(&mut app);
        deliver(&mut app, &first, scene_with_node(1, 0.0));
        app.update();
        assert_eq!(node_xs(&mut app, 1).len(), 1, "a first scene spawned");

        let later = waiting_root(&mut app);
        apply_node_components(
            app.world_mut(),
            22,
            &[(TRANSFORM.to_string(), transform_json(7.0))],
            &[],
        );
        app.update();
        app.update();
        assert!(app.world().resource::<PendingNodeChanges>().contains(22));

        deliver(&mut app, &later, scene_with_node(22, 1.0));
        app.update();
        assert_eq!(node_xs(&mut app, 22), vec![7.0]);
    }

    /// Under the link a later scene adds to the references live sync reads; a
    /// game with no link has no use for an earlier scene's and lets them go.
    #[test]
    fn a_later_scene_keeps_earlier_references_only_under_the_link() {
        let spawn_over_earlier_reference = |app: &mut App| {
            app.update();
            let earlier = UntypedHandle::default_for_type(std::any::TypeId::of::<Swatch>());
            let mut names = bevy::platform::collections::HashMap::default();
            names.insert("#Earlier".to_string(), earlier);
            app.world_mut().insert_resource(BsnSceneAssets(names));
            let root = waiting_root(app);
            deliver(app, &root, scene_with_node(40, 0.0));
            app.update();
            app.world()
                .resource::<BsnSceneAssets>()
                .0
                .contains_key("#Earlier")
        };
        assert!(spawn_over_earlier_reference(&mut scene_app()));

        let mut plain = App::new();
        plain.add_plugins(MinimalPlugins);
        plain.add_plugins(bevy::transform::TransformPlugin);
        plain.add_plugins(bevy::asset::AssetPlugin::default());
        plain.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
        plain.add_plugins(crate::JackdawPlugin);
        assert!(!spawn_over_earlier_reference(&mut plain));
    }

    /// A root spawned earlier has the change directly and is not written again
    /// when a later root spawns, so what the game did to it since stays.
    #[test]
    fn a_held_edit_is_not_written_again_to_entities_that_already_had_it() {
        let mut app = scene_app();
        let first = waiting_root(&mut app);
        deliver(&mut app, &first, scene_with_node(23, 1.0));
        app.update();

        let later = waiting_root(&mut app);
        apply_node_components(
            app.world_mut(),
            23,
            &[(TRANSFORM.to_string(), transform_json(5.0))],
            &[],
        );
        assert_eq!(node_xs(&mut app, 23), vec![5.0], "applied on arrival");
        let mut query = app
            .world_mut()
            .query_filtered::<&mut Transform, With<SceneNodeId>>();
        for mut transform in query.iter_mut(app.world_mut()) {
            transform.translation.x = 40.0;
        }

        deliver(&mut app, &later, scene_with_node(23, 1.0));
        app.update();
        let mut xs = node_xs(&mut app, 23);
        xs.sort_by(f32::total_cmp);
        assert_eq!(
            xs,
            vec![5.0, 40.0],
            "the new entity gets it, the old keeps 40"
        );
    }

    #[test]
    fn an_unknown_type_warns_once_and_the_rest_still_applies() {
        let mut app = app();
        let entity = app
            .world_mut()
            .spawn((SceneNodeId(4), Transform::default()))
            .id();
        let set = [
            ("game::Missing".to_string(), serde_json::json!({})),
            (TRANSFORM.to_string(), serde_json::json!("not a transform")),
            (NAME.to_string(), serde_json::json!("Kept")),
        ];
        apply_node_components(app.world_mut(), 4, &set, &["game::Gone".to_string()]);
        apply_node_components(app.world_mut(), 4, &set, &[]);

        let warned = &app.world().resource::<NodeSyncWarnings>().0;
        assert_eq!(warned.len(), 3, "{warned:?}");
        assert_eq!(app.world().get::<Name>(entity).unwrap().as_str(), "Kept");
        assert_eq!(
            app.world().get::<Transform>(entity).unwrap().translation.x,
            0.0
        );
    }

    #[test]
    fn an_immutable_component_is_replaced() {
        let mut app = app();
        let entity = app
            .world_mut()
            .spawn((SceneNodeId(8), Tint { level: 1 }))
            .id();
        apply_node_components(
            app.world_mut(),
            8,
            &[(TINT.to_string(), serde_json::json!({ "level": 5 }))],
            &[],
        );
        assert_eq!(app.world().get::<Tint>(entity), Some(&Tint { level: 5 }));
    }

    #[test]
    fn a_shorter_list_drops_the_stale_tail() {
        let mut app = app();
        let entity = app
            .world_mut()
            .spawn((
                SceneNodeId(9),
                Path {
                    points: vec![1.0, 2.0, 3.0],
                },
            ))
            .id();
        apply_node_components(
            app.world_mut(),
            9,
            &[(PATH.to_string(), serde_json::json!({ "points": [7.0] }))],
            &[],
        );
        assert_eq!(app.world().get::<Path>(entity).unwrap().points, vec![7.0]);
    }

    #[test]
    fn a_handle_reference_resolves_through_the_scene_assets() {
        let mut app = app();
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<Swatch>();
        app.register_type::<Painted>();
        app.register_type_data::<Handle<Swatch>, ReflectHandle>();
        let swatch = app.world_mut().resource_mut::<Assets<Swatch>>().add(Swatch);
        let mut scene = bevy::platform::collections::HashMap::default();
        scene.insert("#Red".to_string(), swatch.clone().untyped());
        app.world_mut().insert_resource(BsnSceneAssets(scene));
        let entity = app
            .world_mut()
            .spawn((SceneNodeId(10), Painted::default()))
            .id();

        apply_node_components(
            app.world_mut(),
            10,
            &[(PAINTED.to_string(), serde_json::json!({ "swatch": "#Red" }))],
            &[],
        );
        assert_eq!(app.world().get::<Painted>(entity).unwrap().swatch, swatch);

        apply_node_components(
            app.world_mut(),
            10,
            &[(
                PAINTED.to_string(),
                serde_json::json!({ "swatch": "#Missing" }),
            )],
            &[],
        );
        assert_eq!(
            app.world().get::<Painted>(entity).unwrap().swatch,
            swatch,
            "a reference the game cannot resolve leaves the component alone"
        );
    }

    #[derive(Resource, Default)]
    struct Inserts(u32);

    #[test]
    fn a_mutable_edit_replaces_in_place_without_running_insert_hooks() {
        let mut app = app();
        app.init_resource::<Inserts>();
        app.add_observer(|_: On<Insert<Path>>, mut inserts: ResMut<Inserts>| inserts.0 += 1);
        let entity = app
            .world_mut()
            .spawn((
                SceneNodeId(11),
                Path {
                    points: vec![1.0, 2.0],
                },
            ))
            .id();
        app.update();
        let before = app.world_mut().increment_change_tick();
        apply_node_components(
            app.world_mut(),
            11,
            &[(PATH.to_string(), serde_json::json!({ "points": [4.0] }))],
            &[],
        );
        assert_eq!(
            app.world().resource::<Inserts>().0,
            1,
            "only the spawn inserted"
        );
        let path = app
            .world()
            .entity(entity)
            .get_ref::<Path>()
            .expect("a path");
        assert_eq!(path.points, vec![4.0]);
        assert!(
            path.last_changed()
                .is_newer_than(before, app.world().read_change_tick()),
            "the replaced value reads as changed"
        );
    }
}
