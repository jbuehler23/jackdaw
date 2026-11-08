use bevy::prelude::*;
use jackdaw_bsn::SceneBsnAst;
use std::collections::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use crate::prefab::canonical_path::{CanonicalPrefabPath, canonical_prefab_path};
use crate::prefab::resolver_bsn::{Baseline, ISA_TYPE, PrefabBaseline, read_isa_source};

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
    entries: HashMap<CanonicalPrefabPath, CachedPrefab>,
    epoch: u64,
    /// Every prefab changed since the changes were last answered, with the
    /// epoch its change moved the cache to.
    changes: Vec<(u64, CanonicalPrefabPath)>,
    last_saved_fingerprints: HashMap<CanonicalPrefabPath, SavedFingerprint>,
    /// Puts each entry in the form live values take, so a value compared
    /// against a prefab read from an older file reads as an override only if
    /// it is one. Without it entries stay as they were read.
    registry: Option<AppTypeRegistry>,
}

/// A cached prefab and the baseline its instances compare against, built on
/// first use and dropped whenever any prefab changes, since it reads the
/// prefabs this one inherits from too.
struct CachedPrefab {
    ast: SceneBsnAst,
    baseline: OnceLock<PrefabBaseline>,
}

impl PrefabAstCache {
    /// A cache whose entries compare against the values this registry's types
    /// write.
    pub fn with_registry(registry: AppTypeRegistry) -> Self {
        Self {
            registry: Some(registry),
            ..Self::default()
        }
    }

    pub fn get(&self, path: &Path) -> Option<&SceneBsnAst> {
        self.get_canonical(&canonical_prefab_path(path))
    }

    pub fn get_canonical(&self, path: &CanonicalPrefabPath) -> Option<&SceneBsnAst> {
        self.entries.get(path).map(|entry| &entry.ast)
    }

    /// The baseline an instance of the prefab at `path` compares against: the
    /// prefab with the prefabs it inherits from expanded into it.
    pub fn baseline(&self, path: &Path) -> Option<Baseline<'_>> {
        let entry = self.entries.get(&canonical_prefab_path(path))?;
        let baseline = entry.baseline.get_or_init(|| {
            let get_prefab = |p: &Path| self.get(p);
            PrefabBaseline::new(&entry.ast, &get_prefab)
        });
        Some(baseline.view(&entry.ast))
    }

    /// The baseline entry an instance node numbered `id` inherits from in the
    /// prefab at `path`, with the document holding it.
    pub fn baseline_entry(&self, path: &Path, id: u32) -> Option<(&SceneBsnAst, Entity)> {
        let baseline = self.baseline(path)?;
        Some((baseline.document(), baseline.node(id)?))
    }

    pub fn insert(&mut self, path: impl AsRef<Path>, mut ast: SceneBsnAst) {
        if let Some(registry) = &self.registry {
            jackdaw_bsn::compact_document(&mut ast, &registry.read());
        }
        let key = canonical_prefab_path(path);
        self.entries.insert(
            key.clone(),
            CachedPrefab {
                ast,
                baseline: OnceLock::new(),
            },
        );
        self.note_change(key);
    }

    /// In-place mutation. Bumps the epoch. Returns `false` if no entry
    /// existed at this path (and does not invoke `mutator`).
    pub fn mutate<F: FnOnce(&mut SceneBsnAst)>(&mut self, path: &Path, mutator: F) -> bool {
        let key = canonical_prefab_path(path);
        let Some(entry) = self.entries.get_mut(&key) else {
            return false;
        };
        mutator(&mut entry.ast);
        self.note_change(key);
        true
    }

    /// Whether `source` is `ancestor` or inherits from it through the `IsA`
    /// chains of the cached prefabs, at its root or in a nested instance.
    pub fn inherits_from(&self, source: &Path, ancestor: &CanonicalPrefabPath) -> bool {
        let mut stack = vec![canonical_prefab_path(source)];
        let mut seen = Vec::new();
        while let Some(current) = stack.pop() {
            if current == *ancestor {
                return true;
            }
            if seen.contains(&current) {
                continue;
            }
            if let Some(prefab) = self.get_canonical(&current) {
                for node in prefab.entities_with_component(ISA_TYPE) {
                    if let Some(next) = read_isa_source(prefab, node) {
                        stack.push(canonical_prefab_path(next));
                    }
                }
            }
            seen.push(current);
        }
        false
    }

    pub fn invalidate(&mut self, path: &Path) {
        let key = canonical_prefab_path(path);
        if self.entries.remove(&key).is_some() {
            self.note_change(key);
        }
    }

    fn note_change(&mut self, key: CanonicalPrefabPath) {
        for entry in self.entries.values_mut() {
            entry.baseline.take();
        }
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
