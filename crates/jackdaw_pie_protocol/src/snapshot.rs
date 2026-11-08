use std::any::TypeId;
use std::collections::HashMap;

use bevy::{
    asset::ReflectHandle,
    ecs::{
        archetype::{Archetype, ArchetypeId},
        reflect::ReflectComponent,
        world::EntityRef,
    },
    prelude::*,
    reflect::{
        TypeRegistry,
        serde::{ReflectSerializerProcessor, TypedReflectSerializer},
    },
};
use serde::{Serialize, Serializer};
use serde_json::Value;

/// A single entity's full ECS state, serialized for remote inspection.
#[derive(Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
pub struct RemoteEntity {
    /// Bevy entity bits (u64) for tracking across snapshots.
    pub entity: u64,
    /// Reflectable components keyed by type path, serialized via `TypedReflectSerializer`.
    pub components: HashMap<String, Value>,
    /// The authored scene node this entity was spawned from, or `None` for a runtime-spawned entity.
    #[serde(default)]
    pub scene_node_id: Option<u64>,
}

/// The bits of an entity a reflected component carries, as JSON: the
/// `{index}v{generation}` text Bevy writes, or bare bits, alone or as the
/// single field of a tuple struct such as `ChildOf`.
pub fn entity_bits(value: &Value) -> Option<u64> {
    match value {
        Value::Number(number) => number.as_u64(),
        Value::String(text) => text.parse::<Entity>().ok().map(Entity::to_bits),
        Value::Array(fields) => fields.first().and_then(entity_bits),
        Value::Object(fields) => fields.values().next().and_then(entity_bits),
        _ => None,
    }
}

/// Component type path prefixes to skip during scene snapshot.
const SKIP_PREFIXES: &[&str] = &[
    "bevy_render::",
    "bevy_picking::",
    "bevy_window::",
    "bevy_ecs::observer::",
    "bevy_camera::primitives::",
    "bevy_camera::visibility::",
    // Camera, RenderTarget, Camera2d/Camera3d: applying these to a preview
    // entity would create an ACTIVE render camera in the editor world (worst
    // case targeting a non-renderable image through a nulled handle, which
    // aborts the render pass). The editor reconstructs camera state it needs
    // from CameraRig and Projection instead.
    "bevy_camera::camera::",
    "bevy_camera::components::",
    // Would cause our own entities to become relation targets. Big mess!
    "avian3d::collision::collider::collider_hierarchy::",
];

/// Specific component type paths to skip.
const SKIP_PATHS: &[&str] = &[
    "bevy_transform::components::transform::TransformTreeChanged",
    "bevy_light::cascade::Cascades",
    "bevy_transform::components::transform::GlobalTransform",
    "bevy_ecs::visibility::InheritedVisibility",
    "bevy_ecs::visibility::ViewVisibility",
    "bevy_ecs::hierarchy::Children",
    // Causes crashes with e.g. ColliderConstructor::ConvexHullFromMesh
    "avian3d::collision::collider::constructor::ColliderConstructor",
];

fn should_skip(type_path: &str) -> bool {
    for prefix in SKIP_PREFIXES {
        if type_path.starts_with(prefix) {
            return true;
        }
    }
    SKIP_PATHS.contains(&type_path)
}

/// Serializer processor for remote snapshots.
/// - `Handle<T>` -> null (game assets aren't loaded in editor)
/// - Entity fields -> raw u64 bits (no index remapping)
/// - Non-finite floats -> descriptive strings
struct RemoteSerializerProcessor;

impl ReflectSerializerProcessor for RemoteSerializerProcessor {
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

        // Non-finite floats
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

        // Handle<T> -> null
        if registry.get_type_data::<ReflectHandle>(type_id).is_some() {
            return Ok(Ok(serializer.serialize_unit()?));
        }

        // Entity -> raw u64 bits
        if type_id == TypeId::of::<Entity>() {
            if let Some(entity) = value.as_any().downcast_ref::<Entity>() {
                return Ok(Ok(serializer.serialize_u64(entity.to_bits())?));
            }
            return Ok(Ok(serializer.serialize_unit()?));
        }

        Ok(Err(serializer))
    }
}

/// One component an entity of some archetype snapshots, resolved from the type
/// registry once per archetype.
struct SnapshotColumn {
    type_path: &'static str,
    reflect: ReflectComponent,
}

impl SnapshotColumn {
    /// The component's serialized value on `entity`, if it has one that serializes.
    fn value(&self, entity: EntityRef, registry: &TypeRegistry) -> Option<Value> {
        let component = self.reflect.reflect(entity)?;
        let processor = RemoteSerializerProcessor;
        let serializer = TypedReflectSerializer::with_processor(component, registry, &processor);
        serde_json::to_value(&serializer).ok()
    }
}

/// A reflected value serialized as [`entity_components`] serializes component
/// values, or `None` when it does not serialize.
pub fn reflect_value_json(value: &dyn PartialReflect, registry: &TypeRegistry) -> Option<Value> {
    let processor = RemoteSerializerProcessor;
    let serializer = TypedReflectSerializer::with_processor(value, registry, &processor);
    serde_json::to_value(&serializer).ok()
}

/// The components of `archetype` a snapshot holds: registered as reflectable
/// components and not on a skip list.
fn snapshot_columns(
    world: &World,
    registry: &TypeRegistry,
    archetype: &Archetype,
) -> Vec<SnapshotColumn> {
    archetype
        .iter_components()
        .filter_map(|id| {
            let type_id = world.components().get_info(id)?.type_id()?;
            let registration = registry.get(type_id)?;
            let type_path = registration.type_info().type_path_table().path();
            if should_skip(type_path) {
                return None;
            }
            Some(SnapshotColumn {
                type_path,
                reflect: registration.data::<ReflectComponent>()?.clone(),
            })
        })
        .collect()
}

/// The values of `entity`'s components by type path, sorted, with the skip lists
/// applied, and the names of the components it holds that do not serialize.
pub fn entity_components(
    world: &World,
    registry: &TypeRegistry,
    entity: EntityRef,
) -> (Vec<(String, Value)>, Vec<String>) {
    let mut values = Vec::new();
    let mut opaque = Vec::new();
    for id in entity.archetype().iter_components() {
        let Some(info) = world.components().get_info(id) else {
            continue;
        };
        let registration = info.type_id().and_then(|type_id| registry.get(type_id));
        let column = registration.and_then(|registration| {
            Some(SnapshotColumn {
                type_path: registration.type_info().type_path_table().path(),
                reflect: registration.data::<ReflectComponent>()?.clone(),
            })
        });
        match column {
            Some(column) if should_skip(column.type_path) => {}
            Some(column) => match column.value(entity, registry) {
                Some(value) => values.push((column.type_path.to_string(), value)),
                None => opaque.push(column.type_path.to_string()),
            },
            None => opaque.push(info.name().to_string()),
        }
    }
    values.sort_by(|a, b| a.0.cmp(&b.0));
    opaque.sort();
    (values, opaque)
}

/// The value of `entity`'s component with type path `type_path`, as
/// [`entity_components`] holds it, or its name among the opaque ones.
pub fn entity_component(
    registry: &TypeRegistry,
    entity: EntityRef,
    type_path: &str,
) -> (Vec<(String, Value)>, Vec<String>) {
    let Some(reflect) = registry
        .get_with_type_path(type_path)
        .and_then(|registration| registration.data::<ReflectComponent>())
    else {
        return (Vec::new(), Vec::new());
    };
    let column = SnapshotColumn {
        type_path: "",
        reflect: reflect.clone(),
    };
    if !reflect.contains(entity) {
        return (Vec::new(), Vec::new());
    }
    match column.value(entity, registry) {
        Some(value) => (vec![(type_path.to_string(), value)], Vec::new()),
        None => (Vec::new(), vec![type_path.to_string()]),
    }
}

/// Reflection-serialize `entities` for remote display, applying the skip lists
/// and `RemoteSerializerProcessor` edge-case handling.
pub fn build_snapshot(
    world: &World,
    registry: &TypeRegistry,
    entities: &[Entity],
) -> Vec<RemoteEntity> {
    let mut columns: HashMap<ArchetypeId, Vec<SnapshotColumn>> = HashMap::new();
    entities
        .iter()
        .map(|&entity| {
            let entity_ref = world.entity(entity);
            let archetype = entity_ref.archetype();
            let columns = columns
                .entry(archetype.id())
                .or_insert_with(|| snapshot_columns(world, registry, archetype));
            let components = columns
                .iter()
                .filter_map(|column| {
                    let value = column.value(entity_ref, registry)?;
                    Some((column.type_path.to_string(), value))
                })
                .collect();
            RemoteEntity {
                entity: entity.to_bits(),
                components,
                scene_node_id: None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::{App, AppTypeRegistry, Name, Transform};

    #[test]
    fn camera_component_paths_are_skipped() {
        assert!(should_skip("bevy_camera::camera::Camera"));
        assert!(should_skip("bevy_camera::camera::RenderTarget"));
        assert!(should_skip("bevy_camera::components::Camera3d"));
        assert!(should_skip("bevy_camera::components::Camera2d"));
        assert!(!should_skip("bevy_camera::projection::Projection"));
    }

    #[test]
    fn build_snapshot_serializes_named_transform_entity() {
        let mut app = App::new();
        app.register_type::<Name>();
        let id = app
            .world_mut()
            .spawn((Name::new("probe"), Transform::from_xyz(1.0, 2.0, 3.0)))
            .id();
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let snap = build_snapshot(app.world(), &registry, &[id]);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].entity, id.to_bits());
        // Name is reflectable and not skipped, so it must be present.
        assert!(
            snap[0].components.keys().any(|k| k.contains("Name")),
            "expected a Name component, got: {:?}",
            snap[0].components.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_entity_is_read_from_its_text_or_its_bits() {
        let entity = Entity::from_raw_u32(5).unwrap();
        let bits = entity.to_bits();
        let text = serde_json::to_value(entity).unwrap();
        assert_eq!(entity_bits(&text), Some(bits));
        assert_eq!(entity_bits(&serde_json::json!(bits)), Some(bits));
        assert_eq!(entity_bits(&serde_json::json!([text])), Some(bits));
        assert_eq!(entity_bits(&serde_json::json!("not an entity")), None);
    }
}
