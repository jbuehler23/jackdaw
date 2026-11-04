//! Applies authored component values from the editor's document to the
//! entities spawned from each scene node.
//!
//! [`SceneNodeIndex`] maps every [`SceneNodeId`] to the entities that carry
//! it, kept current by observers on the component. [`apply_node_components`]
//! writes a node's changed components to each of those entities through
//! reflection.
//!
//! A change for a node no entity carries yet (an edit made while the game was
//! building or loading its scene) is held in [`PendingNodeChanges`] and
//! applied once the first entity for that node is indexed, after the scene
//! that spawned it has finished inserting its components. Held changes are
//! bounded by the number of distinct node ids, and once the first scene
//! entity has been indexed each one is dropped [`PENDING_LIFETIME`] after it
//! was last changed (or after that first entity, if later): a node that does
//! not appear by then is not one the game spawns.

use bevy::asset::{AssetServer, ReflectHandle, UntypedHandle};
use bevy::ecs::component::ComponentId;
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::serde::{ReflectDeserializerProcessor, TypedReflectDeserializer};
use bevy::reflect::{PartialReflect, ReflectFromReflect, TypeRegistration, TypeRegistry};
use jackdaw_bsn::BsnSceneAssets;
use jackdaw_scene_types::SceneNodeId;
use std::time::Duration;

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

    /// Index `entity` under `node`, answering whether it is the node's first.
    fn add(&mut self, node: SceneNodeId, entity: Entity) -> bool {
        let entities = self.0.entry(node).or_default();
        if !entities.contains(&entity) {
            entities.push(entity);
        }
        entities.len() == 1
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

/// How long a held change waits for its node once the game has indexed its
/// first scene entity.
pub const PENDING_LIFETIME: Duration = Duration::from_secs(30);

/// Node changes that arrived before any entity carried their node, by node.
#[derive(Resource, Default)]
pub struct PendingNodeChanges {
    nodes: HashMap<u64, PendingNode>,
    /// Held nodes that gained their first entity, applied next update.
    ready: Vec<u64>,
    /// Real time at which the first scene entity was indexed.
    first_indexed: Option<Duration>,
}

impl PendingNodeChanges {
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
    fn hold(
        &mut self,
        node: u64,
        set: &[(String, serde_json::Value)],
        removed: &[String],
        now: Duration,
    ) {
        let held = self.nodes.entry(node).or_default();
        held.changed = now;
        for (type_path, value) in set {
            held.removed.retain(|path| path != type_path);
            match held.set.iter_mut().find(|(path, _)| path == type_path) {
                Some((_, held_value)) => *held_value = value.clone(),
                None => held.set.push((type_path.clone(), value.clone())),
            }
        }
        for type_path in removed {
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
    changed: Duration,
}

fn real_now(world: &World) -> Duration {
    world
        .get_resource::<Time<Real>>()
        .map_or(Duration::ZERO, Time::elapsed)
}

/// Install [`SceneNodeIndex`] and the observers that keep it current, indexing
/// the entities that already carry a [`SceneNodeId`].
pub fn install_node_index(app: &mut App) {
    app.init_resource::<SceneNodeIndex>();
    app.init_resource::<NodeSyncWarnings>();
    app.init_resource::<PendingNodeChanges>();
    app.add_observer(index_node_insert);
    app.add_observer(index_node_replace);
    app.add_systems(
        Update,
        apply_pending_node_changes.after(crate::spawn_loaded_scenes),
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
    mut pending: ResMut<PendingNodeChanges>,
    time: Option<Res<Time<Real>>>,
) {
    let Ok(node) = nodes.get(insert.entity) else {
        return;
    };
    let first = index.add(*node, insert.entity);
    if pending.first_indexed.is_none() {
        pending.first_indexed = Some(time.map_or(Duration::ZERO, |time| time.elapsed()));
    }
    if first && pending.contains(node.0) && !pending.ready.contains(&node.0) {
        pending.ready.push(node.0);
    }
}

/// Apply the held changes of nodes that gained their first entity, and drop
/// held changes older than [`PENDING_LIFETIME`]. Runs after scenes spawn, so
/// a held value is not overwritten by a component the spawn inserts later.
fn apply_pending_node_changes(world: &mut World) {
    let now = real_now(world);
    let ready: Vec<(u64, PendingNode)> = {
        let mut pending = world.resource_mut::<PendingNodeChanges>();
        let ready = std::mem::take(&mut pending.ready);
        ready
            .into_iter()
            .filter_map(|node| pending.nodes.remove(&node).map(|held| (node, held)))
            .collect()
    };
    for (node, held) in ready {
        apply_node_components(world, node, &held.set, &held.removed);
    }
    let mut pending = world.resource_mut::<PendingNodeChanges>();
    if let Some(first_indexed) = pending.first_indexed {
        pending.nodes.retain(|_, held| {
            now.saturating_sub(held.changed.max(first_indexed)) < PENDING_LIFETIME
        });
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
/// default. A `#Name` or `@Name` nothing answers to is refused.
struct HandleReferenceProcessor<'a> {
    server: Option<&'a AssetServer>,
    scene: Option<&'a BsnSceneAssets>,
    catalog: Option<&'a JackdawCatalog>,
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
                None if reference.starts_with('#') || reference.starts_with('@') => {
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
) -> Result<DecodedComponent, String> {
    let registry = world.resource::<AppTypeRegistry>().read();
    let registration = registry
        .get_with_type_path(type_path)
        .ok_or_else(|| format!("unknown type path `{type_path}`"))?;
    let reflect = registration
        .data::<ReflectComponent>()
        .ok_or_else(|| format!("`{type_path}` has no ReflectComponent"))?
        .clone();
    let mut processor = HandleReferenceProcessor {
        server: world.get_resource::<AssetServer>(),
        scene: world.get_resource::<BsnSceneAssets>(),
        catalog: world.get_resource::<JackdawCatalog>(),
    };
    let value = TypedReflectDeserializer::with_processor(registration, &registry, &mut processor)
        .deserialize(value)
        .map_err(|err| format!("failed to deserialize `{type_path}`: {err}"))?;
    let from_reflect = registration.data::<ReflectFromReflect>().cloned();
    let value = match &from_reflect {
        Some(from_reflect) => from_reflect
            .from_reflect(value.as_ref())
            .ok_or_else(|| format!("`{type_path}` cannot be built from the value sent"))?
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
/// skipped with a warning once per type path. When no entity carries `node`
/// yet, the change is held in [`PendingNodeChanges`] until one does.
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
    if entities.is_empty() {
        let now = real_now(world);
        if let Some(mut pending) = world.get_resource_mut::<PendingNodeChanges>() {
            pending.hold(node, set, removed, now);
        }
        return;
    }
    for (type_path, value) in set {
        match decode_component(world, type_path, value) {
            Ok(decoded) => {
                for &entity in &entities {
                    write_component(world, entity, &decoded);
                }
            }
            Err(reason) => warn_once(world, type_path, &reason),
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
        for &entity in &entities {
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

    #[test]
    fn a_held_edit_lands_after_a_scene_spawns_the_node() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::transform::TransformPlugin);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
        install_node_index(&mut app);
        app.add_plugins(crate::JackdawPlugin);

        let scene = "\
bevy_ecs::hierarchy::Children [
    jackdaw_scene_types::node_id::SceneNodeId(21)
    bevy_transform::components::transform::Transform {
        translation: glam::Vec3 { x: 1.0, y: 0.0, z: 0.0 },
    }
]
";
        let handle = app
            .world_mut()
            .resource_mut::<Assets<crate::JackdawScene>>()
            .add(crate::JackdawScene::new(
                scene.to_string(),
                std::path::PathBuf::new(),
            ));

        apply_node_components(
            app.world_mut(),
            21,
            &[(TRANSFORM.to_string(), transform_json(9.0))],
            &[],
        );
        assert!(app.world().resource::<PendingNodeChanges>().contains(21));

        app.world_mut().spawn(crate::JackdawSceneRoot(handle));
        app.update();
        app.update();

        let mut query = app.world_mut().query::<(&SceneNodeId, &Transform)>();
        let found: Vec<f32> = query
            .iter(app.world())
            .filter(|(id, _)| id.0 == 21)
            .map(|(_, transform)| transform.translation.x)
            .collect();
        assert_eq!(found, vec![9.0], "the held edit wins over the scene value");
    }

    /// An edit for a node no entity carries yet waits, and lands on the
    /// entity spawned for it, after the spawn's own components.
    #[test]
    fn an_edit_before_its_node_spawns_lands_when_it_spawns() {
        let mut app = app();
        apply_node_components(
            app.world_mut(),
            8,
            &[(TRANSFORM.to_string(), transform_json(3.0))],
            &[],
        );
        assert!(app.world().resource::<PendingNodeChanges>().contains(8));

        let entity = app.world_mut().spawn(SceneNodeId(8)).id();
        app.world_mut()
            .entity_mut(entity)
            .insert(Transform::default());
        app.update();
        assert_eq!(x_of(&app, entity), 3.0);
        assert!(app.world().resource::<PendingNodeChanges>().is_empty());

        let twin = app
            .world_mut()
            .spawn((SceneNodeId(8), Transform::default()))
            .id();
        app.update();
        assert_eq!(
            x_of(&app, twin),
            0.0,
            "an applied change is not held for later entities"
        );
    }

    /// A removal cancels a held value of its type, and a later value cancels
    /// a held removal.
    #[test]
    fn held_changes_merge_per_type() {
        let mut app = app();
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
        app.update();
        assert!(app.world().get::<Name>(removed).is_none());
        assert_eq!(x_of(&app, removed), 1.0);
        assert_eq!(
            app.world().get::<Name>(renamed).map(Name::as_str),
            Some("Late")
        );
    }

    /// Held changes wait as long as no scene entity exists, and expire
    /// [`PENDING_LIFETIME`] after one does.
    #[test]
    fn held_changes_expire_after_the_first_scene_entity() {
        let mut app = app();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            PENDING_LIFETIME / 3,
        ));
        apply_node_components(
            app.world_mut(),
            8,
            &[(TRANSFORM.to_string(), transform_json(3.0))],
            &[],
        );
        for _ in 0..6 {
            app.update();
        }
        assert!(
            app.world().resource::<PendingNodeChanges>().contains(8),
            "nothing expires before the scene spawns"
        );

        app.world_mut().spawn(SceneNodeId(1));
        app.update();
        assert!(app.world().resource::<PendingNodeChanges>().contains(8));
        for _ in 0..4 {
            app.update();
        }
        assert!(app.world().resource::<PendingNodeChanges>().is_empty());
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
