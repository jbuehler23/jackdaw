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
        let canonical = dunce::canonicalize(&path).unwrap_or(path);
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
                    dunce::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()) == path
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

        // Capture the live document's sparse form BEFORE the cache updates:
        // the sparsify pass strips values still matching the baseline the
        // scene was last resolved against, so it must compare against the
        // old prefab. Comparing against the new one would misread every
        // changed inherited value as an authored override.
        let sparse_text = capture_sparse_scene_text(world);

        match crate::prefab::save_load::read_prefab_ast(&path) {
            Ok(new_ast) => {
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
        }
        if let Some(sparse_text) = &sparse_text {
            respawn_from_sparse_text(world, sparse_text);
        }
    }
}

/// Emit the live BSN document to sparse text against the CURRENT prefab
/// cache: inherited descendants reduce to override entries and instance
/// roots shed values still matching their baselines. `None` when there is
/// no live document.
pub fn capture_sparse_scene_text(world: &mut World) -> Option<String> {
    world.get_resource::<jackdaw_bsn::SceneBsnAst>()?;
    // Inline runtime assets are embedded so material handles resolve across
    // a despawn + respawn.
    let parent_path = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|r| r.root.clone())
        .unwrap_or_else(|| PathBuf::from("."));
    Some(crate::scene_io::emit_bsn_scene_with_inline_assets(
        world,
        &parent_path,
    ))
}

/// Re-resolve every `IsA` instance in the live BSN document against the
/// prefab cache, despawn the existing preview entities, and respawn the
/// scene from the resolved document.
///
/// The live [`jackdaw_bsn::SceneBsnAst`] is a full linked document (one node
/// per spawned entity). Its authored mutations (a new instance root, an
/// override on an inherited descendant) are captured by emitting it to sparse
/// text, which the resolver reads back and expands: new `IsA` nodes
/// materialize their prefab subtrees, and inherited values re-derive from the
/// cached baselines. `load_bsn_scene` then reinstalls the resolved document
/// as the live source of truth.
pub fn reload_all_instances(world: &mut World) {
    let Some(sparse_text) = capture_sparse_scene_text(world) else {
        return;
    };
    respawn_from_sparse_text(world, &sparse_text);
}

/// Resolve `sparse_text` against the prefab cache and respawn the scene from
/// the resolved document, preserving undo history and selection-independent
/// editor state.
pub fn respawn_from_sparse_text(world: &mut World, sparse_text: &str) {
    // Parse the sparse form back and resolve every `IsA` instance against
    // the prefab cache.
    let resolved_text = {
        let authored = match jackdaw_bsn::parse_bsn_text(sparse_text) {
            Ok(a) => a,
            Err(e) => {
                warn!("reload_all_instances: parse failed: {e}");
                return;
            }
        };
        let cache = world.resource::<PrefabAstCache>();
        let get_prefab = |p: &Path| cache.get(p);
        match crate::prefab::resolver_bsn::resolve_scene(&authored, &get_prefab) {
            Ok(resolved) => jackdaw_bsn::emit_scene(&resolved),
            Err(e) => {
                warn!("reload_all_instances: resolver failed: {e}");
                return;
            }
        }
    };

    // Inlined despawn + clear that preserves `CommandHistory`.
    // `clear_scene_entities` truncates the undo stack (intended for
    // scene-file loads); prefab reloads must not lose history pushed
    // before this point or undo of preceding operators breaks.
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

    // Respawn from the resolved document. `load_bsn_scene` installs the
    // resolved document as the live `SceneBsnAst`, re-adds inline assets, and
    // applies patches, producing a full linked document once more.
    if let Err(err) = jackdaw_bsn::load_bsn_scene(world, &resolved_text) {
        bevy::log::error!("reload_all_instances: load_bsn_scene failed: {err}");
        return;
    }

    // History is preserved across the inline despawn above, so the
    // dirty-state baselines stay valid and the status bar / per-tab
    // dirty dot keep tracking the correct undo depth.

    // Rebuild the outliner from scratch. Observer-driven row creation
    // can fire mid-apply (Add<Transform> before IsA / Name land) and pin
    // the wrong category icon; a clean rebuild classifies every row
    // against the final archetype.
    if let Err(err) = world.run_system_cached(crate::hierarchy::clear_all_tree_rows) {
        bevy::log::warn!("reload_all_instances: clear_all_tree_rows failed: {err}");
    }
    if let Err(err) = crate::hierarchy::rebuild_hierarchy(world) {
        bevy::log::warn!("reload_all_instances: rebuild_hierarchy failed: {err}");
    }
}
