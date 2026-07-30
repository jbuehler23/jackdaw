//! Locate the SDK dylib, its deps dirs, and the rustc wrapper.
//!
//! Both `ext_build` and `project_build` need these paths; the
//! computation lives here so call sites can't drift. Paths are
//! computed, not verified; callers perform their own existence checks.
//!
//! Two on-disk shapes exist, and [`SdkPaths::compute`] tries the
//! sources that can hold them in a fixed order (see [`SdkOrigin`]):
//!
//! 1. An installed distribution:
//!    `<dir>/sdk/<triple>/libjackdaw_sdk.so` + `deps/`,
//!    `<dir>/sdk/host-deps/`, `<dir>/sdk/manifest.txt`, wrapper and
//!    runner binaries next to the editor in `<dir>`. This is what a
//!    downloaded bundle unpacks to, named by `JACKDAW_SDK_DIR` or
//!    found next to the running executable.
//! 2. A cargo workspace's `target/`: the SDK build lives in
//!    `<root>/target/<triple>/<profile>/` (built with an explicit
//!    `--target`; host-side proc-macro dylibs stay in
//!    `target/<profile>/deps/`). Both a source checkout and the SDK
//!    prepared on first use under `~/.jackdaw/` have this shape; in a
//!    checkout the manifest is generated on demand by
//!    `project_build::plan`.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// Where the SDK a build is using came from.
///
/// Three install paths can each supply one, and they can coexist on the
/// same machine: a downloaded bundle, an SDK this binary built for
/// itself, and a source checkout's `target/`. Which one won decides how
/// long builds take and which artifacts a project links, so it has to
/// be reportable rather than implicit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SdkOrigin {
    /// `JACKDAW_SDK_DIR` named it explicitly.
    Override,
    /// A source checkout's `target/`, co-built with this editor.
    DevCheckout,
    /// Staged next to the executable by a downloaded release bundle.
    /// The only path with no first-run compile.
    Bundled,
    /// Built on first use into the per-version cache.
    BootstrapCache,
    /// Nothing resolved; the paths describe where one would go.
    Missing,
}

impl SdkOrigin {
    /// A short phrase for reports, naming the install path a user would
    /// recognise.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Override => "explicit JACKDAW_SDK_DIR",
            Self::DevCheckout => "source checkout",
            Self::Bundled => "release bundle",
            Self::BootstrapCache => "prepared on first use",
            Self::Missing => "not found",
        }
    }
}

/// Everything an editor-driven project build needs to point
/// cargo-spawned rustc at the editor's SDK.
pub struct SdkPaths {
    /// Which install path supplied this SDK.
    pub origin: SdkOrigin,
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
    /// Record which install path produced these. The constructors set a
    /// sensible default for their layout; [`compute`](Self::compute)
    /// overrides it with the branch that actually won.
    #[must_use]
    pub fn with_origin(mut self, origin: SdkOrigin) -> Self {
        self.origin = origin;
        self
    }

    /// What makes this SDK unusable, if anything. Only conditions under
    /// which no project can build: a build that starts anyway spends
    /// minutes to reach a rustc error that names a crate rather than the
    /// cause, so these are checked first, from a directory listing.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.dylib_exists() {
            out.push(format!("no SDK library at {}", self.dylib.display()));
        }
        if !self.wrapper.is_file() {
            out.push(format!("no rustc wrapper at {}", self.wrapper.display()));
        }
        out
    }

    pub fn compute() -> Self {
        let triple = host_triple().to_string();
        // 1. Explicit override: a packaged distribution.
        if let Ok(from_env) = std::env::var("JACKDAW_SDK_DIR") {
            return Self::for_override_root(std::path::Path::new(&from_env));
        }

        // 2. Dev checkout, when its SDK is actually built. A contributor
        //    running from `target/` must link project code against the SDK
        //    co-built with this editor, at the same profile: a debug editor
        //    and a release bootstrap cache are not link-compatible, because
        //    cargo bakes the profile into each crate's `-C metadata`, which
        //    changes the mangled symbol names. So an in-tree SDK wins over
        //    any cache a `embed-recipe` build left behind while testing the
        //    packaged flow.
        let dev = Self::dev_checkout(&triple).with_origin(SdkOrigin::DevCheckout);
        if dev.dylib_exists() && !dev_sdk_superseded(&dev, &triple) {
            return dev;
        }

        // 2b. An installed distribution launched directly: the relocatable
        //     SDK is staged in `sdk/` next to the editor executable (the
        //     `cargo xtask bundle` layout). Auto-detected with no env var so a
        //     downloaded jackdaw "just works" when run from its extracted
        //     folder, not only when a launcher sets `JACKDAW_SDK_DIR`.
        if let Ok(exe) = std::env::current_exe()
            && let Some(dir) = exe.parent()
        {
            let installed = Self::for_installed_root(dir).with_origin(SdkOrigin::Bundled);
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
            return Self::for_workspace_profile(&cache.join("build"), "release")
                .with_origin(SdkOrigin::BootstrapCache);
        }

        // Neither resolved: return the dev layout so a missing-SDK error
        // points at where a contributor builds it.
        dev.with_origin(SdkOrigin::Missing)
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
            origin: SdkOrigin::DevCheckout,
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
    /// location. The editor runs from `<workspace>/target/<profile>/` (a
    /// plain build) or `<workspace>/target/<triple>/<profile>/` (a
    /// `--target` build); both resolve to the same layout, with SDK
    /// artifacts in the triple dir and host-side tools and proc-macro
    /// deps beside the executable.
    ///
    /// The profile is read from the path rather than assumed. A release
    /// editor resolving a debug SDK is not a near miss: cargo bakes the
    /// profile into each crate's `-C metadata`, so the mangled symbol
    /// names differ and nothing the project links will match.
    fn dev_checkout(triple: &str) -> Self {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(ToOwned::to_owned))
            .unwrap_or_else(|| PathBuf::from("."));
        let profile = exe_dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "debug".to_string());
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
        let host_dir = target_dir.join(&profile);
        let triple_dir = target_dir.join(triple).join(&profile);
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
            origin: SdkOrigin::Bundled,
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

    /// Resolve `$JACKDAW_SDK_DIR`, whichever of the three shapes it
    /// names. The variable was documented for a release bundle root,
    /// but pointing it at a prepared SDK cache or a jackdaw checkout is
    /// the obvious thing to try, and reading those as bundles produced
    /// only a bare "no such file" from much later in the build. Each
    /// shape is recognised by a directory that only it has.
    pub fn for_override_root(root: &std::path::Path) -> Self {
        if root.join("sdk").is_dir() {
            return Self::for_installed_root(root).with_origin(SdkOrigin::Override);
        }
        // A prepared SDK cache: `<cache>/build/` is a dev-style
        // workspace, always built at release.
        let cache_build = root.join("build");
        if cache_build.join("Cargo.toml").is_file() {
            return Self::for_workspace_profile(&cache_build, "release")
                .with_origin(SdkOrigin::Override);
        }
        if let Some(sdk) = Self::for_workspace_detect(root) {
            return sdk.with_origin(SdkOrigin::Override);
        }
        Self::for_installed_root(root).with_origin(SdkOrigin::Override)
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
    /// Lets `jd` installed on `PATH` locate the SDK the editor
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

/// Whether a checkout's in-tree SDK is stale enough that the prepared
/// cache is the better choice.
///
/// `cargo run` builds the editor into `target/<profile>/` and never
/// touches the `target/<triple>/` dir the SDK lives in, so the two come
/// from separate commands and drift apart. Once they do, the in-tree SDK
/// cannot link: its rlibs reference proc-macro builds that later
/// compiles have since replaced, which rustc reports as a missing crate
/// nobody has heard of.
///
/// Every condition here has to hold, because preferring the in-tree SDK
/// is right whenever it is current, and a contributor working on the SDK
/// itself must never be quietly switched away from it:
///
/// * the editor was not built into the triple dir. When it was, editor
///   and SDK came from one `--target` invocation, and the editor linking
///   last makes it newer by seconds with nothing wrong.
/// * the SDK predates the editor, which is what "rebuilt without it"
///   looks like.
/// * the editor is a release build, so the cache (always release) can
///   actually link against it.
/// * a usable cache exists. Falling back to nothing would be worse than
///   a stale SDK plus the warning the build prints.
fn dev_sdk_superseded(dev: &SdkPaths, triple: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let Some(exe_dir) = exe.parent() else {
        return false;
    };
    let built_with_target = exe_dir
        .parent()
        .and_then(|p| p.file_name())
        .is_some_and(|name| name == triple);
    let is_release = exe_dir.file_name().is_some_and(|name| name == "release");
    if built_with_target || !is_release {
        return false;
    }
    let modified = |path: &std::path::Path| std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let (Some(sdk_built), Some(editor_built)) = (modified(&dev.dylib), modified(&exe)) else {
        return false;
    };
    if sdk_built >= editor_built {
        return false;
    }
    crate::bootstrap::cache_dir()
        .is_some_and(|cache| crate::bootstrap::cache_resolves(&cache, triple))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the layout `cargo xtask bundle` stages, per the doc comment
    /// on `jackdaw_cli::package`.
    fn fake_bundle(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("jackdaw_bundle_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let triple = host_triple();
        let sdk = root.join("sdk").join(triple);
        std::fs::create_dir_all(sdk.join("deps")).unwrap();
        std::fs::create_dir_all(root.join("sdk").join("host-deps")).unwrap();
        std::fs::write(sdk.join(dylib_name()), b"sdk").unwrap();
        std::fs::write(root.join("sdk").join("manifest.txt"), b"").unwrap();
        std::fs::write(root.join(wrapper_name()), b"wrapper").unwrap();
        std::fs::write(root.join(runner_name()), b"runner").unwrap();
        std::fs::write(root.join("Cargo.lock"), b"").unwrap();
        std::fs::write(root.join("toolchain.txt"), b"nightly-2026-03-05\n").unwrap();
        root
    }

    /// The bundler and the resolver have to agree on where everything
    /// sits, or a downloaded release resolves to nothing. Nothing else
    /// checks that contract without building a real ~3 GB bundle.
    #[test]
    fn a_staged_bundle_layout_resolves_cleanly() {
        let root = fake_bundle("layout");
        let sdk = SdkPaths::for_installed_root(&root);

        assert_eq!(sdk.origin, SdkOrigin::Bundled);
        assert!(sdk.dylib_exists(), "dylib at {}", sdk.dylib.display());
        assert!(sdk.wrapper.is_file());
        assert!(sdk.runner.is_file());
        assert!(sdk.manifest.is_file());
        assert!(sdk.lockfile.is_file());
        assert_eq!(sdk.toolchain.as_deref(), Some("nightly-2026-03-05"));
        assert!(
            sdk.problems().is_empty(),
            "a freshly staged bundle is clean: {:?}",
            sdk.problems()
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// A missing SDK names what is absent rather than failing later.
    #[test]
    fn an_empty_root_reports_what_is_missing() {
        let root =
            std::env::temp_dir().join(format!("jackdaw_bundle_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let problems = SdkPaths::for_installed_root(&root).problems();
        assert!(problems.iter().any(|p| p.contains("no SDK library")));
        assert!(problems.iter().any(|p| p.contains("no rustc wrapper")));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `JACKDAW_SDK_DIR` takes whichever of the three SDK shapes it is
    /// pointed at, rather than only the bundle it was documented for.
    #[test]
    fn the_override_accepts_every_sdk_shape() {
        let bundle = fake_bundle("override_bundle");
        let sdk = SdkPaths::for_override_root(&bundle);
        assert_eq!(sdk.origin, SdkOrigin::Override);
        assert!(sdk.dylib_exists(), "bundle root: {}", sdk.dylib.display());

        // A prepared cache holds a dev-style workspace under `build/`.
        let cache = std::env::temp_dir().join(format!("jackdaw_cache_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&cache);
        let build = cache.join("build");
        let triple_dir = build.join("target").join(host_triple()).join("release");
        std::fs::create_dir_all(triple_dir.join("deps")).unwrap();
        std::fs::create_dir_all(build.join("target").join("release").join("deps")).unwrap();
        std::fs::write(build.join("Cargo.toml"), b"[workspace]\n").unwrap();
        std::fs::write(triple_dir.join(dylib_name()), b"sdk").unwrap();
        std::fs::write(
            build.join("target").join("release").join(wrapper_name()),
            b"w",
        )
        .unwrap();

        let sdk = SdkPaths::for_override_root(&cache);
        assert_eq!(sdk.origin, SdkOrigin::Override);
        assert!(sdk.dylib_exists(), "cache root: {}", sdk.dylib.display());

        let _ = std::fs::remove_dir_all(&bundle);
        let _ = std::fs::remove_dir_all(&cache);
    }

    /// Each origin reads as an install path a user would recognise, so
    /// a report tells them which of the three they are on.
    #[test]
    fn every_origin_describes_an_install_path() {
        for origin in [
            SdkOrigin::Override,
            SdkOrigin::DevCheckout,
            SdkOrigin::Bundled,
            SdkOrigin::BootstrapCache,
            SdkOrigin::Missing,
        ] {
            assert!(!origin.describe().is_empty());
        }
        assert_eq!(SdkOrigin::Bundled.describe(), "release bundle");
    }

    /// `cargo run --release` puts the editor in `target/release`, and
    /// the SDK it must link sits in `target/<triple>/release`. Resolving
    /// `debug` regardless meant a release editor pointed at a debug SDK,
    /// which cannot link: cargo bakes the profile into each crate's
    /// `-C metadata`, so the mangled symbol names differ.
    #[test]
    fn the_dev_layout_follows_the_profile_the_editor_was_built_at() {
        let triple = host_triple();
        for profile in ["debug", "release"] {
            let root = std::env::temp_dir()
                .join(format!("jackdaw_profile_{profile}_{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let sdk = SdkPaths::for_workspace_profile(&root, profile);

            assert!(
                sdk.dylib
                    .starts_with(root.join("target").join(triple).join(profile)),
                "{profile}: SDK in the matching triple dir, got {}",
                sdk.dylib.display()
            );
            assert!(
                sdk.wrapper.starts_with(root.join("target").join(profile)),
                "{profile}: wrapper beside the executable, got {}",
                sdk.wrapper.display()
            );
            assert!(
                !sdk.dylib.to_string_lossy().contains(if profile == "debug" {
                    "/release/"
                } else {
                    "/debug/"
                }),
                "{profile}: no path component from the other profile: {}",
                sdk.dylib.display()
            );
        }
    }
}
