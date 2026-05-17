use std::any::TypeId;
use std::collections::HashMap;

use bevy::{
    asset::ReflectHandle,
    ecs::reflect::AppTypeRegistry,
    prelude::*,
    reflect::{
        TypeRegistry,
        serde::{ReflectSerializerProcessor, TypedReflectSerializer},
    },
    remote::BrpResult,
};
use serde::{Serialize, Serializer};
use serde_json::Value;

/// A single entity's full ECS state, serialized for remote inspection.
#[derive(Serialize, serde::Deserialize, Clone, Debug)]
pub struct RemoteEntity {
    /// Bevy entity bits (u64) for tracking across snapshots.
    pub entity: u64,
    /// ALL reflectable components, keyed by type path, serialized via `TypedReflectSerializer`.
    pub components: HashMap<String, Value>,
}

/// Component type path prefixes to skip during scene snapshot.
const SKIP_PREFIXES: &[&str] = &[
    "bevy_render::",
    "bevy_picking::",
    "bevy_window::",
    "bevy_ecs::observer::",
    "bevy_camera::primitives::",
    "bevy_camera::visibility::",
];

/// Specific component type paths to skip.
const SKIP_PATHS: &[&str] = &[
    "bevy_transform::components::transform::TransformTreeChanged",
    "bevy_light::cascade::Cascades",
    "bevy_transform::components::transform::GlobalTransform",
    "bevy_ecs::visibility::InheritedVisibility",
    "bevy_ecs::visibility::ViewVisibility",
    "bevy_ecs::hierarchy::Children",
];

fn should_skip(type_path: &str) -> bool {
    for prefix in SKIP_PREFIXES {
        if type_path.starts_with(prefix) {
            return true;
        }
    }
    SKIP_PATHS.contains(&type_path)
}

/// BRP handler for `jackdaw/scene_snapshot`.
/// Returns a `Vec<RemoteEntity>` with all reflectable components serialized.
pub fn scene_snapshot_handler(
    In(_params): In<Option<Value>>,
    query: Query<Entity, With<Transform>>,
    world: &World,
    registry: Res<AppTypeRegistry>,
) -> BrpResult {
    let registry = registry.read();
    let entities: Vec<Entity> = query.iter().collect();
    let result = build_snapshot(world, &registry, &entities);
    Ok(serde_json::to_value(&result).unwrap())
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

fn build_snapshot(
    world: &World,
    registry: &TypeRegistry,
    entities: &[Entity],
) -> Vec<RemoteEntity> {
    let processor = RemoteSerializerProcessor;

    entities
        .iter()
        .map(|&entity| {
            let entity_ref = world.entity(entity);
            let mut components = HashMap::new();

            for registration in registry.iter() {
                let type_path = registration.type_info().type_path_table().path();

                if should_skip(type_path) {
                    continue;
                }

                let Some(reflect_component) = registration.data::<ReflectComponent>() else {
                    continue;
                };
                let Some(component) = reflect_component.reflect(entity_ref) else {
                    continue;
                };

                let serializer =
                    TypedReflectSerializer::with_processor(component, registry, &processor);
                if let Ok(value) = serde_json::to_value(&serializer) {
                    components.insert(type_path.to_string(), value);
                }
            }

            RemoteEntity {
                entity: entity.to_bits(),
                components,
            }
        })
        .collect()
}
