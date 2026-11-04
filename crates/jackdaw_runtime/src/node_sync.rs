//! Applies authored component values from the editor's document to the
//! entities spawned from each scene node.
//!
//! [`SceneNodeIndex`] maps every [`SceneNodeId`] to the entities that carry
//! it, kept current by observers on the component. [`apply_node_components`]
//! writes a node's changed components to each of those entities through
//! reflection.

use bevy::asset::{AssetServer, ReflectHandle, UntypedHandle};
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::serde::{ReflectDeserializerProcessor, TypedReflectDeserializer};
use bevy::reflect::{PartialReflect, TypeRegistration, TypeRegistry};
use jackdaw_scene_types::SceneNodeId;
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

/// Install [`SceneNodeIndex`] and the observers that keep it current, indexing
/// the entities that already carry a [`SceneNodeId`].
pub fn install_node_index(app: &mut App) {
    app.init_resource::<SceneNodeIndex>();
    app.init_resource::<NodeSyncWarnings>();
    app.add_observer(index_node_insert);
    app.add_observer(index_node_replace);
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

fn index_node_replace(
    replace: On<Discard<SceneNodeId>>,
    nodes: Query<&SceneNodeId>,
    mut index: ResMut<SceneNodeIndex>,
) {
    if let Ok(node) = nodes.get(replace.entity) {
        index.remove(*node, replace.entity);
    }
}

/// Reads an asset handle as the asset path the editor sent, loading it through
/// the asset server, and `null` as the handle type's default.
struct HandlePathProcessor<'a> {
    server: Option<&'a AssetServer>,
}

impl ReflectDeserializerProcessor for HandlePathProcessor<'_> {
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
        let untyped = match (Option::<String>::deserialize(deserializer)?, self.server) {
            (Some(path), Some(server)) => server.load_builder().load_erased(asset_type, path),
            _ => UntypedHandle::default_for_type(asset_type),
        };
        Ok(Ok(reflect_handle.typed(untyped).into_partial_reflect()))
    }
}

/// A component value read from JSON, with the reflection needed to write it.
pub(crate) struct DecodedComponent {
    pub(crate) reflect: ReflectComponent,
    pub(crate) value: Box<dyn PartialReflect>,
}

/// Deserialize `value` as the component `type_path` names. Asset handles are
/// read as asset paths and loaded through the world's asset server.
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
    let mut processor = HandlePathProcessor {
        server: world.get_resource::<AssetServer>(),
    };
    let value = TypedReflectDeserializer::with_processor(registration, &registry, &mut processor)
        .deserialize(value)
        .map_err(|err| format!("failed to deserialize `{type_path}`: {err}"))?;
    Ok(DecodedComponent { reflect, value })
}

/// Look up the reflection for `type_path` without a value.
fn reflect_component_for(world: &World, type_path: &str) -> Result<ReflectComponent, String> {
    let registry = world.resource::<AppTypeRegistry>().read();
    let registration = registry
        .get_with_type_path(type_path)
        .ok_or_else(|| format!("unknown type path `{type_path}`"))?;
    registration
        .data::<ReflectComponent>()
        .cloned()
        .ok_or_else(|| format!("`{type_path}` has no ReflectComponent"))
}

/// Write `decoded` onto `entity`: applied over the component when present,
/// inserted when not.
pub(crate) fn write_component(world: &mut World, entity: Entity, decoded: &DecodedComponent) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let Ok(entity_ref) = world.get_entity(entity) else {
        return;
    };
    let present = decoded.reflect.contains(entity_ref);
    let mut entity_mut = world.entity_mut(entity);
    if present {
        decoded.reflect.apply(entity_mut, decoded.value.as_ref());
    } else {
        decoded
            .reflect
            .insert(&mut entity_mut, decoded.value.as_ref(), &registry);
    }
}

fn warn_once(world: &mut World, type_path: &str, reason: &str) {
    let mut warnings = world.get_resource_or_init::<NodeSyncWarnings>();
    if warnings.0.insert(type_path.to_string()) {
        warn!("PIE: node sync skipped {reason}");
    }
}

/// Apply one node's changed components to every entity spawned from it: each
/// value in `set` replaces (or adds) that component, and each type path in
/// `removed` is removed. A type the game does not know, or a value that does
/// not deserialize, is skipped with a warning once per type path.
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
    for type_path in removed {
        match reflect_component_for(world, type_path) {
            Ok(reflect) => {
                for &entity in &entities {
                    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
                        reflect.remove(&mut entity_mut);
                    }
                }
            }
            Err(reason) => warn_once(world, type_path, &reason),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
    const NAME: &str = "bevy_ecs::name::Name";

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.register_type::<Name>();
        app.register_type::<Transform>();
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
}
