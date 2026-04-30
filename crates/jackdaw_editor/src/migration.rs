//! Hot-reload state migration for the game `SubApp` world.
//!
//! Captures the `SubApp`'s reflect-registered components into a
//! `DynamicScene` keyed by `TypePath`, then writes them back into
//! the post-swap `SubApp` world. Component state survives reload as
//! long as the type's `TypePath` survives.
//!
//! Migration is via Bevy's reflection system (string-based identity
//! via `TypePath`), not `TypeId`, so it survives dlopen swaps where
//! `TypeId` would diverge.

use bevy::ecs::entity::EntityHashMap;
use bevy::prelude::*;
use bevy::scene::{DynamicScene, DynamicSceneBuilder, SceneFilter};

#[derive(Resource)]
pub struct MigrationSnapshot {
    pub scene: DynamicScene,
}

/// Walks every entity in `sub_world`, builds a `DynamicScene` capturing
/// every reflect-registered component on each. Returns the snapshot so
/// it can be applied after the dlopen swap.
pub fn capture_subapp_snapshot(sub_world: &mut World) -> MigrationSnapshot {
    // `World` in Bevy 0.18 doesn't expose a direct `iter_entities`
    // helper; the canonical way to enumerate every live entity is a
    // bare `Entity` query. This matches the pattern used elsewhere
    // (e.g. `extract_scene_entities`).
    let entities: Vec<Entity> = sub_world.query::<Entity>().iter(sub_world).collect();
    let scene = DynamicSceneBuilder::from_world(sub_world)
        .with_component_filter(SceneFilter::allow_all())
        .extract_entities(entities.into_iter())
        .build();
    MigrationSnapshot { scene }
}

/// Writes a captured snapshot into a freshly-rebuilt `SubApp` world.
/// Components whose `TypePath` exists in the new world's type registry
/// migrate; types not in the new registry are dropped silently by Bevy's
/// scene writer (no panic).
pub fn apply_subapp_snapshot(snapshot: MigrationSnapshot, sub_world: &mut World) {
    let mut entity_map: EntityHashMap<Entity> = EntityHashMap::default();
    if let Err(err) = snapshot.scene.write_to_world(sub_world, &mut entity_map) {
        warn!("hot-reload migration: scene write failed: {err}");
    }
}
