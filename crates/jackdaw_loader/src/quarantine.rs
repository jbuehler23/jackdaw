//! Startup quarantine: crash containment for in-process native code.
//!
//! Native code cannot be made crash-proof, so the loader contains the
//! blast radius instead: a sentinel file is written before each dylib
//! loads and removed once the load (and the session) survives. A
//! sentinel left behind means the process died with that dylib armed;
//! the next session sees it, refuses to load the dylib, and can tell
//! the user "Extension X crashed".

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

pub struct Quarantine {
    dir: PathBuf,
}

impl Quarantine {
    /// Open (creating if needed) the sentinel directory. The editor
    /// passes a per-user state dir; tests pass a temp dir.
    pub fn open(dir: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Whether a previous session died while this dylib was armed.
    pub fn is_quarantined(&self, dylib: &Path) -> bool {
        self.sentinel_path(dylib).exists()
    }

    /// Every dylib currently quarantined (for the startup notice and
    /// the "load anyway" affordance, which clears the sentinel).
    pub fn quarantined_dylibs(&self) -> Vec<PathBuf> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
            .map(|contents| PathBuf::from(contents.trim()))
            .collect()
    }

    /// Clear a dylib's sentinel (user chose to load it again).
    pub fn clear(&self, dylib: &Path) {
        let _ = std::fs::remove_file(self.sentinel_path(dylib));
    }

    /// Arm the sentinel before loading `dylib`. Disarm the returned
    /// guard once the load survived; a process death in between leaves
    /// the sentinel on disk.
    pub fn arm(&self, dylib: &Path) -> std::io::Result<QuarantineGuard> {
        let sentinel = self.sentinel_path(dylib);
        std::fs::write(&sentinel, dylib.display().to_string())?;
        Ok(QuarantineGuard { sentinel })
    }

    fn sentinel_path(&self, dylib: &Path) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        dylib.hash(&mut hasher);
        self.dir.join(format!("{:016x}.crashed", hasher.finish()))
    }
}

/// Proof that a load is in flight. Dropping WITHOUT disarming keeps
/// the sentinel: disarm is deliberate, not RAII, because the guard
/// must survive until the session considers the dylib safe (not
/// merely until the loading function returns).
pub struct QuarantineGuard {
    sentinel: PathBuf,
}

impl QuarantineGuard {
    pub fn disarm(self) {
        let _ = std::fs::remove_file(&self.sentinel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_quarantine(name: &str) -> Quarantine {
        let dir = std::env::temp_dir().join(format!(
            "jackdaw_quarantine_test_{name}_{:x}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        Quarantine::open(dir).unwrap()
    }

    #[test]
    fn armed_sentinel_survives_until_disarm() {
        let q = temp_quarantine("disarm");
        let dylib = Path::new("/proj/.jackdaw/libgame.so");
        assert!(!q.is_quarantined(dylib));

        let guard = q.arm(dylib).unwrap();
        assert!(q.is_quarantined(dylib));
        assert_eq!(q.quarantined_dylibs(), vec![dylib.to_path_buf()]);

        guard.disarm();
        assert!(!q.is_quarantined(dylib));
    }

    #[test]
    fn dropping_the_guard_keeps_the_sentinel() {
        let q = temp_quarantine("drop");
        let dylib = Path::new("/proj/.jackdaw/libgame.so");
        drop(q.arm(dylib).unwrap());
        assert!(q.is_quarantined(dylib), "drop must not disarm");
        q.clear(dylib);
        assert!(!q.is_quarantined(dylib));
    }
}
