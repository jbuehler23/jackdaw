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
use std::process::Command;

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

/// Whether an SDK-builder recipe is baked into this binary. False when
/// this crate was compiled outside the workspace (for example as a
/// crates.io dependency, or the recipe building itself), where there is
/// nothing to bootstrap from.
pub fn recipe_is_embedded() -> bool {
    !crate::RECIPE_FILES.is_empty()
}

/// Extract the embedded recipe into `dst`, ready for `cargo build`.
pub fn write_recipe(dst: &Path) -> std::io::Result<()> {
    for (rel, bytes) in crate::RECIPE_FILES {
        let path = dst.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)?;
    }
    Ok(())
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

/// Build the SDK into the cache if it is missing or stale, and return the
/// cache dir, which [`SdkPaths::compute`](crate::sdk_paths::SdkPaths::compute)
/// then resolves with no env var. The first call is slow: it installs the
/// pinned toolchain via rustup and compiles the SDK (~10-15 min); later
/// calls with a matching stamp return at once. `progress` receives phase
/// strings for the setup UI.
///
/// The artifact arrangement mirrors the dev/CI layout (SDK dylib, runner,
/// and their deps built with `--target`; the wrapper built for the host),
/// so `for_workspace_profile` locates the build outputs and
/// `for_installed_root` names their cache destinations. These paths are
/// validated against the first real bootstrap build.
pub fn ensure_sdk(mut progress: impl FnMut(&str)) -> Result<PathBuf, String> {
    if !recipe_is_embedded() {
        return Err("this jackdaw was built without an embedded SDK recipe \
                    (the `embed-recipe` feature); it cannot bootstrap an SDK"
            .to_string());
    }
    let triple = crate::sdk_paths::host_triple().to_string();
    let cache = cache_dir().ok_or_else(|| "no home directory for the SDK cache".to_string())?;

    if read_stamp(&cache).is_some_and(|s| s.matches(&triple, crate::RECIPE_HASH))
        && cache_resolves(&cache, &triple)
    {
        return Ok(cache);
    }

    progress("Installing the pinned Rust toolchain");
    install_toolchain()?;

    let build_dir = cache.join("build");
    let _ = std::fs::remove_dir_all(&build_dir);
    progress("Unpacking SDK sources");
    write_recipe(&build_dir).map_err(|e| format!("unpack recipe: {e}"))?;

    progress("Building the SDK (one-time; this can take several minutes)");
    build_recipe(&build_dir, &triple)?;

    progress("Installing the SDK");
    arrange(&build_dir, &cache, &triple)?;

    write_stamp(&cache, &Stamp::current(&triple, crate::RECIPE_HASH))
        .map_err(|e| format!("write stamp: {e}"))?;
    gc_other_versions();
    progress("SDK ready");
    Ok(cache)
}

fn install_toolchain() -> Result<(), String> {
    let status = Command::new("rustup")
        .args([
            "toolchain",
            "install",
            SDK_TOOLCHAIN_CHANNEL,
            "--profile",
            "minimal",
        ])
        .status()
        .map_err(|e| format!("rustup is required to build the SDK but could not run: {e}"))?;
    if !status.success() {
        return Err(format!("failed to install the {SDK_TOOLCHAIN_CHANNEL} toolchain"));
    }
    Ok(())
}

fn build_recipe(build_dir: &Path, triple: &str) -> Result<(), String> {
    // SDK dylib + runner are cross-target artifacts (`--target`); their
    // deps land in `target/<triple>/release` and proc-macro host deps in
    // `target/release`.
    run_cargo(
        build_dir,
        &[
            "build",
            "--release",
            "--target",
            triple,
            "-p",
            "jackdaw_sdk",
            "-p",
            "jackdaw_runner",
        ],
    )?;
    // The rustc wrapper is a host tool; build it without `--target` so it
    // lands in `target/release`, where `for_workspace_profile` looks.
    run_cargo(build_dir, &["build", "--release", "-p", "jackdaw_rustc_wrapper"])
}

fn run_cargo(build_dir: &Path, args: &[&str]) -> Result<(), String> {
    let output = Command::new("cargo")
        .arg(format!("+{SDK_TOOLCHAIN_CHANNEL}"))
        .args(args)
        .env("CARGO_INCREMENTAL", "0")
        .current_dir(build_dir)
        .output()
        .map_err(|e| format!("cargo build: {e}"))?;
    if !output.status.success() {
        let log = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = log.lines().rev().take(30).collect();
        let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
        return Err(format!("SDK build failed:\n{tail}"));
    }
    Ok(())
}

fn arrange(build_dir: &Path, cache: &Path, triple: &str) -> Result<(), String> {
    let built = crate::sdk_paths::SdkPaths::for_workspace_profile(build_dir, "release");
    let out = crate::sdk_paths::SdkPaths::for_installed_root(cache);

    // Fresh install layout under the cache.
    let _ = std::fs::remove_dir_all(cache.join("sdk"));
    mkdirs(&cache.join("sdk").join(triple))?;
    mkdirs(&cache.join("sdk").join("host-deps"))?;

    copy_file(&built.dylib, &out.dylib)?;
    copy_dir(&built.deps, &out.deps)?;
    copy_dir(&built.host_deps, &out.host_deps)?;
    copy_file(&built.runner, &out.runner)?;
    copy_file(&built.wrapper, &out.wrapper)?;
    copy_file(&build_dir.join("Cargo.lock"), &out.lockfile)?;
    std::fs::write(cache.join("toolchain.txt"), SDK_TOOLCHAIN_CHANNEL).map_err(io)?;

    // The extern-redirect manifest project builds consult.
    let manifest = crate::plan::SdkManifest::generate_dev(build_dir, &built)
        .map_err(|e| format!("generate SDK manifest: {e}"))?;
    manifest
        .write(&out.manifest)
        .map_err(|e| format!("write SDK manifest: {e}"))
}

fn io(e: std::io::Error) -> String {
    e.to_string()
}

fn mkdirs(p: &Path) -> Result<(), String> {
    std::fs::create_dir_all(p).map_err(io)
}

fn copy_file(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        mkdirs(parent)?;
    }
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| format!("copy {} -> {}: {e}", from.display(), to.display()))
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    mkdirs(to)?;
    let entries = std::fs::read_dir(from).map_err(|e| format!("read {}: {e}", from.display()))?;
    for entry in entries.flatten() {
        let f = entry.path();
        let t = to.join(entry.file_name());
        if f.is_dir() {
            copy_dir(&f, &t)?;
        } else {
            std::fs::copy(&f, &t).map_err(io)?;
        }
    }
    Ok(())
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
