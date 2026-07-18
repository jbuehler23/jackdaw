//! SDK bootstrap: build the SDK once into a per-version cache so an
//! installed or downloaded jackdaw sets itself up on first use, without a
//! source checkout.
//!
//! This module owns the cache location and its validity stamp. The cache
//! is laid out exactly as the `JACKDAW_SDK_DIR` "installed" layout that
//! [`SdkPaths::for_installed_root`](crate::sdk_paths::SdkPaths::for_installed_root)
//! reads, so a bootstrapped SDK is discovered with no env var. The build
//! orchestration (extract the embedded recipe, ensure the toolchain,
//! cargo build, arrange the artifacts) lands on top of this.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The rustup toolchain the SDK is pinned to. Must match the embedded
/// recipe's `rust-toolchain.toml`: the rmeta trick requires project
/// builds and the SDK to share an exact rustc.
pub const SDK_TOOLCHAIN_CHANNEL: &str = "nightly-2026-03-05";

/// The jackdaw data dir: `$XDG_DATA_HOME/jackdaw` when set to an absolute
/// path, else `~/.jackdaw`. `None` when no home directory resolves.
pub fn data_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME") {
        let xdg = PathBuf::from(xdg);
        if xdg.is_absolute() {
            return Some(xdg.join("jackdaw"));
        }
    }
    home_dir().map(|home| home.join(".jackdaw"))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// The cache dir for this (jackdaw version, toolchain) SDK build. Keyed
/// so a version or toolchain change lands in a fresh dir and old ones can
/// be reclaimed.
pub fn cache_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("sdk").join(cache_key()))
}

fn cache_key() -> String {
    format!("{}-{}", env!("CARGO_PKG_VERSION"), SDK_TOOLCHAIN_CHANNEL)
}

/// Validity stamp written after a successful build. A mismatch (version,
/// toolchain, target, or the embedded-recipe hash) triggers a rebuild.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stamp {
    pub version: String,
    pub channel: String,
    pub triple: String,
    /// Hash of the embedded recipe the SDK was built from, so a jackdaw
    /// upgrade that changes the recipe rebuilds even at the same version.
    pub recipe_hash: String,
}

impl Stamp {
    pub fn current(triple: &str, recipe_hash: &str) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").to_string(),
            channel: SDK_TOOLCHAIN_CHANNEL.to_string(),
            triple: triple.to_string(),
            recipe_hash: recipe_hash.to_string(),
        }
    }

    /// Whether a stamp matches the running binary for the given target and
    /// embedded recipe. Used by `ensure_sdk` to decide whether to rebuild.
    pub fn matches(&self, triple: &str, recipe_hash: &str) -> bool {
        self.version == env!("CARGO_PKG_VERSION")
            && self.channel == SDK_TOOLCHAIN_CHANNEL
            && self.triple == triple
            && self.recipe_hash == recipe_hash
    }
}

fn stamp_path(cache: &Path) -> PathBuf {
    cache.join("stamp.json")
}

pub fn read_stamp(cache: &Path) -> Option<Stamp> {
    let bytes = std::fs::read(stamp_path(cache)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub fn write_stamp(cache: &Path, stamp: &Stamp) -> std::io::Result<()> {
    std::fs::create_dir_all(cache)?;
    let json = serde_json::to_vec_pretty(stamp)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(stamp_path(cache), json)
}

/// Whether the cache holds an SDK usable by the running binary: a stamp
/// for this version/toolchain/target and a present SDK dylib. Used by
/// `SdkPaths::compute` to auto-discover a bootstrapped SDK. The stricter
/// recipe-hash check lives in `ensure_sdk`, which decides rebuilds.
pub fn cache_resolves(cache: &Path, triple: &str) -> bool {
    let stamp_ok = read_stamp(cache).is_some_and(|s| {
        s.version == env!("CARGO_PKG_VERSION")
            && s.channel == SDK_TOOLCHAIN_CHANNEL
            && s.triple == triple
    });
    stamp_ok
        && cache
            .join("sdk")
            .join(triple)
            .join(crate::sdk_paths::dylib_name())
            .is_file()
}

/// Remove cache dirs for other (version, toolchain) keys, keeping the
/// current one. Best-effort; called after a successful build.
pub fn gc_other_versions() {
    let Some(sdk_root) = data_dir().map(|d| d.join("sdk")) else {
        return;
    };
    let keep = cache_key();
    let Ok(entries) = std::fs::read_dir(&sdk_root) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy() != keep {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_round_trips_and_matches() {
        let dir = std::env::temp_dir().join(format!("jackdaw_stamp_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let stamp = Stamp::current("x86_64-unknown-linux-gnu", "abc123");
        write_stamp(&dir, &stamp).unwrap();
        let read = read_stamp(&dir).unwrap();
        assert!(read.matches("x86_64-unknown-linux-gnu", "abc123"));
        assert!(!read.matches("x86_64-unknown-linux-gnu", "different"));
        assert!(!read.matches("aarch64-apple-darwin", "abc123"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_key_is_version_and_channel() {
        let key = cache_key();
        assert!(key.starts_with(env!("CARGO_PKG_VERSION")));
        assert!(key.ends_with(SDK_TOOLCHAIN_CHANNEL));
    }

    #[test]
    fn data_dir_ends_in_a_jackdaw_component() {
        // `~/.jackdaw` or `<xdg>/jackdaw`; only checked when a home or XDG
        // resolves in the test env.
        if let Some(dir) = data_dir() {
            let last = dir.file_name().unwrap().to_string_lossy().into_owned();
            assert!(last == "jackdaw" || last == ".jackdaw", "got {last}");
        }
    }
}
