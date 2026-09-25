//! Drives the resolver + scene respawn whenever the prefab cache
//! mutates. Replaces the previous ad-hoc propagation chain that
//! pushed updates from `apply_to_prefab_source` out to each open tab
//! by walking `Scenes.tabs`.

use bevy::prelude::*;

use std::collections::HashSet;

use crate::prefab::cache::PrefabAstCache;
use crate::prefab::canonical_path::{CanonicalPrefabPath, canonical_prefab_path};

/// Last cache epoch we acted on. Bumped after every reactive resolve.
#[derive(Resource, Default, Debug)]
pub struct LastResolvedEpoch(pub u64);

/// Re-resolve the active scene whenever a prefab it draws from changes.
///
/// Scene-AST edits that don't touch the cache go through their own respawn
/// path (operators call `reload_all_instances` directly); this driver only
/// reacts to cache mutations. A prefab cached for something else, such as a
/// scatter palette entry, costs the scene nothing: respawning it would throw
/// away and rebuild every entity, and the undo history with them, for a
/// document that reads the same.
pub fn drive_respawn_on_prefab_cache_change(world: &mut World) {
    let current = world.resource::<PrefabAstCache>().epoch();
    let last = world.resource::<LastResolvedEpoch>().0;
    if current == last {
        return;
    }

    if scene_draws_from_a_changed_prefab(world) {
        crate::prefab::watcher::reload_all_instances(world);
        bevy::log::debug!(
            "prefab cache epoch {last} -> {current}: resolved + respawned active scene"
        );
    }

    world.resource_mut::<PrefabAstCache>().answer_changes();
    world.resource_mut::<LastResolvedEpoch>().0 = current;
}

/// Whether any prefab changed since the last answer is one the live scene
/// draws from, directly or through the prefabs those draw from. With no live
/// document to read, every change counts.
fn scene_draws_from_a_changed_prefab(world: &World) -> bool {
    let Some(live) = world.get_resource::<jackdaw_bsn::SceneBsnAst>() else {
        return true;
    };
    let cache = world.resource::<PrefabAstCache>();
    let drawn = prefabs_drawn_from(live, cache);
    cache.unanswered_changes().any(|path| drawn.contains(path))
}

/// Every prefab `document` draws from, following each one the cache holds into
/// the prefabs it draws from in turn.
fn prefabs_drawn_from(
    document: &jackdaw_bsn::SceneBsnAst,
    cache: &PrefabAstCache,
) -> HashSet<CanonicalPrefabPath> {
    let sources = |ast: &jackdaw_bsn::SceneBsnAst| -> Vec<std::path::PathBuf> {
        ast.entities_with_component(jackdaw_prefab::ISA_TYPE)
            .into_iter()
            .filter_map(|node| jackdaw_prefab::read_isa_source(ast, node))
            .collect()
    };
    let mut drawn = HashSet::new();
    let mut pending = sources(document);
    while let Some(path) = pending.pop() {
        let key = canonical_prefab_path(&path);
        if let Some(prefab) = cache.get_canonical(&key)
            && !drawn.contains(&key)
        {
            pending.extend(sources(prefab));
        }
        drawn.insert(key);
    }
    drawn
}
