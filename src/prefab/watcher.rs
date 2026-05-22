//! Filesystem watcher for prefab files. When a cached prefab changes
//! on disk, re-parse it into the cache and re-resolve the live scene
//! so inherited entities pick up the new values.

use bevy::prelude::*;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::prefab::cache::PrefabAstCache;

pub struct PrefabWatcherPlugin;

impl Plugin for PrefabWatcherPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PrefabWatchState>()
            .add_systems(Update, (refresh_watch_list, drain_changes).chain());
    }
}

#[derive(Resource, Default)]
struct PrefabWatchState {
    watcher: Option<RecommendedWatcher>,
    watched: Vec<PathBuf>,
    pending: Arc<Mutex<Vec<PathBuf>>>,
    debounced: Vec<(PathBuf, Instant)>,
}

const DEBOUNCE: Duration = Duration::from_millis(150);

fn refresh_watch_list(mut state: ResMut<PrefabWatchState>, cache: Res<PrefabAstCache>) {
    let current_paths: Vec<PathBuf> = cache.paths().map(PathBuf::from).collect();
    if current_paths == state.watched {
        return;
    }
    let pending = state.pending.clone();
    let mut new_watcher: RecommendedWatcher =
        match notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(ev) = res
                && matches!(
                    ev.kind,
                    EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
                )
                && let Ok(mut lock) = pending.lock()
            {
                for p in ev.paths {
                    lock.push(p);
                }
            }
        }) {
            Ok(w) => w,
            Err(e) => {
                warn!("prefab watcher init failed: {e}");
                return;
            }
        };
    let mut to_queue: Vec<PathBuf> = Vec::new();
    for p in &current_paths {
        if let Err(e) = new_watcher.watch(p, RecursiveMode::NonRecursive) {
            warn!("watch failed for {}: {}", p.display(), e);
        }
        // Queue a synthetic check for any path newly added to the watch
        // list. notify only reports events that happen after `watch()`
        // returns, so a file modified between cache insert and watcher
        // install would never trigger a reload otherwise.
        if !state.watched.contains(p) {
            to_queue.push(p.clone());
        }
    }
    if !to_queue.is_empty()
        && let Ok(mut lock) = state.pending.lock()
    {
        lock.extend(to_queue);
    }
    state.watcher = Some(new_watcher);
    state.watched = current_paths;
}

fn drain_changes(world: &mut World) {
    let (pending_paths, mut debounced) = {
        let state = world.resource::<PrefabWatchState>();
        let pending = match state.pending.lock() {
            Ok(mut lock) => lock.drain(..).collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };
        let debounced_now = state.debounced.clone();
        (pending, debounced_now)
    };
    let now = Instant::now();
    for path in pending_paths {
        let canonical = path.canonicalize().unwrap_or(path);
        debounced.push((canonical, now));
    }
    let mut to_reload: Vec<PathBuf> = Vec::new();
    debounced.retain(|(p, t)| {
        if now.duration_since(*t) >= DEBOUNCE {
            to_reload.push(p.clone());
            false
        } else {
            true
        }
    });
    world.resource_mut::<PrefabWatchState>().debounced = debounced;

    for path in to_reload {
        // Match against the canonicalized form of any cached path so an
        // event for a symlinked or non-canonical write still updates the
        // entry the resolver looks up.
        let cache_key = {
            let cache = world.resource::<PrefabAstCache>();
            cache
                .paths()
                .find(|p| {
                    p.canonicalize().unwrap_or_else(|_| p.to_path_buf()) == path
                        || *p == path.as_path()
                })
                .map(PathBuf::from)
                .unwrap_or_else(|| path.clone())
        };

        // If the current file matches the fingerprint of what the
        // editor last wrote, this event is our own echo. Skipping
        // avoids clobbering further in-memory edits that landed
        // between the save and the watcher firing. A failure to read
        // the current fingerprint (file deleted, permissions) is
        // treated as "not our write" so the existing reload path can
        // report the error.
        let our_write = match (
            crate::prefab::cache::compute_file_fingerprint(&path).ok(),
            world
                .resource::<PrefabAstCache>()
                .last_saved_fingerprint(&path),
        ) {
            (Some(curr), Some(saved)) => &curr == saved,
            _ => false,
        };
        if our_write {
            continue;
        }

        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<jackdaw_jsn::format::JsnScene>(&text) {
                Ok(scene) => {
                    let new_ast = jackdaw_jsn::SceneJsnAst::from_jsn_scene(&scene, &[]);
                    world
                        .resource_mut::<PrefabAstCache>()
                        .insert(cache_key.clone(), new_ast);
                }
                Err(e) => {
                    warn!("prefab reload parse failed for {}: {e}", path.display());
                    world
                        .resource_mut::<PrefabAstCache>()
                        .invalidate(&cache_key);
                    continue;
                }
            },
            Err(e) => {
                warn!("prefab reload read failed for {}: {e}", path.display());
                world
                    .resource_mut::<PrefabAstCache>()
                    .invalidate(&cache_key);
                continue;
            }
        }
        reload_instances_for_prefab(world, &cache_key);
    }
}

fn reload_instances_for_prefab(world: &mut World, _prefab_path: &Path) {
    reload_all_instances(world);
}

/// Re-resolve every `IsA` instance in the live scene against the
/// prefab cache, despawn the existing preview entities, and respawn
/// the scene from the resolved AST. The unresolved (authored) AST is
/// reinstalled as the source of truth so subsequent edits target the
/// instance roots, not the materialized inherited entities.
pub fn reload_all_instances(world: &mut World) {
    let unresolved_ast = world.resource::<jackdaw_jsn::SceneJsnAst>().clone();
    let resolved = {
        let cache = world.resource::<PrefabAstCache>();
        match crate::prefab::resolver::resolve_scene(&unresolved_ast, cache) {
            Ok(a) => a,
            Err(e) => {
                warn!("reload_all_instances: resolver failed: {e}");
                return;
            }
        }
    };
    let jsn = crate::scene_io::jsn_scene_from_ast(&resolved);
    // Inline the despawn + ast/tree/selection clears, but PRESERVE
    // `CommandHistory`. `clear_scene_entities` truncates the undo
    // stack (intended for scene-file loads); we don't want that here
    // because the operator framework relies on pushing a SnapshotDiff
    // AFTER reload_all_instances finishes, and a fresh push to a
    // history that just got cleared still works, but any history
    // accumulated BEFORE the call (e.g. from prior operators or
    // saved-state) would be lost on every prefab reload.
    world.resource_mut::<jackdaw_jsn::SceneJsnAst>().clear();
    world
        .resource_mut::<crate::selection::Selection>()
        .entities
        .clear();
    if let Err(err) = world.run_system_cached(crate::hierarchy::clear_all_tree_rows) {
        bevy::log::error!("reload_all_instances: clear_all_tree_rows failed: {err}");
    }
    if let Err(err) = crate::scene_io::despawn_scene_entities(world) {
        bevy::log::error!("reload_all_instances: despawn_scene_entities failed: {err}");
    }
    let parent = std::path::Path::new(".");
    let local = std::collections::HashMap::new();
    let spawned = crate::scene_io::load_scene_from_jsn(world, &jsn.scene, parent, &local);

    // Re-install the unresolved AST as the source of truth, rebinding
    // the first N spawned entities (the authored ones) to their AST
    // node indices. Inherited entities live ECS-only until edited.
    let authored_count = unresolved_ast.nodes.len();
    let mut new_ast = unresolved_ast;
    for (i, node) in new_ast.nodes.iter_mut().enumerate().take(authored_count) {
        node.ecs_entity = spawned.get(i).copied();
    }
    new_ast.ecs_to_jsn = new_ast
        .nodes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| n.ecs_entity.map(|e| (e, i)))
        .collect();
    *world.resource_mut::<jackdaw_jsn::SceneJsnAst>() = new_ast;

    // History is preserved across the inline despawn above, so the
    // dirty-state baselines stay valid and the status bar / per-tab
    // dirty dot keep tracking the correct undo depth.

    // Force-rebuild the outliner. The observer-driven row creation can
    // fire from a transient archetype during `insert_reflect` (Add<Transform>
    // dispatches before IsA / Name land), causing classify_entity to pick
    // the wrong category and pin the wrong icon. Clear + rebuild guarantees
    // every row is classified against the final, fully-populated archetype.
    if let Err(err) = world.run_system_cached(crate::hierarchy::clear_all_tree_rows) {
        bevy::log::warn!("reload_all_instances: clear_all_tree_rows failed: {err}");
    }
    if let Err(err) = crate::hierarchy::rebuild_hierarchy(world) {
        bevy::log::warn!("reload_all_instances: rebuild_hierarchy failed: {err}");
    }
}
