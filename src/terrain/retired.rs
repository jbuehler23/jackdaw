//! Terrain undo entries outliving the entity they were recorded against.
//!
//! An undo entry for a sculpt, paint or channel edit names the terrain by its
//! entity, and a whole-scene respawn (a snapshot undo, a prefab reload) mints
//! a new one. The sidecar path survives the respawn, so an entry finds the
//! terrain that now holds the same data.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<RetiredTerrains>()
        .add_observer(retire_terrain);
}

/// The sidecar path each despawned terrain held.
#[derive(Resource, Default)]
struct RetiredTerrains(HashMap<Entity, String>);

fn retire_terrain(
    removed: On<Remove, jackdaw_scene_types::Terrain>,
    terrains: Query<&jackdaw_scene_types::Terrain>,
    mut retired: ResMut<RetiredTerrains>,
) {
    let entity = removed.event_target();
    if let Ok(terrain) = terrains.get(entity)
        && !terrain.data_path.is_empty()
    {
        retired.0.insert(entity, terrain.data_path.clone());
    }
}

/// The terrain an undo entry recorded as `entity`: that entity while it is
/// still a terrain, otherwise the terrain now holding the data it held.
pub(crate) fn live_terrain(world: &mut World, entity: Entity) -> Option<Entity> {
    if world.get::<jackdaw_scene_types::Terrain>(entity).is_some() {
        return Some(entity);
    }
    let data_path = world
        .get_resource::<RetiredTerrains>()?
        .0
        .get(&entity)?
        .clone();
    world
        .query::<(Entity, &jackdaw_scene_types::Terrain)>()
        .iter(world)
        .find(|(_, terrain)| terrain.data_path == data_path)
        .map(|(entity, _)| entity)
}
