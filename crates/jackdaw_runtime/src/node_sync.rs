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
