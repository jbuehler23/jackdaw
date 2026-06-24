use std::any::TypeId;
use std::collections::HashMap;

use bevy::{
    asset::{ReflectHandle, UntypedHandle},
    prelude::*,
    reflect::{
        TypeRegistration, TypeRegistry,
        serde::{ReflectDeserializerProcessor, ReflectSerializerProcessor, TypedReflectSerializer},
    },
};
use serde::de::IgnoredAny;
use serde::{Deserializer, Serialize, Serializer};
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

/// Deserializer counterpart to `RemoteSerializerProcessor`. The serializer emits
/// `null` for every `Handle<T>` (the game's assets are not loaded in the editor).
/// A plain reflect deserializer rejects that `null` and fails the whole component,
/// which drops any brush whose face materials serialized to null. This turns the
/// `null` back into the handle type's default, so the component deserializes and
/// brush faces fall back to the editor's own material in `regenerate_brush_meshes`.
pub struct RemoteDeserializerProcessor;

impl ReflectDeserializerProcessor for RemoteDeserializerProcessor {
    fn try_deserialize<'de, D>(
        &mut self,
        registration: &TypeRegistration,
        _registry: &TypeRegistry,
        deserializer: D,
    ) -> Result<Result<Box<dyn PartialReflect>, D>, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Intercept asset handles: the serializer emits `null` for each one, so
        // consume that and rebuild the handle type's default. Bevy 0.19 dropped
        // `Handle`'s `Default` impl (so `ReflectDefault` is no longer
        // registered), so build the default through `ReflectHandle` instead.
        if let Some(reflect_handle) = registration.data::<ReflectHandle>() {
            deserializer.deserialize_option(IgnoredAny)?;
            let untyped = UntypedHandle::default_for_type(reflect_handle.asset_type_id());
            return Ok(Ok(reflect_handle.typed(untyped).into_partial_reflect()));
        }
        Ok(Err(deserializer))
    }
}

/// Reflection-serialize `entities` for remote display, applying the skip lists
/// and `RemoteSerializerProcessor` edge-case handling.
pub fn build_snapshot(
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
        // The projection still streams: the editor's camera lock reads it.
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
}
