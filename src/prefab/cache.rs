use bevy::prelude::*;
use jackdaw_bsn::SceneBsnAst;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::prefab::canonical_path::{CanonicalPrefabPath, canonical_prefab_path};

/// Snapshot of a prefab file's identity at the moment the editor
/// wrote it. The watcher compares the current on-disk fingerprint
/// against this entry to recognise its own echoed write event and
/// skip the reload that would otherwise clobber in-memory edits
/// landing between the save and the watcher firing.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedFingerprint {
    pub mtime: std::time::SystemTime,
    pub content_hash: u64,
}

/// Parsed prefab ASTs keyed by canonical path. The resolver reads
/// from here when an `IsA` reference needs to be expanded. Every
/// mutation bumps `epoch` so on-change systems can detect work
/// without diffing the whole map.
#[derive(Resource, Default)]
pub struct PrefabAstCache {
    entries: HashMap<CanonicalPrefabPath, SceneBsnAst>,
    epoch: u64,
    dirty_paths: HashSet<CanonicalPrefabPath>,
    last_saved_fingerprints: HashMap<CanonicalPrefabPath, SavedFingerprint>,
}

impl PrefabAstCache {
    pub fn get(&self, path: &Path) -> Option<&SceneBsnAst> {
        self.entries.get(&canonical_prefab_path(path))
    }

    pub fn get_canonical(&self, path: &CanonicalPrefabPath) -> Option<&SceneBsnAst> {
        self.entries.get(path)
    }

    /// A JSON-AST view of the cached prefab at `path`, produced by emitting
    /// the stored BSN document and bridging it back through the `.jsn`
    /// scene format. The JSON-scene resolution paths (legacy `.jsn` scene
    /// open, tab swap, undo restore) consume this so they can keep merging
    /// prefab inheritance while the cache stores BSN documents.
    pub fn get_as_jsn(&self, path: &Path) -> Option<jackdaw_jsn::SceneJsnAst> {
        self.get(path).map(bsn_ast_to_jsn_ast)
    }

    /// [`Self::get_as_jsn`] keyed by an already-canonical path.
    pub fn get_canonical_as_jsn(
        &self,
        path: &CanonicalPrefabPath,
    ) -> Option<jackdaw_jsn::SceneJsnAst> {
        self.get_canonical(path).map(bsn_ast_to_jsn_ast)
    }

    pub fn insert(&mut self, path: impl AsRef<Path>, ast: SceneBsnAst) {
        let key = canonical_prefab_path(path);
        self.entries.insert(key.clone(), ast);
        self.epoch = self.epoch.wrapping_add(1);
        self.dirty_paths.insert(key);
    }

    /// In-place mutation. Bumps the epoch and marks the path dirty.
    /// Returns `false` if no entry existed at this path (and does not
    /// invoke `mutator`).
    pub fn mutate<F: FnOnce(&mut SceneBsnAst)>(&mut self, path: &Path, mutator: F) -> bool {
        let key = canonical_prefab_path(path);
        let Some(entry) = self.entries.get_mut(&key) else {
            return false;
        };
        mutator(entry);
        self.epoch = self.epoch.wrapping_add(1);
        self.dirty_paths.insert(key);
        true
    }

    pub fn invalidate(&mut self, path: &Path) {
        let key = canonical_prefab_path(path);
        if self.entries.remove(&key).is_some() {
            self.epoch = self.epoch.wrapping_add(1);
            self.dirty_paths.insert(key);
        }
    }

    pub fn paths(&self) -> impl Iterator<Item = &Path> {
        self.entries.keys().map(CanonicalPrefabPath::as_path)
    }

    /// Monotonically-increasing version counter. Bumped on insert /
    /// mutate / invalidate. Consumers compare against their last-seen
    /// epoch to decide whether to react.
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Paths that have changed since `dirty_paths` was last cleared.
    /// Read-only view; clear with `clear_dirty`.
    pub fn dirty_paths(&self) -> impl Iterator<Item = &CanonicalPrefabPath> {
        self.dirty_paths.iter()
    }

    pub fn clear_dirty(&mut self) {
        self.dirty_paths.clear();
    }

    /// Stash the post-write fingerprint of a prefab file. The watcher
    /// reads this back to decide whether an incoming filesystem event
    /// describes the editor's own write or a genuine external edit.
    pub fn record_saved_fingerprint(&mut self, path: &Path, fingerprint: SavedFingerprint) {
        let key = canonical_prefab_path(path);
        self.last_saved_fingerprints.insert(key, fingerprint);
    }

    /// Last fingerprint the editor recorded for `path`, if any.
    pub fn last_saved_fingerprint(&self, path: &Path) -> Option<&SavedFingerprint> {
        self.last_saved_fingerprints
            .get(&canonical_prefab_path(path))
    }
}

/// Bridge a cached BSN prefab document into a `SceneJsnAst`. Emits the
/// document to `.bsn` text, parses it back through the `.jsn` bridge, and
/// rebuilds a JSON AST. Node ids are re-minted by `from_jsn_scene`; that is
/// fine because the JSON resolver only consumes prefab documents as
/// read-only inheritance sources.
pub fn bsn_ast_to_jsn_ast(ast: &SceneBsnAst) -> jackdaw_jsn::SceneJsnAst {
    let text = jackdaw_bsn::emit_scene(ast);
    match jackdaw_jsn::bsn_bridge::bsn_scene_to_jsn(&text) {
        Ok(scene) => jackdaw_jsn::SceneJsnAst::from_jsn_scene(&scene, &[]),
        Err(err) => {
            warn!("bsn_ast_to_jsn_ast: bridge failed: {err}");
            jackdaw_jsn::SceneJsnAst::default()
        }
    }
}

/// Read `path`'s metadata + bytes and produce a `SavedFingerprint`.
/// Used by save paths to record what they just wrote and by the
/// watcher to decide whether an event describes our own echo.
pub fn compute_file_fingerprint(path: &Path) -> std::io::Result<SavedFingerprint> {
    use std::hash::{Hash, Hasher};
    let metadata = std::fs::metadata(path)?;
    let mtime = metadata.modified()?;
    let bytes = std::fs::read(path)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    Ok(SavedFingerprint {
        mtime,
        content_hash: hasher.finish(),
    })
}
