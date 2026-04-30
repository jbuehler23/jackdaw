//! Shared `CARGO_TARGET_DIR` management.
//!
//! All projects on a machine running jackdaw `<version>` share one
//! target dir at `~/.cache/jackdaw/<version>/target/` so cargo's
//! content-addressable artifact reuse means bevy and `jackdaw_editor`
//! are compiled once globally per version, then reused across all
//! user projects.

use std::path::PathBuf;

/// Returns the shared cache root for the current jackdaw version,
/// e.g. `~/.cache/jackdaw/0.4.0/`. Falls back to a temp directory if
/// the user has no platform cache dir.
pub fn cache_root() -> PathBuf {
    let base = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    base.join("jackdaw").join(env!("CARGO_PKG_VERSION"))
}

/// The shared `CARGO_TARGET_DIR`. Cargo invocations from any user
/// project's `.cargo/config.toml` point at this path.
pub fn target_dir() -> PathBuf {
    cache_root().join("target")
}

/// Path to the `.warmed` sentinel file. Presence indicates the
/// first-time setup compile completed successfully for this jackdaw
/// version.
pub fn warmed_sentinel() -> PathBuf {
    cache_root().join(".warmed")
}

/// `true` if first-time setup has completed for this jackdaw version.
pub fn is_warmed() -> bool {
    warmed_sentinel().exists()
}

/// Mark the cache as warmed (called by `setup_flow` on success).
pub fn mark_warmed() -> std::io::Result<()> {
    let path = warmed_sentinel();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, env!("CARGO_PKG_VERSION"))?;
    Ok(())
}

/// Path to the warm-up project that the setup flow scaffolds and
/// builds to populate the shared cache.
pub fn setup_scaffold_dir() -> PathBuf {
    cache_root().join("setup")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_paths_are_versioned() {
        let root = cache_root();
        assert!(root.to_string_lossy().contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn target_dir_under_cache_root() {
        assert!(target_dir().starts_with(cache_root()));
    }

    #[test]
    fn warmed_sentinel_under_cache_root() {
        assert!(warmed_sentinel().starts_with(cache_root()));
    }

    #[test]
    fn setup_scaffold_under_cache_root() {
        assert!(setup_scaffold_dir().starts_with(cache_root()));
    }
}
