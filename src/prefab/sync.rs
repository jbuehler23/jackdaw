//! Drives the resolver + scene respawn whenever the prefab cache
//! mutates. Replaces the previous ad-hoc propagation chain that
//! pushed updates from `apply_to_prefab_source` out to each open tab
//! by walking `Scenes.tabs`.

use bevy::prelude::*;

use crate::prefab::cache::PrefabAstCache;

/// Last cache epoch we acted on. Bumped after every reactive resolve.
#[derive(Resource, Default, Debug)]
pub struct LastResolvedEpoch(pub u64);

/// Re-resolve the active scene whenever the prefab cache's epoch
/// advances. Scene-AST edits that don't touch the cache go through
/// their own respawn path (operators call `reload_all_instances`
/// directly); this driver only reacts to cache mutations.
pub fn drive_respawn_on_prefab_cache_change(world: &mut World) {
    let current = world.resource::<PrefabAstCache>().epoch();
    let last = world.resource::<LastResolvedEpoch>().0;
    if current == last {
        return;
    }

    crate::prefab::watcher::reload_all_instances(world);

    world.resource_mut::<LastResolvedEpoch>().0 = current;

    bevy::log::debug!("prefab cache epoch {last} -> {current}: resolved + respawned active scene");
}
