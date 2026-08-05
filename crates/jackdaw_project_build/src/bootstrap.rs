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
        && crate::sdk_paths::SdkPaths::for_workspace_profile(&cache.join("build"), "release")
            .dylib
            .is_file()
}

/// Whether an SDK-builder recipe is baked into this binary. False when
/// this crate was compiled outside the workspace (for example as a
/// crates.io dependency, or the recipe building itself), where there is
/// nothing to bootstrap from.
pub fn recipe_is_embedded() -> bool {
    !crate::RECIPE_FILES.is_empty()
}

/// Whether a first-use SDK build is still owed: this binary carries a
/// recipe but no matching, resolvable cache exists yet. False in a dev
/// checkout (no embedded recipe; the dev SDK is used) and once setup has
/// run. Drives the editor's first-run setup screen and the CLI's
/// auto-`ensure_sdk`.
pub fn needs_setup() -> bool {
    if !recipe_is_embedded() {
        return false;
    }
    // A release bundle (or an explicit JACKDAW_SDK_DIR) already ships a
    // complete SDK next to the binary; without this check every `jd
    // build` and editor first-run on a downloaded bundle recompiled the
    // whole SDK from the embedded recipe anyway, hours of work for
    // artifacts the download already contains.
    let resolved = crate::sdk_paths::SdkPaths::compute();
    if matches!(
        resolved.origin,
        crate::sdk_paths::SdkOrigin::Bundled | crate::sdk_paths::SdkOrigin::Override
    ) && resolved.problems().is_empty()
        && resolved.manifest.is_file()
    {
        return false;
    }
    let triple = crate::sdk_paths::host_triple();
    let Some(cache) = cache_dir() else {
        return false;
    };
    !(read_stamp(&cache).is_some_and(|s| s.matches(triple, crate::RECIPE_HASH))
        && cache_resolves(&cache, triple))
}

/// One prerequisite [`ensure_sdk`] needs, for `doctor`-style reporting.
pub struct Prereq {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
    pub fix: Option<String>,
}

/// Check the tools an SDK build needs before committing to a long
/// compile: cargo and rustup (hard requirements) plus the pinned
/// toolchain (informational; setup installs it). Fast and side-effect
/// free. Used by `jd doctor` and as an early gate in
/// [`ensure_sdk`].
pub fn check_prerequisites() -> Vec<Prereq> {
    let mut out = Vec::new();

    out.push(match tool_version("cargo", "--version") {
        Some(version) => Prereq {
            name: "cargo",
            ok: true,
            detail: version,
            fix: None,
        },
        None => Prereq {
            name: "cargo",
            ok: false,
            detail: "not found on PATH".to_string(),
            fix: Some("install Rust from https://rustup.rs".to_string()),
        },
    });

    let rustup = tool_version("rustup", "--version");
    out.push(match &rustup {
        Some(version) => Prereq {
            name: "rustup",
            ok: true,
            detail: version.clone(),
            fix: None,
        },
        None => Prereq {
            name: "rustup",
            ok: false,
            detail: "not found on PATH".to_string(),
            fix: Some(
                "install rustup from https://rustup.rs (jackdaw manages the SDK toolchain with it)"
                    .to_string(),
            ),
        },
    });

    // The SDK build compiles jackdaw's CSG kernel (`manifold-csg-sys`),
    // a C++ library built with cmake. Without it the failure lands
    // minutes into a compile, so it belongs in the same up-front report
    // as cargo and rustup.
    out.push(match tool_version("cmake", "--version") {
        Some(version) => Prereq {
            name: "cmake",
            ok: true,
            detail: version,
            fix: None,
        },
        None => Prereq {
            name: "cmake",
            ok: false,
            detail: "not found on PATH".to_string(),
            fix: Some(
                "install cmake from https://cmake.org/download (the CSG kernel is built with it)"
                    .to_string(),
            ),
        },
    });

    // On Windows a MinGW `gcc` on PATH makes cmake pick it over MSVC,
    // and the resulting objects fail to link (LNK1143).
    #[cfg(windows)]
    if tool_version("gcc", "--version").is_some()
        && !std::env::var("CMAKE_GENERATOR")
            .is_ok_and(|generator| generator.contains("Visual Studio"))
    {
        out.push(Prereq {
            name: "Windows C++ toolchain",
            ok: false,
            detail: "MinGW gcc is on PATH; cmake may pick it over MSVC and fail to link"
                .to_string(),
            fix: Some("set CMAKE_GENERATOR=\"Visual Studio 17 2022\" before building".to_string()),
        });
    }

    // The pinned toolchain is not a hard failure: setup installs it on
    // demand. Report its state so `doctor` can preview whether the first
    // build pays for a toolchain download.
    if rustup.is_some() {
        let installed = Command::new("rustup")
            .args(["toolchain", "list"])
            .output()
            .ok()
            .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains(SDK_TOOLCHAIN_CHANNEL));
        out.push(Prereq {
            name: "SDK toolchain",
            ok: true,
            detail: if installed {
                format!("{SDK_TOOLCHAIN_CHANNEL} installed")
            } else {
                format!("{SDK_TOOLCHAIN_CHANNEL} will be installed on first setup")
            },
            fix: None,
        });
    }

    out
}

/// First line of `<cmd> <arg>` stdout, or `None` if the tool is absent or
/// exits non-zero.
fn tool_version(cmd: &str, arg: &str) -> Option<String> {
    let output = Command::new(cmd).arg(arg).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or("")
            .trim()
            .to_string(),
    )
}

/// Structured progress from [`ensure_sdk`], consumed by the CLI (which
/// prints phase lines and lets cargo's inherited stderr show its own
/// progress) and the editor's first-run screen (which drives a progress
/// bar from the per-crate counts). Phase strings are static literals.
pub enum SetupProgress {
    /// A high-level step began (toolchain, unpack, build, manifest).
    Phase(&'static str),
    /// The estimated number of compile units for the build phase, emitted
    /// once before compilation starts. The bar's denominator.
    Total(u32),
    /// One more compile unit finished. `done` is cumulative across the
    /// SDK, runner, and wrapper cargo invocations.
    Compiled { crate_name: String, done: u32 },
    /// A line of rendered cargo diagnostics, for a log tail.
    Log(String),
}

/// Extract the embedded recipe into `dst`, ready for `cargo build`. Files
/// already present with identical bytes are left untouched so their mtimes
/// (and cargo's build fingerprints) survive a re-run: only the first setup
/// pays the full compile.
pub fn write_recipe(dst: &Path) -> std::io::Result<()> {
    for (rel, bytes) in crate::RECIPE_FILES {
        let path = dst.join(rel);
        if std::fs::read(&path).is_ok_and(|existing| existing == *bytes) {
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, bytes)?;
    }
    prune_removed_crates(dst)
}

/// Delete crate directories the recipe no longer ships.
///
/// Writing is otherwise purely additive, and the recipe's root declares
/// `members = ["crates/*"]`, so a crate dropped between versions stays
/// on disk and stays a workspace member. If it was dropped *because* it
/// could not resolve there, the cache is permanently broken and no
/// amount of upgrading fixes it: the user would have to know to delete
/// `~/.jackdaw/sdk` by hand, which nothing tells them.
fn prune_removed_crates(dst: &Path) -> std::io::Result<()> {
    let Ok(entries) = std::fs::read_dir(dst.join("crates")) else {
        return Ok(());
    };
    let shipped: std::collections::BTreeSet<&str> = crate::RECIPE_FILES
        .iter()
        .filter_map(|(rel, _)| rel.strip_prefix("crates/"))
        .filter_map(|rest| rest.split('/').next())
        .collect();
    for entry in entries.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        let name = entry.file_name();
        if !shipped.contains(name.to_string_lossy().as_ref()) {
            std::fs::remove_dir_all(entry.path())?;
        }
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
/// The cache is treated like a dev checkout: the recipe is built in place
/// under `<cache>/build/` and `SdkPaths` points at that build's `target/`
/// (via `for_workspace_profile`), so nothing is copied and the manifest's
/// artifact paths stay valid.
pub fn ensure_sdk(mut report: impl FnMut(SetupProgress)) -> Result<PathBuf, String> {
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

    // Fail before the long build if a hard prerequisite is missing, with
    // an actionable message instead of a cryptic mid-compile error.
    let missing: Vec<String> = check_prerequisites()
        .into_iter()
        .filter(|p| !p.ok)
        .map(|p| match p.fix {
            Some(fix) => format!("{} ({}) - {fix}", p.name, p.detail),
            None => format!("{} ({})", p.name, p.detail),
        })
        .collect();
    if !missing.is_empty() {
        return Err(format!("missing prerequisites: {}", missing.join("; ")));
    }

    report(SetupProgress::Phase("Installing the pinned Rust toolchain"));
    install_toolchain()?;

    let build_dir = cache.join("build");
    report(SetupProgress::Phase("Unpacking SDK sources"));
    // Overwrite the sources in place (no wipe) so a re-run reuses the
    // build cache rather than recompiling from scratch.
    write_recipe(&build_dir).map_err(|e| format!("unpack recipe: {e}"))?;
    // Pin the toolchain for every cargo invocation in this recipe: the
    // build, the manifest enumeration, and the project builds that later
    // resolve this cache all use one rustc, as the rmeta trick requires.
    std::fs::write(
        build_dir.join("rust-toolchain.toml"),
        format!("[toolchain]\nchannel = \"{SDK_TOOLCHAIN_CHANNEL}\"\n"),
    )
    .map_err(|e| format!("write rust-toolchain.toml: {e}"))?;

    report(SetupProgress::Phase(
        "Building the SDK (one-time; this can take several minutes)",
    ));
    build_recipe(&build_dir, &triple, &mut report)?;

    report(SetupProgress::Phase("Writing the SDK manifest"));
    let built = crate::sdk_paths::SdkPaths::for_workspace_profile(&build_dir, "release");
    // The feature sets the three install paths resolve have to nest:
    // a release bundle builds `-p jackdaw --features dylib` (the whole
    // editor), this builds `-p jackdaw_sdk -p jackdaw_runner`, and a
    // project builds its own graph. Each must be a superset of the next.
    // Resolving fewer features than a project does is what breaks: the
    // project compiles code expecting an impl that the SDK rlib it links
    // was built without, and the error names a crate nobody touched.
    // `jackdaw_sdk` depends on the whole runtime with every feature on to
    // keep that ordering; `tests/sdk_feature_closure.rs` guards it.
    // Enumerate artifacts by re-invoking the SAME package set the build
    // phase used (`-p jackdaw_sdk -p jackdaw_runner`). A narrower set (e.g.
    // `-p jackdaw_sdk` alone) resolves different features for shared deps
    // like bevy and forces a full second rebuild; matching it makes this a
    // pure cache hit that only re-reports the artifact filenames. The
    // manifest still filters to jackdaw_sdk's runtime closure, so the extra
    // runner artifacts drop out.
    crate::plan::SdkManifest::generate(
        &build_dir,
        &built,
        &["-p", "jackdaw_sdk", "-p", "jackdaw_runner", "--release"],
    )
    .map_err(|e| format!("generate SDK manifest: {e}"))?;

    write_stamp(&cache, &Stamp::current(&triple, crate::RECIPE_HASH))
        .map_err(|e| format!("write stamp: {e}"))?;
    gc_other_versions();
    report(SetupProgress::Phase("SDK ready"));
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
        return Err(format!(
            "failed to install the {SDK_TOOLCHAIN_CHANNEL} toolchain"
        ));
    }
    Ok(())
}

fn build_recipe(
    build_dir: &Path,
    triple: &str,
    report: &mut impl FnMut(SetupProgress),
) -> Result<(), String> {
    // A denominator for the bar, best-effort: the union build closure of
    // the three artifacts. Undershoots slightly (a crate can compile to
    // several units); the UI clamps overshoot.
    if let Some(total) = estimate_units(build_dir, triple) {
        report(SetupProgress::Total(total));
    }
    // `done` is cumulative so the bar advances continuously across both
    // cargo invocations rather than resetting for the wrapper.
    let mut done = 0u32;
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
        &mut done,
        report,
    )?;
    // The rustc wrapper is a host tool; build it without `--target` so it
    // lands in `target/release`, where `for_workspace_profile` looks.
    run_cargo(
        build_dir,
        &["build", "--release", "-p", "jackdaw_rustc_wrapper"],
        &mut done,
        report,
    )
}

/// Run one cargo build, streaming progress. cargo's status lines and its
/// TTY progress bar go to inherited stderr, so a CLI user keeps the
/// familiar live output; the machine-readable artifact stream on stdout is
/// parsed into [`SetupProgress`] events so the editor (which has no
/// terminal) can drive its own bar. `done` accumulates across calls.
fn run_cargo(
    build_dir: &Path,
    args: &[&str],
    done: &mut u32,
    report: &mut impl FnMut(SetupProgress),
) -> Result<(), String> {
    use std::io::BufRead;

    let mut child = Command::new("cargo")
        .arg(format!("+{SDK_TOOLCHAIN_CHANNEL}"))
        .args(args)
        .arg("--message-format=json-render-diagnostics")
        .env("CARGO_INCREMENTAL", "0")
        .current_dir(build_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit())
        .spawn()
        .map_err(|e| format!("cargo build: {e}"))?;

    let stdout = child.stdout.take().expect("piped stdout");
    for line in std::io::BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
    {
        report_cargo_line(&line, done, report);
    }

    let status = child.wait().map_err(|e| format!("cargo build: {e}"))?;
    if !status.success() {
        return Err("SDK build failed (see the cargo output above)".to_string());
    }
    Ok(())
}

/// Turn one line of `cargo --message-format=json-render-diagnostics` into
/// setup events: a finished compile unit bumps `done`; a rendered
/// diagnostic becomes log lines. Non-JSON or other records are ignored.
fn report_cargo_line(line: &str, done: &mut u32, report: &mut impl FnMut(SetupProgress)) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    match value.get("reason").and_then(serde_json::Value::as_str) {
        Some("compiler-artifact") => {
            *done += 1;
            let crate_name = value
                .get("target")
                .and_then(|t| t.get("name"))
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string();
            report(SetupProgress::Compiled {
                crate_name,
                done: *done,
            });
        }
        Some("compiler-message") => {
            if let Some(rendered) = value
                .get("message")
                .and_then(|m| m.get("rendered"))
                .and_then(serde_json::Value::as_str)
            {
                for l in rendered.lines() {
                    report(SetupProgress::Log(l.to_string()));
                }
            }
        }
        _ => {}
    }
}

/// Estimate the build phase's compile-unit count from the union normal +
/// build dependency closure of the three artifacts (`cargo tree`). A
/// denominator for the progress bar; `None` on any failure, and the UI
/// then shows a running count instead of a filled bar.
fn estimate_units(build_dir: &Path, triple: &str) -> Option<u32> {
    let output = Command::new("cargo")
        .arg(format!("+{SDK_TOOLCHAIN_CHANNEL}"))
        .args([
            "tree",
            "-p",
            "jackdaw_sdk",
            "-p",
            "jackdaw_runner",
            "-p",
            "jackdaw_rustc_wrapper",
            "--target",
            triple,
            "-e",
            "normal,build",
            "--prefix",
            "none",
        ])
        .current_dir(build_dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut units = std::collections::BTreeSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        if let (Some(name), Some(version)) = (parts.next(), parts.next()) {
            units.insert(format!("{name} {version}"));
        }
    }
    (!units.is_empty()).then_some(units.len() as u32)
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
