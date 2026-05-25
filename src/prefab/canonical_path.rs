//! Canonical-form path identity for prefab cache keys. Prefab files
//! get referenced through many forms (absolute, project-relative,
//! through a symlink, with `..` components). All callers must funnel
//! through `canonical_prefab_path` so the cache never holds two
//! entries that refer to the same file.

use std::path::{Path, PathBuf};

/// Canonical form of a prefab path, used as the only key into
/// `PrefabAstCache`. Constructed via `canonical_prefab_path`; never
/// instantiate from a raw `PathBuf` outside this module.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CanonicalPrefabPath(PathBuf);

impl CanonicalPrefabPath {
    pub fn as_path(&self) -> &Path {
        &self.0
    }

    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl AsRef<Path> for CanonicalPrefabPath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

/// Produce a `CanonicalPrefabPath` from any path-like input. Runs
/// `std::fs::canonicalize` when the file exists; falls back to a
/// best-effort normalization (absolutize + remove `.` / `..`) when
/// it does not. The fallback path matters because callers (e.g.
/// `spawn_instance` issued before the prefab is on disk) need stable
/// keys before the file lands.
pub fn canonical_prefab_path(path: impl AsRef<Path>) -> CanonicalPrefabPath {
    let p = path.as_ref();
    if let Ok(canon) = p.canonicalize() {
        return CanonicalPrefabPath(canon);
    }
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    };
    CanonicalPrefabPath(normalize(&absolute))
}

fn normalize(p: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}
