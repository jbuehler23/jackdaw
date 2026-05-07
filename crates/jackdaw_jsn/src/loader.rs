use bevy::{
    asset::{AssetLoader, LoadContext, io::Reader},
    ecs::{
        reflect::AppTypeRegistry,
        world::{FromWorld, World},
    },
    prelude::*,
    reflect::{TypeRegistryArc, serde::TypedReflectDeserializer},
    scene::DynamicScene,
};
use serde::de::DeserializeSeed;

use crate::format::{JsnEntity, JsnScene, JsnSceneV2};

/// Asset loader for `.jsn` files → `DynamicScene`.
#[derive(Debug, TypePath)]
pub struct JsnAssetLoader {
    type_registry: TypeRegistryArc,
}

impl FromWorld for JsnAssetLoader {
    fn from_world(world: &mut World) -> Self {
        let type_registry = world.resource::<AppTypeRegistry>();
        Self {
            type_registry: type_registry.0.clone(),
        }
    }
}

impl AssetLoader for JsnAssetLoader {
    type Asset = DynamicScene;
    type Settings = ();
    type Error = JsnLoadError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &Self::Settings,
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| JsnLoadError::Io(e.to_string()))?;

        let text = std::str::from_utf8(&bytes).map_err(|e| JsnLoadError::Parse(e.to_string()))?;

        let jsn: JsnScene = match serde_json::from_str(text) {
            Ok(jsn) => jsn,
            Err(v3_err) => match serde_json::from_str::<JsnSceneV2>(text) {
                Ok(v2) => v2.migrate_to_v3(),
                Err(_) => return Err(JsnLoadError::Parse(v3_err.to_string())),
            },
        };

        // Build a DynamicScene by spawning into a temporary world
        let scene =
            build_dynamic_scene(&jsn.scene, &self.type_registry).map_err(JsnLoadError::Scene)?;

        Ok(scene)
    }

    fn extensions(&self) -> &[&str] {
        &["jsn"]
    }
}

/// Spawn `JsnEntity` list into a temp world, then extract a `DynamicScene`.
fn build_dynamic_scene(
    entities: &[JsnEntity],
    type_registry: &TypeRegistryArc,
) -> Result<DynamicScene, String> {
    let mut world = World::new();
    world.insert_resource(AppTypeRegistry(type_registry.clone()));

    // First pass: spawn empty entities (Name/Transform/Visibility come through components)
    let mut spawned: Vec<Entity> = Vec::new();
    for _jsn in entities {
        let entity = world.spawn_empty();
        spawned.push(entity.id());
    }

    // Second pass: set parents (ChildOf)
    for (i, jsn) in entities.iter().enumerate() {
        if let Some(parent_idx) = jsn.parent
            && let Some(&parent_entity) = spawned.get(parent_idx)
        {
            world.entity_mut(spawned[i]).insert(ChildOf(parent_entity));
        }
    }

    // Third pass: deserialize extensible components via reflection
    let registry = type_registry.read();
    for (i, jsn) in entities.iter().enumerate() {
        for (type_path, value) in &jsn.components {
            let Some(registration) = registry.get_with_type_path(type_path) else {
                warn!("Unknown type '{type_path}', skipping");
                continue;
            };
            if registration.data::<ReflectComponent>().is_none() {
                continue;
            }
            let deserializer = TypedReflectDeserializer::new(registration, &registry);
            let Ok(reflected) = deserializer.deserialize(value) else {
                warn!("Failed to deserialize '{type_path}', skipping");
                continue;
            };
            world.entity_mut(spawned[i]).insert_reflect(reflected);
        }
    }
    drop(registry);

    // Extract all spawned entities into a DynamicScene
    let scene = DynamicSceneBuilder::from_world(&world)
        .extract_entities(spawned.into_iter())
        .build();

    Ok(scene)
}

#[derive(Debug, thiserror::Error)]
pub enum JsnLoadError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("Parse error: {0}")]
    Parse(String),
    #[error("Scene deserialization error: {0}")]
    Scene(String),
}
