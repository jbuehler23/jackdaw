//! Thin rustc wrapper for jackdaw extension and game projects.
//!
//! # What it does
//!
//! Cargo invokes this binary as `RUSTC_WRAPPER`, so every rustc call
//! in the project passes through here. Target-side invocations are
//! rewritten so the whole dependency graph shares the SDK's crates:
//!
//! * The generated shim's `--extern bevy=<anything>` becomes
//!   `--extern bevy=$JACKDAW_SDK_DYLIB`, guaranteeing the final project
//!   dylib imports the SDK facade. Other crates compile against the
//!   exact SDK-built Bevy facade rlib from the redirect plan; that rlib
//!   imports the shared `bevy_dylib` without exposing Jackdaw's merged
//!   facade to third-party crates.
//! * Every dependency edge listed in the `$JACKDAW_SDK_EXTERN_MAP`
//!   redirect plan is rewritten to the SDK artifact it names. The plan
//!   covers the SDK's full runtime closure (bevy subcrates plus public
//!   deps like glam and serde), per edge, only where the project's
//!   resolved version is byte-identical to the SDK's.
//! * `--extern jackdaw_api=$JACKDAW_SDK_DYLIB` is injected for the
//!   primary crate. The user never declares `jackdaw_api`; the wrapper
//!   makes `use jackdaw_api::...` work anyway.
//! * `-L dependency=$JACKDAW_SDK_DEPS` is appended so rustc can find
//!   transitive rlib metadata when resolving re-exported types.
//! * `-C prefer-dynamic` is appended so rustc links through the SDK
//!   dylib rather than statically embedding its rlib form.
//!
//! Host-side invocations (no `--target`: build scripts, proc-macro
//! crates and their deps) and compiles of plan-replaced packages pass
//! through untouched. The driving cargo invocation must therefore
//! always pass an explicit `--target` (the host triple).
//!
//! # Why the wrapper exists as a library plus a binary
//!
//! The logic lives in a library so the build driver can call [`run`]
//! in-process, and ships as a binary so rustc can exec it as
//! `RUSTC_WRAPPER`.
//!
//! # Why
//!
//! Cargo's `-Cmetadata` hash is not stable across independent
//! workspaces, so "build bevy twice and hope the hashes line up"
//! doesn't work. Forcing the user crate to link against the one
//! `libjackdaw_sdk.so` shipped with the editor makes every
//! `TypeId::of::<T>()` in user code agree with the editor's copy,
//! which is what reflection and dlopen require.
//!
//! # Env vars the wrapper reads
//!
//! | Var                      | Required       | Purpose                              |
//! |--------------------------|----------------|--------------------------------------|
//! | `JACKDAW_SDK_DYLIB`      | yes            | Absolute path to `libjackdaw_sdk.so` |
//! | `JACKDAW_SDK_DEPS`       | yes            | Absolute path to the `deps/` dir     |
//! | `JACKDAW_SDK_HOST_DEPS`  | no             | Host deps dir (proc-macro dylibs)    |
//! | `JACKDAW_SDK_EXTERN_MAP` | no             | Path to the per-edge redirect plan   |
//! | `JACKDAW_WRAPPER_LOG`    | no             | If `1`, log rewrites to stderr       |
//! | `CARGO_PKG_NAME`         | (set by cargo) | Consumer key for plan edge lookups   |

use std::env;
use std::ffi::OsString;
use std::process::{Command, ExitCode};
use tracing::{error, warn};

const ENV_SDK_DYLIB: &str = "JACKDAW_SDK_DYLIB";
const ENV_SDK_DEPS: &str = "JACKDAW_SDK_DEPS";
const ENV_SDK_HOST_DEPS: &str = "JACKDAW_SDK_HOST_DEPS";
// Newline-separated directories holding native import libraries the
// SDK's crates reference. Paths can contain spaces and, on Windows, a
// colon after the drive letter, so neither is usable as a separator.
const ENV_SDK_LINK_PATHS: &str = "JACKDAW_SDK_LINK_PATHS";
const ENV_LOG: &str = "JACKDAW_WRAPPER_LOG";
const ENV_EXTERN_MAP: &str = "JACKDAW_SDK_EXTERN_MAP";
const PRIMARY_SHIM: &str = "jackdaw_shim@0.0.0";
/// Crate aliases we redirect to `libjackdaw_sdk.so` whenever cargo
/// emits an `--extern` flag for them. User code writes
/// `use bevy::prelude::*;` and cargo passes `--extern bevy=<stub>.rlib`
/// to rustc; we rewrite the value here.
const REDIRECTED_CRATES: &[&str] = &["bevy"];

/// Crate aliases we inject unconditionally so `use jackdaw_api::...`
/// resolves without the user having to declare `jackdaw_api` in
/// their Cargo.toml. The rustc command picks up these `--extern`
/// flags exactly as cargo-emitted ones would be.
const INJECTED_CRATES: &[&str] = &["jackdaw_api"];

/// Entry point for both the standalone wrapper binary in this crate
/// and the wrapper binary shipped by the top-level `jackdaw` package.
/// Returns the exit code rustc produced (or 1 on a wrapper-side
/// failure).
pub fn run() -> ExitCode {
    // Stderr only: cargo parses this wrapper's stdout during its
    // `--print=file-names` probe invocations, and any stray line there
    // corrupts cargo's idea of what every unit emits.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let mut argv: Vec<OsString> = env::args_os().collect();
    // argv[0] is our binary; argv[1] is the real rustc path; argv[2..]
    // are rustc's args.
    if argv.len() < 2 {
        error!("jackdaw-rustc-wrapper: no rustc path provided");
        return ExitCode::from(1);
    }
    let rustc = argv.remove(1);
    let mut rustc_args: Vec<OsString> = argv.split_off(1);

    let log = env::var_os(ENV_LOG).is_some_and(|v| v == "1");

    // The redirects apply to EVERY target-side crate in the graph, so
    // ecosystem dependencies (physics, etc.) compile against the same
    // SDK bevy and closure crates the user's crate does: one instance
    // of every shared crate. Host-side units pass through unchanged.
    if let Err(e) = rewrite_args(&mut rustc_args, log) {
        error!("jackdaw-rustc-wrapper: {e}");
        return ExitCode::from(1);
    }

    // Trace the FIRST attempt for crates named in the comma-separated
    // list. The failure re-run above only explains deterministic
    // failures: a unit that fails once and re-runs clean needs its
    // original attempt traced, and only the caller knows which unit
    // that is.
    let mut cmd = Command::new(&rustc);
    cmd.args(&rustc_args);
    if let (Ok(traced), Ok(pkg)) = (
        env::var("JACKDAW_WRAPPER_TRACE_CRATES"),
        env::var("CARGO_PKG_NAME"),
    ) && traced.split(',').any(|t| t.trim() == pkg)
    {
        cmd.env("RUSTC_LOG", "rustc_metadata::locator=info");
    }
    let status = cmd.status();

    // One retry before reporting failure. On macOS runners a unit
    // sporadically fails to load a sibling artifact that demonstrably
    // exists and validates - the victim crate changes run to run, and
    // identical re-runs succeed. The real failures this could mask are
    // deterministic and fail the retry identically, costing one extra
    // compile of one crate on the way to the same error.
    let status = match status {
        Ok(s) if !s.success() => {
            warn!(
                "jackdaw-rustc-wrapper: rustc failed for {}; retrying the unit once",
                env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "<unknown>".into()),
            );
            let retry = Command::new(&rustc).args(&rustc_args).status();
            if let Ok(r) = &retry
                && r.success()
            {
                warn!(
                    "jackdaw-rustc-wrapper: the retry succeeded; the failure was transient and the build is fine"
                );
            }
            retry
        }
        other => other,
    };

    match status {
        Ok(s) if s.success() => ExitCode::from(0),
        Ok(s) => {
            // The rewrite is invisible until it goes wrong, and then it
            // goes wrong as a rustc error against a crate nobody named.
            // Reporting the externs this unit actually received turns
            // "can't find crate for X" into something that says whether
            // X was redirected, left alone, or handed over with no path
            // at all.
            error!(
                "jackdaw-rustc-wrapper: rustc failed for {}; externs it received: {}",
                env::var("CARGO_PKG_NAME").unwrap_or_else(|_| "<unknown>".into()),
                describe_externs(&rustc_args)
            );
            // A `can't find crate` for a crate that WAS handed over as a
            // valid rlib means rustc rejected the candidate for a reason
            // its diagnostic does not name. The crate locator logs the
            // reason; re-running the failed unit once with that trace on
            // turns the next CI failure into a self-explaining one.
            // Opt-in because the trace is large.
            //
            // The re-run's exit is reported because it has disagreed
            // with the original: two macOS runs failed a unit whose
            // identical re-run resolved every crate cleanly, which is
            // the signature of a racy read of something a concurrent
            // unit was still writing, not of a broken SDK.
            if env::var_os("JACKDAW_WRAPPER_EXPLAIN").is_some_and(|v| v == "1") {
                error!("jackdaw-rustc-wrapper: re-running with the crate locator trace");
                let rerun = Command::new(&rustc)
                    .args(&rustc_args)
                    .env("RUSTC_LOG", "rustc_metadata::locator=debug")
                    .status();
                match rerun {
                    Ok(r) if r.success() => error!(
                        "jackdaw-rustc-wrapper: the re-run SUCCEEDED with identical \
                         arguments; the original failure was not deterministic"
                    ),
                    Ok(r) => error!("jackdaw-rustc-wrapper: the re-run failed too ({r})"),
                    Err(e) => error!("jackdaw-rustc-wrapper: could not re-run: {e}"),
                }
            }
            ExitCode::from(s.code().unwrap_or(1) as u8)
        }
        Err(e) => {
            error!("jackdaw-rustc-wrapper: failed to spawn {rustc:?}: {e}");
            ExitCode::from(1)
        }
    }
}

/// The SDK's recorded proc-macro dylibs as crate name -> path, from
/// the `jackdaw_sdk_host_deps.txt` the manifest generation writes. The
/// exact list, not a directory scan: a deps dir accumulates generations
/// and the wrapper must name the one the SDK rlibs were built against.
/// Entries are used as recorded when they exist (a dev checkout records
/// absolute paths); otherwise their basename is resolved against
/// `$JACKDAW_SDK_HOST_DEPS` (a bundle stages basenames beside relocated
/// files). Same layout probing as the native link paths: the file sits
/// beside the manifest, one or two levels above `deps/`.
fn sdk_proc_macros(deps: &OsString) -> std::collections::BTreeMap<String, OsString> {
    let mut out = std::collections::BTreeMap::new();
    let deps = std::path::Path::new(deps);
    let host_deps = env::var_os(ENV_SDK_HOST_DEPS);
    for up in [
        deps.parent(),
        deps.parent().and_then(std::path::Path::parent),
    ] {
        let Some(dir) = up else { continue };
        let Ok(text) = std::fs::read_to_string(dir.join("jackdaw_sdk_host_deps.txt")) else {
            continue;
        };
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let recorded = std::path::Path::new(line);
            let Some(file_name) = recorded.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(stem) = file_name
                .strip_prefix(std::env::consts::DLL_PREFIX)
                .and_then(|rest| rest.strip_suffix(std::env::consts::DLL_SUFFIX))
            else {
                continue;
            };
            let Some((name, _hash)) = stem.rsplit_once('-') else {
                continue;
            };
            let path = if recorded.is_file() {
                recorded.as_os_str().to_os_string()
            } else if let Some(host_deps) = &host_deps {
                let joined = std::path::Path::new(host_deps).join(file_name);
                if !joined.is_file() {
                    continue;
                }
                joined.into_os_string()
            } else {
                continue;
            };
            out.insert(name.to_string(), path);
        }
        break;
    }
    out
}

/// The `--extern` flags an invocation carries, one `alias=>source` per
/// entry: where the path points and how big that file is, or `no path`
/// for the bare form.
///
/// The size is there because every other way an extern can be wrong
/// announces itself differently: a missing file, a truncated one, a
/// wrong-target or wrong-crate rlib each produce their own rustc error.
/// A plain `can't find crate` against a crate that was redirected to a
/// path means the file itself is not what it should be, and its size is
/// the cheapest thing that distinguishes "it is fine" from "it is a
/// stub".
fn describe_externs(args: &[OsString]) -> String {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < args.len() {
        if args[i] == "--extern" {
            let value = args[i + 1].to_string_lossy();
            out.push(match value.split_once('=') {
                Some((alias, path)) => {
                    let size = match std::fs::metadata(path) {
                        Ok(meta) => format!("{} bytes", meta.len()),
                        Err(e) => format!("unreadable: {e}"),
                    };
                    format!("{alias}=>{path} ({size})")
                }
                None => format!("{value}=>no path"),
            });
            i += 2;
            continue;
        }
        i += 1;
    }
    out.join(", ")
}

/// Rewrite one rustc invocation. The `bevy` facade extern redirects to
/// the SDK dylib; any other extern named in the SDK extern map (the
/// SDK's runtime dependency closure: bevy subcrates plus their public
/// deps like `glam` and `serde`) redirects to the exact artifact the
/// SDK was built with. When any redirect fired, a
/// `-L dependency=$JACKDAW_SDK_DEPS` and `-C prefer-dynamic` are
/// appended so the redirected metadata resolves and the final link goes
/// through the dylib. The primary package additionally gets
/// `--extern jackdaw_api=` injected.
///
/// Host-side units (no `--target` flag) pass through untouched. So do
/// compiles of crates the plan itself replaces: cargo schedules those
/// units anyway, but every consumer's edge points at the SDK artifact,
/// so their output is never consumed. They used to be rewritten "for
/// coherence", which compiled them against the SDK's resolution instead
/// of their own graph's: where the two resolutions disagreed (macOS),
/// the shadow `bevy_image` unit failed to load the SDK's `image` and
/// took the whole build down for an artifact nothing needed. Vanilla,
/// a shadow unit consumes its own graph's shadow siblings, which is
/// self-consistent by construction. Non-replaced units still get every
/// edge rewritten, so nothing that is actually used sees two instances
/// of one crate.
fn rewrite_args(argv: &mut Vec<OsString>, log: bool) -> Result<(), String> {
    let deps = env::var_os(ENV_SDK_DEPS)
        .ok_or_else(|| format!("{ENV_SDK_DEPS} not set; cannot point -L at deps/"))?;
    // The SDK dylib is the metadata facade for both public crate aliases. It
    // imports the separately shipped Bevy and Jackdaw runtime dylibs, so the
    // project never embeds their process-wide state.
    let dylib = env::var_os(ENV_SDK_DYLIB)
        .ok_or_else(|| format!("{ENV_SDK_DYLIB} not set; cannot redirect --extern"))?;
    let (bevy_target, api_target) = (dylib.clone(), dylib);
    let extern_map = load_extern_map();

    // Host-side units (no --target: build scripts, proc-macro crates
    // and their deps) run against their own host dep units and must
    // never see SDK artifacts. This requires the driving cargo
    // invocation to pass an explicit --target (the host triple), which
    // is what makes cargo omit the flag on host units.
    if !argv.iter().any(|a| a == "--target") {
        return Ok(());
    }

    // The consumer key is name@version: a graph can hold two versions
    // of one crate, and the plan records a distinct redirect per
    // version. cargo sets both vars for every compile.
    let consumer = match (env::var("CARGO_PKG_NAME"), env::var("CARGO_PKG_VERSION")) {
        (Ok(name), Ok(version)) => format!("{}@{version}", name.replace('-', "_")),
        _ => String::new(),
    };
    // `cargo rustc` can expose CARGO_PRIMARY_PACKAGE to dependency wrapper
    // invocations as well as the selected package. The generated build root
    // has a fixed identity, so use that identity directly: only the shim may
    // compile against Jackdaw's merged facade or receive the injected API.
    let is_primary = consumer == PRIMARY_SHIM;

    if !is_primary
        && let Ok(pkg) = env::var("CARGO_PKG_NAME")
        && extern_map.replaces(&pkg.replace('-', "_"))
    {
        if log {
            error!(
                "jackdaw-rustc-wrapper: {pkg} is plan-replaced; compiling its shadow unit vanilla"
            );
        }
        return Ok(());
    }

    let mut redirected = false;
    let mut i = 0;
    while i < argv.len() {
        if argv[i] == "--extern" && i + 1 < argv.len() {
            if let Some(new_value) = rewrite_extern(
                &argv[i + 1],
                &bevy_target,
                &extern_map,
                &consumer,
                is_primary,
            )? {
                if log {
                    error!(
                        "jackdaw-rustc-wrapper: rewrite --extern {:?} -> {:?}",
                        argv[i + 1],
                        new_value
                    );
                }
                argv[i + 1] = new_value;
                redirected = true;
            }
            i += 2;
            continue;
        }
        i += 1;
    }

    // A rewritten unit consumes SDK rlibs whose metadata pins exact
    // proc-macro hashes, but cargo's `--extern` for a proc macro points
    // at the project's own build of it, and an explicit extern RESTRICTS
    // rustc to its candidates: the `-L` host-deps dir is never even
    // searched. Where the project's macro build happens to hash-match
    // the SDK's, that works by luck; on macOS it did not, and loading
    // SDK `jackdaw_api` failed as a bare "can't find crate". The extern
    // is REPLACED with the SDK's recorded macro, which satisfies both
    // roles with one candidate: the transitive pin (which demands the
    // SDK hash) and the unit's own direct use (macro expansion is
    // identical). Appending instead of replacing was tried first and
    // fails with E0464 "multiple candidates" on the direct use.
    if redirected {
        let sdk_macros = sdk_proc_macros(&deps);
        if !sdk_macros.is_empty() {
            let mut i = 0;
            while i < argv.len() {
                if argv[i] == "--extern" && i + 1 < argv.len() {
                    if let Some(value) = argv[i + 1].to_str()
                        && let Some((alias, path)) = value.split_once('=')
                        && path.ends_with(std::env::consts::DLL_SUFFIX)
                        && let Some(staged) = sdk_macros.get(alias)
                    {
                        let mut flag = OsString::from(alias);
                        flag.push("=");
                        flag.push(staged);
                        if log {
                            error!(
                                "jackdaw-rustc-wrapper: rewrite proc-macro --extern {:?} -> {:?}",
                                argv[i + 1],
                                flag
                            );
                        }
                        argv[i + 1] = flag;
                    }
                    i += 2;
                    continue;
                }
                i += 1;
            }
        }
    }

    if is_primary {
        for alias in INJECTED_CRATES {
            let mut flag = OsString::from(alias);
            flag.push("=");
            flag.push(&api_target);
            argv.push(OsString::from("--extern"));
            argv.push(flag);
            if log {
                error!(
                    "jackdaw-rustc-wrapper: injected --extern {}={}",
                    alias,
                    api_target.to_string_lossy()
                );
            }
        }
    }

    // Every target-side unit gets the SDK deps dir as a search path: a
    // unit that was NOT itself rewritten can still consume a crate that
    // was, and loading that crate's metadata requires resolving the SDK
    // artifacts it references. rustc matches transitive crates by exact
    // SVH, so the extra path cannot shadow the user graph's own crates.
    let mut deps_flag = OsString::from("dependency=");
    deps_flag.push(&deps);
    argv.push(OsString::from("-L"));
    argv.push(deps_flag);
    // SDK rlibs can reference host-side proc-macro dylibs (a MacrosOnly
    // dependency like thiserror's derive crate); those live in the SDK
    // build's host deps dir, not the triple dir.
    if let Some(host_deps) = env::var_os(ENV_SDK_HOST_DEPS) {
        let mut host_flag = OsString::from("dependency=");
        host_flag.push(&host_deps);
        argv.push(OsString::from("-L"));
        argv.push(host_flag);
    }

    // Native search paths from the SDK's build scripts. A crate that
    // ships its own import libraries (`windows` ships `windows.0.52.0.lib`
    // and friends) passes its `#[link]` directives to consumers through
    // crate metadata, but not the directory they live in. Without these
    // the consumer's link fails on a bare library name it cannot find.
    // Never an issue on Linux, where native libraries are system wide.
    for path in native_link_paths(&deps) {
        let mut flag = OsString::from("native=");
        flag.push(&path);
        argv.push(OsString::from("-L"));
        argv.push(flag);
    }

    // Link through the SDK facade (and therefore its Bevy and Jackdaw runtime
    // dylibs) rather than embedding the facade's rlib form.
    if redirected || is_primary {
        argv.push(OsString::from("-C"));
        argv.push(OsString::from("prefer-dynamic"));
        if log {
            error!("jackdaw-rustc-wrapper: appended -C prefer-dynamic");
        }
    }
    append_strict_dylib_codegen(argv);

    Ok(())
}

/// Keep Mach-O/PE project and extension dylibs self-contained for generic code.
///
/// With cross-crate sharing enabled, rustc can assign a monomorphization used
/// by the SDK facade to a runtime dylib, while Mach-O's two-level namespace or
/// PE's import table still requires that otherwise-unreferenced symbol from
/// that exact library. Every target unit goes through this wrapper, so applying
/// the same rule here as in the SDK workspace also covers transitive project
/// dependencies.
fn append_strict_dylib_codegen(argv: &mut Vec<OsString>) {
    let targets_strict_dylib = argv.windows(2).any(|pair| {
        pair[0] == "--target"
            && (pair[1].to_string_lossy().ends_with("apple-darwin")
                || pair[1].to_string_lossy().contains("windows"))
    });
    if targets_strict_dylib && !argv.iter().any(|arg| arg == "-Zshare-generics=no") {
        argv.push(OsString::from("-Zshare-generics=no"));
    }
}

/// Parse `$JACKDAW_SDK_EXTERN_MAP`, the per-project redirect plan the
/// editor generates by joining the SDK's artifact list against the
/// project's resolve graph. Each `consumer@version:alias=artifact`
/// line redirects one dependency edge; the consumer carries its exact
/// version so two versions of one crate get distinct redirects. Lines
/// are only emitted where the consumer's resolved version of the
/// dependency is byte-identical to the SDK's. Absent or unreadable
/// plans degrade to facade-only redirection.
fn load_extern_map() -> ExternMap {
    let mut map = ExternMap::default();
    let Some(path) = env::var_os(ENV_EXTERN_MAP) else {
        return map;
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        error!(
            "jackdaw-rustc-wrapper: could not read extern map at {:?}; \
             falling back to facade-only redirection",
            path
        );
        return map;
    };
    for line in contents.lines() {
        if let Some(name) = line.strip_prefix("replaced:") {
            map.replaced.insert(name.to_string());
        } else if let Some((edge, artifact)) = line.split_once('=')
            && let Some((consumer, alias)) = edge.split_once(':')
        {
            map.edges.push((
                consumer.to_string(),
                alias.to_string(),
                OsString::from(artifact),
            ));
        }
    }
    map
}

#[derive(Default)]
struct ExternMap {
    edges: Vec<(String, String, OsString)>,
    /// Package names the plan holds SDK artifacts for, written by
    /// `write_plan` as `replaced:<name>` lines.
    replaced: std::collections::BTreeSet<String>,
}

impl ExternMap {
    fn edge_artifact(&self, consumer: &str, alias: &str) -> Option<&OsString> {
        self.edges
            .iter()
            .find(|(c, a, _)| c == consumer && a == alias)
            .map(|(_, _, artifact)| artifact)
    }

    /// Whether `name` is itself a crate the plan redirects consumers to
    /// the SDK for. Such a crate still gets compiled by cargo, but its
    /// artifact is dead weight: every consumer's edge points at the SDK
    /// copy instead.
    ///
    /// Keyed on the PACKAGE names the plan records, not on edge aliases:
    /// a renamed dependency (rustix consumes `errno` as `libc_errno`)
    /// splits the two, and an alias-keyed answer sent rustix's shadow
    /// vanilla while errno's stayed rewritten. The two then disagreed
    /// about which `libc` exists, failing the build on a crate nothing
    /// consumes. The alias fallback covers a plan written before the
    /// `replaced:` lines existed.
    fn replaces(&self, name: &str) -> bool {
        if self.replaced.is_empty() {
            return self.edges.iter().any(|(_, alias, _)| alias == name);
        }
        self.replaced.contains(name)
    }
}

/// Given an `--extern` value, return the redirected form. The `bevy`
/// facade goes to `bevy_target` for the primary shim, guaranteeing the final
/// dylib records an SDK dependency. Other crates use the exact prebuilt Bevy
/// facade rlib from the plan; that rlib imports `bevy_dylib`, preserving shared
/// runtime state without making third-party crates compile against Jackdaw's
/// merged facade. `Ok(None)` means no redirect applies and the caller leaves
/// the flag alone.
///
/// `value` is usually `<alias>=<path>`, but cargo also emits a bare
/// `<alias>` with no path, and that form has to be redirected too. Left
/// alone it escapes the plan entirely, and rustc, told a crate exists
/// but given nowhere to find it, fails with a bare
/// `can't find crate for X` against the `use` in whichever crate held
/// the edge. A `-L dependency=` path does not rescue it. That is how a
/// macOS bundle failed on `image`, a crate neither the user nor the
/// editor ever names.
///
/// A redirect target that is not on disk is an error here rather than
/// something rustc discovers, for the same reason: the wrapper knows
/// both the artifact and why it was substituted, so it says so.
fn rewrite_extern(
    value: &OsString,
    bevy_target: &OsString,
    extern_map: &ExternMap,
    consumer: &str,
    is_primary: bool,
) -> Result<Option<OsString>, String> {
    let Some(s) = value.to_str() else {
        return Ok(None);
    };
    let alias = s.split_once('=').map_or(s, |(alias, _path)| alias);
    let target = if REDIRECTED_CRATES.contains(&alias) && is_primary {
        bevy_target
    } else {
        match extern_map.edge_artifact(consumer, alias) {
            Some(artifact) => artifact,
            None if REDIRECTED_CRATES.contains(&alias) => bevy_target,
            None => return Ok(None),
        }
    };
    if !std::path::Path::new(target).exists() {
        return Err(format!(
            "the SDK artifact for `{alias}` (needed by {consumer}) is missing: {}. \
             The SDK's manifest names it but the file is not there, so the SDK is \
             incomplete and every project built against it will fail",
            target.to_string_lossy()
        ));
    }
    let mut out = OsString::from(alias);
    out.push("=");
    out.push(target);
    Ok(Some(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ext(value: &str) -> OsString {
        OsString::from(value)
    }

    #[test]
    fn apple_project_units_disable_shared_generics() {
        let mut args = vec![ext("--target"), ext("aarch64-apple-darwin")];
        append_strict_dylib_codegen(&mut args);
        assert_eq!(
            args.iter()
                .filter(|arg| *arg == "-Zshare-generics=no")
                .count(),
            1
        );

        // Rewriting an already-configured unit is idempotent.
        append_strict_dylib_codegen(&mut args);
        assert_eq!(
            args.iter()
                .filter(|arg| *arg == "-Zshare-generics=no")
                .count(),
            1
        );
    }

    #[test]
    fn windows_project_units_disable_shared_generics() {
        let mut args = vec![ext("--target"), ext("x86_64-pc-windows-msvc")];
        append_strict_dylib_codegen(&mut args);
        assert!(args.iter().any(|arg| arg == "-Zshare-generics=no"));
    }

    #[test]
    fn non_apple_project_units_keep_their_codegen_flags() {
        let mut args = vec![ext("--target"), ext("x86_64-unknown-linux-gnu")];
        append_strict_dylib_codegen(&mut args);
        assert!(!args.iter().any(|arg| arg == "-Zshare-generics=no"));
    }

    /// A staged SDK dir holding `names`, so redirect targets exist.
    fn staged(tag: &str, names: &[&str]) -> std::path::PathBuf {
        let dir = env::temp_dir().join(format!("jackdaw_wrapper_{tag}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("staging dir");
        for name in names {
            std::fs::write(dir.join(name), b"").expect("staged artifact");
        }
        dir
    }

    /// The bevy facade always resolves to the SDK dylib supplied by the
    /// build driver.
    #[test]
    fn the_bevy_facade_uses_the_sdk_dylib() {
        let map = ExternMap::default();
        let sdk = staged("facade", &["libjackdaw_sdk.so", "libbevy-def.rlib"]);

        let dylib = OsString::from(sdk.join("libjackdaw_sdk.so"));
        let rewritten = rewrite_extern(
            &ext("bevy=/proj/target/deps/libbevy-abc.rlib"),
            &dylib,
            &map,
            "my_game@0.1.0",
            true,
        )
        .expect("an existing target is not an error")
        .expect("the facade is always redirected");
        let mut expected = OsString::from("bevy=");
        expected.push(&dylib);
        assert_eq!(rewritten, expected);

        let _ = std::fs::remove_dir_all(&sdk);
    }

    /// Dependencies compile against Bevy's real facade rather than the merged
    /// Jackdaw facade. The selected rlib imports the same `bevy_dylib` as the
    /// SDK, so this changes compile-time crate shape without duplicating state.
    #[test]
    fn a_dependency_uses_the_planned_bevy_facade() {
        let sdk = staged("bevy_edge", &["libjackdaw_sdk.so", "libbevy-def.rlib"]);
        let artifact = OsString::from(sdk.join("libbevy-def.rlib"));
        let map = ExternMap {
            edges: vec![(
                "bevy_enhanced_input@0.26.0".into(),
                "bevy".into(),
                artifact.clone(),
            )],
            replaced: std::collections::BTreeSet::new(),
        };
        let dylib = OsString::from(sdk.join("libjackdaw_sdk.so"));
        let rewritten = rewrite_extern(
            &ext("bevy=/project/deps/libbevy-abc.rlib"),
            &dylib,
            &map,
            "bevy_enhanced_input@0.26.0",
            false,
        )
        .expect("the planned artifact is there")
        .expect("the dependency's Bevy edge is redirected");
        let mut expected = OsString::from("bevy=");
        expected.push(&artifact);
        assert_eq!(rewritten, expected);
        let _ = std::fs::remove_dir_all(&sdk);
    }

    /// A crate nobody has an opinion about passes through untouched.
    #[test]
    fn unlisted_crates_are_left_alone() {
        let map = ExternMap::default();
        let sdk = staged("unlisted", &["libjackdaw_sdk.so"]);
        let dylib = OsString::from(sdk.join("libjackdaw_sdk.so"));
        assert!(
            rewrite_extern(
                &ext("rand=/proj/target/deps/librand-abc.rlib"),
                &dylib,
                &map,
                "my_game@0.1.0",
                false,
            )
            .expect("no redirect is not an error")
            .is_none()
        );
        let _ = std::fs::remove_dir_all(&sdk);
    }

    /// What the failure diagnostic reports. The bare form has to be
    /// distinguishable from a redirected one, because "no path" is the
    /// difference between a redirect that did not fire and one that
    /// pointed somewhere wrong.
    #[test]
    fn the_failure_report_separates_redirected_from_pathless() {
        let args = vec![
            ext("--extern"),
            ext("image=/sdk/deps/libimage-abc.rlib"),
            ext("-C"),
            ext("opt-level=3"),
            ext("--extern"),
            ext("glam"),
        ];
        let described = describe_externs(&args);
        assert!(
            described.contains("image=>/sdk/deps/libimage-abc.rlib"),
            "{described}"
        );
        assert!(
            described.contains("unreadable"),
            "a path that is not there says so: {described}"
        );
        assert!(described.contains("glam=>no path"), "{described}");
    }

    /// The recorded proc-macro list maps crate names to exact staged
    /// files: absolute entries as recorded, basenames resolved against
    /// the host-deps dir, and entries that resolve to nothing dropped.
    #[test]
    fn sdk_proc_macros_resolve_from_the_recorded_list() {
        let root = std::env::temp_dir().join(format!("jackdaw_hostmap_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let triple = root.join("triple");
        let deps = triple.join("deps");
        std::fs::create_dir_all(&deps).unwrap();
        let suffix = std::env::consts::DLL_SUFFIX;
        let prefix = std::env::consts::DLL_PREFIX;
        let abs = root.join(format!("{prefix}serde_derive-aaaa{suffix}"));
        std::fs::write(&abs, b"").unwrap();
        let staged_name = format!("{prefix}jackdaw_api_macros-80d8{suffix}");
        let host_deps = root.join("host-deps");
        std::fs::create_dir_all(&host_deps).unwrap();
        std::fs::write(host_deps.join(&staged_name), b"").unwrap();
        std::fs::write(
            triple.join("jackdaw_sdk_host_deps.txt"),
            format!(
                "{}\n/nowhere/{staged_name}\n/nowhere/{prefix}gone_macro-ffff{suffix}\n",
                abs.display()
            ),
        )
        .unwrap();
        // SAFETY: test-local env mutation, read back immediately.
        unsafe { env::set_var(ENV_SDK_HOST_DEPS, &host_deps) };
        let map = sdk_proc_macros(&OsString::from(deps.as_os_str()));
        unsafe { env::remove_var(ENV_SDK_HOST_DEPS) };
        assert_eq!(
            map.get("serde_derive").cloned(),
            Some(abs.clone().into_os_string()),
            "absolute recorded entries are used as recorded"
        );
        assert!(
            map.get("jackdaw_api_macros")
                .is_some_and(|p| p.to_string_lossy().contains("host-deps")),
            "missing absolute entries resolve by basename: {map:?}"
        );
        assert!(!map.contains_key("gone_macro"), "unresolvable entries drop");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A crate the plan replaces compiles as a shadow unit nothing
    /// consumes; it must pass through vanilla rather than be crossed
    /// into the SDK's resolution.
    #[test]
    fn plan_replaced_crates_are_recognised() {
        let mut map = ExternMap {
            edges: vec![(
                "bevy_render@0.19.0".into(),
                "bevy_image".into(),
                ext("/sdk/deps/libbevy_image-abc.rlib"),
            )],
            replaced: std::collections::BTreeSet::new(),
        };
        // Stale plan without `replaced:` lines: alias fallback.
        assert!(map.replaces("bevy_image"));
        assert!(!map.replaces("bevy_render"), "consumers are not replaced");
        assert!(!map.replaces("my_game"));

        // A recorded set answers by package name, so a renamed
        // dependency's package (errno, consumed as libc_errno) is
        // recognised while the alias is not a package at all.
        map.replaced
            .extend(["bevy_image".to_string(), "errno".to_string()]);
        assert!(map.replaces("errno"));
        assert!(!map.replaces("libc_errno"), "aliases are not packages");
        assert!(!map.replaces("my_game"));
    }

    /// cargo also emits `--extern <alias>` with no path. Left alone it
    /// escapes the plan, and rustc, told the crate exists but given
    /// nowhere to find it, fails with a bare `can't find crate for X`
    /// against a third-party crate. A macOS bundle failed exactly this
    /// way on `image`.
    #[test]
    fn a_pathless_extern_is_redirected_too() {
        let sdk = staged("pathless", &["libjackdaw_sdk.so", "libimage-abc.rlib"]);
        let artifact = OsString::from(sdk.join("libimage-abc.rlib"));
        let map = ExternMap {
            edges: vec![("bevy_image@0.19.0".into(), "image".into(), artifact.clone())],
            replaced: std::collections::BTreeSet::new(),
        };
        let dylib = OsString::from(sdk.join("libjackdaw_sdk.so"));
        let rewritten = rewrite_extern(&ext("image"), &dylib, &map, "bevy_image@0.19.0", false)
            .expect("the artifact is there")
            .expect("a pathless extern still names an edge in the plan");
        let mut expected = OsString::from("image=");
        expected.push(&artifact);
        assert_eq!(rewritten, expected);
        let _ = std::fs::remove_dir_all(&sdk);
    }

    /// An SDK whose manifest names an artifact it did not stage fails
    /// here, naming the crate and the consumer. Left to rustc it becomes
    /// `can't find crate` inside whichever third-party crate held the
    /// edge, which points nowhere near the SDK.
    #[test]
    fn a_missing_sdk_artifact_names_itself() {
        let sdk = staged("missing", &["libjackdaw_sdk.so"]);
        let map = ExternMap {
            edges: vec![(
                "bevy_image@0.19.0".into(),
                "image".into(),
                OsString::from(sdk.join("libimage-abc.rlib")),
            )],
            replaced: std::collections::BTreeSet::new(),
        };
        let dylib = OsString::from(sdk.join("libjackdaw_sdk.so"));
        let err = rewrite_extern(
            &ext("image=/proj/target/deps/libimage-def.rlib"),
            &dylib,
            &map,
            "bevy_image@0.19.0",
            false,
        )
        .expect_err("a redirect to a file that is not there is an error");
        assert!(err.contains("image"), "{err}");
        assert!(err.contains("bevy_image@0.19.0"), "{err}");
        let _ = std::fs::remove_dir_all(&sdk);
    }
}

/// Directories holding native import libraries the SDK's crates link by
/// bare name.
///
/// Taken from `$JACKDAW_SDK_LINK_PATHS` when the build driver set it,
/// and otherwise read from the file the SDK build wrote beside its
/// manifest. The fallback matters: the SDK pipeline tests drive cargo
/// directly rather than through the driver, so an env-only channel
/// reaches production but not them, and they are what proves this on
/// Windows.
fn native_link_paths(deps: &OsString) -> Vec<OsString> {
    if let Some(paths) = env::var_os(ENV_SDK_LINK_PATHS) {
        return paths
            .to_string_lossy()
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(OsString::from)
            .collect();
    }
    // `deps/` sits under the triple dir in a checkout or prepared cache,
    // and under `sdk/<triple>/` in an installed bundle, so the file is
    // one or two levels up depending on the layout.
    let deps = std::path::Path::new(deps);
    let mut out = Vec::new();
    for up in [
        deps.parent(),
        deps.parent().and_then(std::path::Path::parent),
    ] {
        let Some(dir) = up else { continue };
        let Ok(text) = std::fs::read_to_string(dir.join("jackdaw_sdk_link_paths.txt")) else {
            continue;
        };
        out.extend(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(OsString::from),
        );
        break;
    }
    out
}

#[cfg(test)]
mod link_path_tests {
    use super::*;

    /// The SDK pipeline tests drive cargo directly and never set the env
    /// var, so the wrapper has to find the file from the deps dir alone.
    /// Both layouts: `<triple>/deps` in a checkout or prepared cache,
    /// `sdk/<triple>/deps` in an installed bundle.
    #[test]
    fn link_paths_are_found_from_either_layout() {
        let root = std::env::temp_dir().join(format!("jackdaw_wrapper_lp_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        // Checkout / cache: the file sits one level above `deps`.
        let triple = root.join("checkout").join("x86_64").join("release");
        std::fs::create_dir_all(triple.join("deps")).unwrap();
        std::fs::write(
            triple.join("jackdaw_sdk_link_paths.txt"),
            b"/registry/windows_msvc/lib\n",
        )
        .unwrap();
        let found = native_link_paths(&OsString::from(triple.join("deps").as_os_str()));
        assert_eq!(found, vec![OsString::from("/registry/windows_msvc/lib")]);

        // Bundle: `sdk/` is two levels above `deps`.
        let sdk = root.join("bundle").join("sdk");
        std::fs::create_dir_all(sdk.join("x86_64").join("deps")).unwrap();
        std::fs::write(
            sdk.join("jackdaw_sdk_link_paths.txt"),
            b"/opt/jackdaw/sdk/native-libs\n",
        )
        .unwrap();
        let found = native_link_paths(&OsString::from(sdk.join("x86_64").join("deps").as_os_str()));
        assert_eq!(found, vec![OsString::from("/opt/jackdaw/sdk/native-libs")]);

        // No file anywhere is not an error: platforms whose native
        // libraries are system wide record none.
        let bare = root.join("bare").join("deps");
        std::fs::create_dir_all(&bare).unwrap();
        assert!(native_link_paths(&OsString::from(bare.as_os_str())).is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }
}
