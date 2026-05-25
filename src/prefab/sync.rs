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

    let dirty_paths: Vec<crate::prefab::CanonicalPrefabPath> = {
        let cache = world.resource::<PrefabAstCache>();
        cache.dirty_paths().cloned().collect()
    };

    crate::prefab::watcher::reload_all_instances(world);

    world.resource_mut::<LastResolvedEpoch>().0 = current;
    world.resource_mut::<PrefabAstCache>().clear_dirty();

    if !dirty_paths.is_empty() {
        bevy::log::debug!(
            "prefab cache change ({} paths) -> resolved + respawned active scene",
            dirty_paths.len()
        );
    }
}
