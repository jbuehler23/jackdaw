//! Filesystem watcher for prefab files. When a cached prefab changes
//! on disk, re-parse it into the cache and re-resolve the live scene
//! so inherited entities pick up the new values.

use bevy::prelude::*;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::prefab::cache::PrefabAstCache;
use crate::prefab::resolver_bsn::ISA_TYPE;

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

/// Every prefab file to watch, taken from the open documents rather than from
/// the cache.
///
/// Deriving the list from the cache alone loses a file as soon as it stops
/// parsing: the failed reload drops the entry, the path falls off the watch
/// list, and the edit that repairs the file goes unnoticed. The scene's `IsA`
/// sources and the open prefab tabs survive a parse failure, so a broken prefab
/// stays watched until nothing refers to it.
fn prefab_paths_to_watch(
    cache: &PrefabAstCache,
    live: Option<&jackdaw_bsn::SceneBsnAst>,
    scenes: Option<&crate::scenes::Scenes>,
) -> Vec<PathBuf> {
    let mut paths: Vec<PathBuf> = cache.paths().map(PathBuf::from).collect();
    let mut add = |path: PathBuf| {
        if !paths.contains(&path) {
            paths.push(path);
        }
    };
    if let Some(live) = live {
        for node in live.entities_with_component(ISA_TYPE) {
            if let Some(source) = crate::prefab::resolver_bsn::read_isa_source(live, node) {
                add(source);
            }
        }
    }
    if let Some(scenes) = scenes {
        for tab in &scenes.tabs {
            if matches!(tab.kind, crate::scenes::TabKind::Prefab)
                && let Some(path) = tab.path.as_ref()
            {
                add(path.clone());
            }
        }
    }
    paths
}

/// Paths an open prefab tab holds. A write to one of those is an edit to an
/// open document, which `crate::scenes::external_watch` prompts about; handling
/// it here as well would apply the edit twice.
fn open_prefab_tab_paths(world: &World) -> Vec<PathBuf> {
    let Some(scenes) = world.get_resource::<crate::scenes::Scenes>() else {
        return Vec::new();
    };
    scenes
        .tabs
        .iter()
        .filter(|tab| matches!(tab.kind, crate::scenes::TabKind::Prefab))
        .filter_map(|tab| tab.path.as_ref())
        .map(|path| dunce::canonicalize(path).unwrap_or_else(|_| path.clone()))
        .collect()
}

fn refresh_watch_list(
    mut state: ResMut<PrefabWatchState>,
    cache: Res<PrefabAstCache>,
    live: Option<Res<jackdaw_bsn::SceneBsnAst>>,
    scenes: Option<Res<crate::scenes::Scenes>>,
) {
    // Walking the live document's `IsA` nodes costs a query per call, and the
    // result can only differ when one of the three inputs changed. A watcher
    // that never installed counts as stale.
    let stale = state.watcher.is_none()
        || cache.is_changed()
        || live.as_ref().is_some_and(DetectChanges::is_changed)
        || scenes.as_ref().is_some_and(DetectChanges::is_changed);
    if !stale {
        return;
    }
    let current_paths = prefab_paths_to_watch(&cache, live.as_deref(), scenes.as_deref());
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

    let open_prefab_tabs = open_prefab_tab_paths(world);
    let mut reloaded = false;
    for path in to_reload {
        // An open prefab is an edited document, not only a source this scene
        // reads. The external-scene watcher prompts about it, so applying the
        // edit here would pre-empt that prompt.
        if open_prefab_tabs.contains(&path) {
            continue;
        }

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
                // Keep the last copy that parsed. Dropping it would leave the
                // next capture with no baseline to sparsify against, rewriting
                // every inherited value in the scene as an authored override,
                // which a later valid version of the file could not undo since
                // overrides win over inherited values.
                warn!(
                    "prefab reload parse failed for {}: {e}; keeping the last \
                     copy that parsed",
                    path.display()
                );
                continue;
            }
        }
        if let Some(sparse_text) = &sparse_text {
            match reload_instances_of(world, sparse_text, &cache_key) {
                PrefabReload::Instances => reloaded = true,
                PrefabReload::Unhandled => {
                    respawn_scene(world, sparse_text);
                    reloaded = true;
                }
                PrefabReload::Nothing => {}
            }
        }
    }

    // Once for the batch: a rebuild per changed file would walk the whole
    // scene again for each one, and only the last walk's rows survive.
    if reloaded {
        rebuild_outliner(world);
    }
}

/// What a per-file reload did, which is what decides whether the caller
/// has anything left to do.
pub enum PrefabReload {
    /// The file's instances were respawned in place; every other entity
    /// kept its id.
    Instances,
    /// Not something this path can do: nothing in the live document
    /// inherits from the file directly (a prefab another prefab
    /// references has no instance node of its own), or an instance is not
    /// a document root and does not line up with a resolved root. The
    /// caller respawns the whole scene.
    Unhandled,
    /// The new text did not parse or resolve. The scene is untouched and
    /// respawning it would only rebuild what is already there.
    Nothing,
}

/// Respawn the instances that inherit from `changed`, leaving every other
/// entity in place so its id, and whatever holds it, survives the reload.
pub fn reload_instances_of(world: &mut World, sparse_text: &str, changed: &Path) -> PrefabReload {
    let wanted = crate::prefab::canonical_prefab_path(changed);
    let registry = world.resource::<AppTypeRegistry>().clone();
    let live_roots: Vec<Entity> = {
        let reg = registry.read();
        let Some(live) = world.get_resource::<jackdaw_bsn::SceneBsnAst>() else {
            return PrefabReload::Unhandled;
        };
        jackdaw_bsn::entity_roots(live, &reg)
    };
    let instances: Vec<Entity> = {
        let live = world.resource::<jackdaw_bsn::SceneBsnAst>();
        live.entities_with_component(ISA_TYPE)
            .into_iter()
            .filter(|&node| {
                crate::prefab::resolver_bsn::read_isa_source(live, node)
                    .is_some_and(|source| crate::prefab::canonical_prefab_path(source) == wanted)
            })
            .collect()
    };
    // A prefab only another prefab file references has no instance node
    // here to respawn; the whole-scene path picks it up through the cache.
    if instances.is_empty() {
        return PrefabReload::Unhandled;
    }
    let at_root: Option<Vec<usize>> = instances
        .iter()
        .map(|node| live_roots.iter().position(|root| root == node))
        .collect();
    let Some(at_root) = at_root else {
        return PrefabReload::Unhandled;
    };

    let resolved = {
        let authored = match jackdaw_bsn::parse_bsn_text(sparse_text) {
            Ok(authored) => authored,
            Err(e) => {
                warn!("prefab reload: parse failed: {e}");
                return PrefabReload::Nothing;
            }
        };
        let cache = world.resource::<PrefabAstCache>();
        let get_prefab = |p: &Path| cache.get(p);
        match crate::prefab::resolver_bsn::resolve_scene(&authored, &get_prefab) {
            Ok(resolved) => resolved,
            Err(e) => {
                warn!("prefab reload: resolver failed: {e}");
                return PrefabReload::Nothing;
            }
        }
    };
    let resolved_roots = {
        let reg = registry.read();
        jackdaw_bsn::entity_roots(&resolved, &reg)
    };
    // The sparse emit keeps document order, so a root's index carries over.
    // Anything else means the two documents disagree about the scene's shape,
    // and a whole-scene respawn is the honest answer.
    let mut pairs = Vec::with_capacity(instances.len());
    for (&node, &index) in instances.iter().zip(&at_root) {
        let Some(&counterpart) = resolved_roots.get(index) else {
            return PrefabReload::Unhandled;
        };
        if crate::prefab::resolver_bsn::read_isa_source(&resolved, counterpart)
            .is_none_or(|source| crate::prefab::canonical_prefab_path(source) != wanted)
        {
            return PrefabReload::Unhandled;
        }
        pairs.push((node, counterpart));
    }

    for (node, counterpart) in pairs {
        let position = world
            .resource::<jackdaw_bsn::SceneBsnAst>()
            .roots
            .iter()
            .position(|&root| root == node);
        despawn_document_subtree(world, node);
        let fresh = {
            let mut live = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
            let fresh = graft_subtree(&resolved, counterpart, &mut live);
            match position {
                Some(position) if position <= live.roots.len() => {
                    live.roots.insert(position, fresh);
                }
                _ => live.add_to_roots(fresh),
            }
            fresh
        };
        let mut spawned = Vec::new();
        jackdaw_bsn::spawn_ast_node(world, fresh, None, &mut spawned);
    }
    jackdaw_bsn::apply_dirty_ast_patches(world);

    // An entity the reload despawned is out of the selection, and its
    // `Selected` marker went with it; re-selecting the survivors puts the
    // resource and the markers back in step.
    let alive: Vec<Entity> = world
        .resource::<crate::selection::Selection>()
        .entities
        .iter()
        .copied()
        .filter(|&entity| world.get_entity(entity).is_ok())
        .collect();
    crate::selection::select_many(world, &alive);
    PrefabReload::Instances
}

/// Despawn the entities a document subtree spawned and drop its nodes.
fn despawn_document_subtree(world: &mut World, root: Entity) {
    let entities: Vec<Entity> = {
        let live = world.resource::<jackdaw_bsn::SceneBsnAst>();
        std::iter::once(root)
            .chain(live.descendants_of(root))
            .filter_map(|node| live.ecs_for_ast(node))
            .collect()
    };
    // Every mapped entity, not only the first: a subtree whose root has no
    // ECS mapping would otherwise leave its descendants in the world with
    // nothing in the document naming them. Despawning a root takes its
    // children with it, so a later id in the list may already be gone.
    for &entity in &entities {
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }
    let mut live = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
    for entity in entities {
        live.remove_entity_node(entity);
    }
}

/// Copy `node` and its subtree from `from` into `into`, returning the new node.
fn graft_subtree(
    from: &jackdaw_bsn::SceneBsnAst,
    node: Entity,
    into: &mut jackdaw_bsn::SceneBsnAst,
) -> Entity {
    let grafted = into.create_entity_node(from.cloned_component_patches(node));
    for child in from.get_children_ast(node) {
        let child = graft_subtree(from, child, into);
        into.add_child_to_ast(grafted, child);
    }
    grafted
}

/// Rebuild the outliner from scratch. Observer-driven row creation can fire
/// mid-apply (`Add<Transform>` before `IsA` / `Name` land) and pin the wrong
/// category icon; a clean rebuild classifies every row against the final
/// archetype.
fn rebuild_outliner(world: &mut World) {
    if let Err(err) = world.run_system_cached(crate::hierarchy::clear_all_tree_rows) {
        bevy::log::warn!("prefab reload: clear_all_tree_rows failed: {err}");
    }
    if let Err(err) = crate::hierarchy::rebuild_hierarchy(world) {
        bevy::log::warn!("prefab reload: rebuild_hierarchy failed: {err}");
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
    respawn_scene(world, sparse_text);
    rebuild_outliner(world);
}

/// [`respawn_from_sparse_text`] without the outliner rebuild, for a caller
/// respawning more than once before the tree is read.
fn respawn_scene(world: &mut World, sparse_text: &str) {
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
    }

    // History is preserved across the inline despawn above, so the
    // dirty-state baselines stay valid and the status bar / per-tab
    // dirty dot keep tracking the correct undo depth.
}
