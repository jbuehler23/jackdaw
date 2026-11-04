use bevy::prelude::*;
use jackdaw_bsn::SceneBsnAst;
use std::collections::HashMap;
use std::path::Path;

use crate::prefab::canonical_path::{CanonicalPrefabPath, canonical_prefab_path};

/// Snapshot of a prefab file's identity at the moment the editor
/// wrote or read it. The watcher compares the current on-disk
/// fingerprint against this entry to recognise its own echoed write,
/// or the first look it takes when a watch starts, and skip the reload
/// that would otherwise clobber in-memory edits.
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
    /// Every prefab changed since the changes were last answered, with the
    /// epoch its change moved the cache to.
    changes: Vec<(u64, CanonicalPrefabPath)>,
    last_saved_fingerprints: HashMap<CanonicalPrefabPath, SavedFingerprint>,
}

impl PrefabAstCache {
    pub fn get(&self, path: &Path) -> Option<&SceneBsnAst> {
        self.entries.get(&canonical_prefab_path(path))
    }

    pub fn get_canonical(&self, path: &CanonicalPrefabPath) -> Option<&SceneBsnAst> {
        self.entries.get(path)
    }

    pub fn insert(&mut self, path: impl AsRef<Path>, ast: SceneBsnAst) {
        let key = canonical_prefab_path(path);
        self.entries.insert(key.clone(), ast);
        self.note_change(key);
    }

    /// In-place mutation. Bumps the epoch. Returns `false` if no entry
    /// existed at this path (and does not invoke `mutator`).
    pub fn mutate<F: FnOnce(&mut SceneBsnAst)>(&mut self, path: &Path, mutator: F) -> bool {
        let key = canonical_prefab_path(path);
        let Some(entry) = self.entries.get_mut(&key) else {
            return false;
        };
        mutator(entry);
        self.note_change(key);
        true
    }

    pub fn invalidate(&mut self, path: &Path) {
        let key = canonical_prefab_path(path);
        if self.entries.remove(&key).is_some() {
            self.note_change(key);
        }
    }

    fn note_change(&mut self, key: CanonicalPrefabPath) {
        self.epoch = self.epoch.wrapping_add(1);
        self.changes.push((self.epoch, key));
    }

    /// The prefabs changed since the changes were last answered.
    pub fn unanswered_changes(&self) -> impl Iterator<Item = &CanonicalPrefabPath> {
        self.changes.iter().map(|(_, path)| path)
    }

    /// Take every change so far as answered.
    pub fn answer_changes(&mut self) {
        self.changes.clear();
    }

    /// Take the changes past `epoch` as answered, leaving any from before it
    /// still to answer: a load that read those prefabs has already used them.
    pub fn answer_changes_after(&mut self, epoch: u64) {
        self.changes.retain(|(at, _)| *at <= epoch);
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

    /// Stash the post-write fingerprint of a prefab file. The watcher
    /// reads this back to decide whether an incoming filesystem event
    /// describes the editor's own write or a genuine external edit.
    pub fn record_saved_fingerprint(&mut self, path: &Path, fingerprint: SavedFingerprint) {
        let key = canonical_prefab_path(path);
        self.last_saved_fingerprints.insert(key, fingerprint);
    }

    /// Drop the fingerprint recorded for `path`, so the next event for it
    /// reads as an external edit.
    pub fn forget_saved_fingerprint(&mut self, path: &Path) {
        self.last_saved_fingerprints
            .remove(&canonical_prefab_path(path));
    }

    /// Record `fingerprint` for `path`, or forget the one it had when there
    /// is none to vouch for what the cache holds.
    pub fn settle_fingerprint(&mut self, path: &Path, fingerprint: Option<SavedFingerprint>) {
        match fingerprint {
            Some(fingerprint) => self.record_saved_fingerprint(path, fingerprint),
            None => self.forget_saved_fingerprint(path),
        }
    }

    /// Last fingerprint the editor recorded for `path`, if any.
    pub fn last_saved_fingerprint(&self, path: &Path) -> Option<&SavedFingerprint> {
        self.last_saved_fingerprints
            .get(&canonical_prefab_path(path))
    }
}

/// Run `read` over `path` and fingerprint the file it read. The fingerprint is
/// `None` when the file changed during the read: what `read` returned may be
/// either version, so no fingerprint describes it.
pub fn read_fingerprinted<T>(
    path: &Path,
    read: impl FnOnce() -> T,
) -> (T, Option<SavedFingerprint>) {
    let before = compute_file_fingerprint(path).ok();
    let value = read();
    let after = compute_file_fingerprint(path).ok();
    let fingerprint = before.filter(|before| after.as_ref() == Some(before));
    (value, fingerprint)
}

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
