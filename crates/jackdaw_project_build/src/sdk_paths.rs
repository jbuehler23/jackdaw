//! Locate the SDK dylib, its deps dirs, and the rustc wrapper.
//!
//! Both `ext_build` and `project_build` need these paths; the
//! computation lives here so call sites can't drift. Paths are
//! computed, not verified; callers perform their own existence checks.
//!
//! Two layouts resolve, in order:
//!
//! 1. `JACKDAW_SDK_DIR` env var: an installed distribution,
//!    `<dir>/sdk/<triple>/libjackdaw_sdk.so` + `deps/`,
//!    `<dir>/sdk/host-deps/`, `<dir>/sdk/manifest.txt`, wrapper and
//!    runner binaries next to the editor in `<dir>`.
//! 2. A dev checkout: the running executable sits in
//!    `<workspace>/target/debug/`, the SDK build lives in
//!    `<workspace>/target/<triple>/debug/` (built with an explicit
//!    `--target`; host-side proc-macro dylibs stay in
//!    `target/debug/deps/`), and the manifest is generated on demand
//!    by `project_build::plan`.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Everything an editor-driven project build needs to point
/// cargo-spawned rustc at the editor's SDK.
pub struct SdkPaths {
    /// Absolute path to `libjackdaw_sdk.{so,dylib,dll}`.
    pub dylib: PathBuf,
    /// The sibling `deps/` directory holding the SDK's target-side
    /// rlib/rmeta artifacts (`-L dependency=`, redirect plan targets).
    pub deps: PathBuf,
    /// The host-side deps directory holding proc-macro dylibs that SDK
    /// rlibs reference as `MacrosOnly` dependencies.
    pub host_deps: PathBuf,
    /// Absolute path to `jackdaw-rustc-wrapper(.exe)`.
    pub wrapper: PathBuf,
    /// Absolute path to `jackdaw-runner(.exe)`.
    pub runner: PathBuf,
    /// The SDK manifest (`name version artifact` lines): the SDK's
    /// runtime closure with the exact artifact each crate compiled to.
    /// Present in installed layouts; generated on demand in dev.
    pub manifest: PathBuf,
    /// The SDK's `Cargo.lock`. Copied into the shim so the project
    /// dylib resolves the shared dependency closure at the exact
    /// versions the SDK was built with; without it a freshly created
    /// project drifts to newer patch releases and its redirected rlibs
    /// become inconsistent with the SDK's.
    pub lockfile: PathBuf,
    /// The target triple the SDK was built for (the host triple).
    pub triple: String,
    /// The rustup toolchain channel the SDK was compiled with. Project
    /// dylibs must build with the same one, or their rlibs are
    /// rejected as compiled by an incompatible rustc. `None` when it
    /// cannot be resolved; the pipeline then relies on the ambient
    /// toolchain.
    pub toolchain: Option<String>,
}

impl SdkPaths {
    pub fn compute() -> Self {
        let triple = host_triple().to_string();
        // 1. Explicit override: a packaged distribution.
        if let Ok(from_env) = std::env::var("JACKDAW_SDK_DIR") {
            return Self::for_installed_root(std::path::Path::new(&from_env));
        }

        // 2. Dev checkout, when its SDK is actually built. A contributor
        //    running from `target/` must link project code against the SDK
        //    co-built with this editor, at the same profile: a debug editor
        //    and a release bootstrap cache are not link-compatible, because
        //    cargo bakes the profile into each crate's `-C metadata`, which
        //    changes the mangled symbol names. So an in-tree SDK wins over
        //    any cache a `embed-recipe` build left behind while testing the
        //    packaged flow.
        let dev = Self::dev_checkout(&triple);
        if dev.dylib_exists() {
            return dev;
        }

        // 2b. An installed distribution launched directly: the relocatable
        //     SDK is staged in `sdk/` next to the editor executable (the
        //     `jackdaw-cli bundle` layout). Auto-detected with no env var so a
        //     downloaded jackdaw "just works" when run from its extracted
        //     folder, not only when a launcher sets `JACKDAW_SDK_DIR`.
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let installed = Self::for_installed_root(dir);
            if installed.dylib_exists() {
                return installed;
            }
        }

        // 3. The bootstrap cache: an SDK this binary built for itself on
        //    first use, kept as a dev-style build under `<cache>/build/`.
        //    Auto-discovered with no env var so a downloaded jackdaw "just
        //    works" once setup has run. Reached only when there is no
        //    in-tree SDK (a real install), so editor and cache profiles
        //    match.
        if let Some(cache) = crate::bootstrap::cache_dir()
            && crate::bootstrap::cache_resolves(&cache, &triple)
        {
            return Self::for_workspace_profile(&cache.join("build"), "release");
        }

        // Neither resolved: return the dev layout so a missing-SDK error
        // points at where a contributor builds it.
        dev
    }

    /// Assemble the dev / workspace layout from a resolved `host_dir`
    /// (host-side tools and proc-macro deps) and `triple_dir` (target-side
    /// SDK artifacts) pair, plus the toolchain and lockfile the caller
    /// derived. Both [`dev_checkout`](Self::dev_checkout) and
    /// [`for_workspace_profile`](Self::for_workspace_profile) produce this
    /// identical field layout.
    fn from_dev_dirs(
        host_dir: &std::path::Path,
        triple_dir: &std::path::Path,
        triple: String,
        toolchain: Option<String>,
        lockfile: PathBuf,
    ) -> Self {
        Self {
            dylib: triple_dir.join(dylib_name()),
            deps: triple_dir.join("deps"),
            host_deps: host_dir.join("deps"),
            wrapper: host_dir.join(wrapper_name()),
            runner: triple_dir.join(runner_name()),
            manifest: triple_dir.join("jackdaw_sdk_manifest.txt"),
            triple,
            toolchain,
            lockfile,
        }
    }

    /// The dev-checkout layout, derived from the running executable's
    /// location. The editor runs either from `<workspace>/target/debug/`
    /// (a plain build) or `<workspace>/target/<triple>/debug/` (a
    /// `--target` build); both resolve to the same layout: SDK artifacts in
    /// the triple dir, host-side tools and proc-macro deps in
    /// `target/debug`.
    fn dev_checkout(triple: &str) -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(ToOwned::to_owned))
            .unwrap_or_else(|| PathBuf::from("."));
        let target_dir = if exe_dir
            .parent()
            .and_then(|p| p.file_name())
            .is_some_and(|name| name.to_string_lossy() == triple)
        {
            exe_dir
                .parent()
                .and_then(std::path::Path::parent)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| PathBuf::from("."))
        } else {
            exe_dir
                .parent()
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| PathBuf::from("."))
        };
        let host_dir = target_dir.join("debug");
        let triple_dir = target_dir.join(triple).join("debug");
        Self::from_dev_dirs(
            &host_dir,
            &triple_dir,
            triple.to_string(),
            read_toolchain_channel(&target_dir.join("../rust-toolchain.toml")),
            target_dir.join("../Cargo.lock"),
        )
    }

    /// Build the paths for a packaged / bootstrapped "installed layout"
    /// rooted at `root`: `sdk/<triple>/libjackdaw_sdk.*` + `deps/`,
    /// `sdk/host-deps/`, `sdk/manifest.txt`, the wrapper and runner
    /// binaries in `root`, `toolchain.txt`, and `Cargo.lock`. Shared by
    /// the `JACKDAW_SDK_DIR` override and the bootstrap cache.
    pub fn for_installed_root(root: &std::path::Path) -> Self {
        let triple = host_triple().to_string();
        let sdk = root.join("sdk").join(&triple);
        Self {
            dylib: sdk.join(dylib_name()),
            deps: sdk.join("deps"),
            host_deps: root.join("sdk").join("host-deps"),
            wrapper: root.join(wrapper_name()),
            runner: root.join(runner_name()),
            manifest: root.join("sdk").join("manifest.txt"),
            triple,
            toolchain: read_toolchain_txt(&root.join("toolchain.txt")),
            lockfile: root.join("Cargo.lock"),
        }
    }

    /// Dev-checkout constructor used by the SDK pipeline tests:
    /// everything under an explicit workspace root instead of the
    /// running executable's location. Resolves the `debug` profile.
    pub fn for_workspace(workspace_root: &std::path::Path) -> Self {
        Self::for_workspace_profile(workspace_root, "debug")
    }

    /// Like [`for_workspace`](Self::for_workspace) but for a specific
    /// cargo profile directory (`debug` / `release`).
    pub fn for_workspace_profile(workspace_root: &std::path::Path, profile: &str) -> Self {
        let triple = host_triple().to_string();
        let host_dir = workspace_root.join("target").join(profile);
        let triple_dir = workspace_root.join("target").join(&triple).join(profile);
        Self::from_dev_dirs(
            &host_dir,
            &triple_dir,
            triple,
            read_toolchain_channel(&workspace_root.join("rust-toolchain.toml")),
            workspace_root.join("Cargo.lock"),
        )
    }

    /// Find a usable SDK under a workspace's `target/` dir, preferring
    /// `release` then `debug`, returning the first whose SDK dylib exists.
    /// Lets a `jackdaw-cli` installed on `PATH` locate the SDK the editor
    /// was built with, rather than looking next to its own binary.
    pub fn for_workspace_detect(workspace_root: &std::path::Path) -> Option<Self> {
        ["release", "debug"].into_iter().find_map(|profile| {
            let sdk = Self::for_workspace_profile(workspace_root, profile);
            sdk.dylib_exists().then_some(sdk)
        })
    }

    pub fn dylib_exists(&self) -> bool {
        self.dylib.is_file()
    }

    pub fn wrapper_exists(&self) -> bool {
        self.wrapper.is_file()
    }
}

/// The host triple, from `rustc -vV`, cached for the process. Every
/// editor-driven build passes it as an explicit `--target` so cargo
/// separates host-side units (which the wrapper must never rewrite).
pub fn host_triple() -> &'static str {
    static TRIPLE: OnceLock<String> = OnceLock::new();
    TRIPLE.get_or_init(|| {
        let output = Command::new("rustc")
            .arg("-vV")
            .output()
            .expect("rustc -vV must run; the editor requires a Rust toolchain");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .expect("rustc -vV reports a host triple")
            .to_string()
    })
}

/// Read the `channel` from a `rust-toolchain.toml` (dev checkout).
fn read_toolchain_channel(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("channel")?;
        let value = rest.trim_start().strip_prefix('=')?.trim();
        Some(value.trim_matches(['"', '\'']).to_string())
    })
}

/// Read a shipped `toolchain.txt` (release layout): the bare channel
/// name on the first non-empty line.
fn read_toolchain_txt(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string)
}

pub(crate) fn dylib_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "jackdaw_sdk.dll"
    } else if cfg!(target_os = "macos") {
        "libjackdaw_sdk.dylib"
    } else {
        "libjackdaw_sdk.so"
    }
}

pub(crate) fn wrapper_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "jackdaw-rustc-wrapper.exe"
    } else {
        "jackdaw-rustc-wrapper"
    }
}

pub(crate) fn runner_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "jackdaw-runner.exe"
    } else {
        "jackdaw-runner"
    }
}
