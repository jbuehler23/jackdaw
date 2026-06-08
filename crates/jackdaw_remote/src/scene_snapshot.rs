use bevy::{ecs::reflect::AppTypeRegistry, prelude::*, remote::BrpResult};
use jackdaw_pie_protocol::build_snapshot;
use serde_json::Value;

pub use jackdaw_pie_protocol::RemoteEntity;

/// BRP handler for `jackdaw/scene_snapshot`.
/// Returns all `Transform`-bearing entities serialized as `Vec<RemoteEntity>`.
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
