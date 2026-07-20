//! Thin rustc wrapper for jackdaw extension and game projects.
//!
//! # What it does
//!
//! Cargo invokes this binary as `RUSTC_WRAPPER`, so every rustc call
//! in the project passes through here. Target-side invocations are
//! rewritten so the whole dependency graph shares the SDK's crates:
//!
//! * `--extern bevy=<anything>` becomes
//!   `--extern bevy=$JACKDAW_SDK_DYLIB` for every consumer. The user's
//!   Cargo.toml still declares a normal bevy dependency so bevy's proc
//!   macros find it via `CARGO_MANIFEST_DIR` and emit `::bevy::...`
//!   paths. Cargo compiles real bevy into the project's target dir;
//!   those rlibs are ignored because every `--extern` that matters
//!   points at the SDK's artifacts.
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
//! | `CARGO_PRIMARY_PACKAGE`  | (set by cargo) | `1` while compiling the user crate   |
//! | `CARGO_PKG_NAME`         | (set by cargo) | Consumer key for plan edge lookups   |

use std::env;
use std::ffi::OsString;
use std::process::{Command, ExitCode};
use tracing::error;

const ENV_SDK_DYLIB: &str = "JACKDAW_SDK_DYLIB";
const ENV_SDK_DEPS: &str = "JACKDAW_SDK_DEPS";
const ENV_SDK_HOST_DEPS: &str = "JACKDAW_SDK_HOST_DEPS";
const ENV_PRIMARY_PACKAGE: &str = "CARGO_PRIMARY_PACKAGE";
const ENV_LOG: &str = "JACKDAW_WRAPPER_LOG";
const ENV_EXTERN_MAP: &str = "JACKDAW_SDK_EXTERN_MAP";
// Static-SDK model: when set to "1", bevy and jackdaw_api are redirected
// to their prebuilt rlibs and `-C prefer-dynamic` is not added, so the
// project dylib embeds one shared bevy compilation (matching TypeIds via
// the rmeta trick) rather than linking a `libjackdaw_sdk` dll. This is the
// only model that links on Windows, where a dll cannot export bevy's
// reflect statics.
const ENV_STATIC: &str = "JACKDAW_SDK_STATIC";
const ENV_SDK_BEVY_RLIB: &str = "JACKDAW_SDK_BEVY_RLIB";
const ENV_SDK_API_RLIB: &str = "JACKDAW_SDK_API_RLIB";

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

    let is_primary = env::var_os(ENV_PRIMARY_PACKAGE).is_some_and(|v| v == "1");
    let log = env::var_os(ENV_LOG).is_some_and(|v| v == "1");

    // The redirects apply to EVERY target-side crate in the graph, so
    // ecosystem dependencies (physics, etc.) compile against the same
    // SDK bevy and closure crates the user's crate does: one instance
    // of every shared crate. Host-side units pass through unchanged.
    if let Err(e) = rewrite_args(&mut rustc_args, is_primary, log) {
        error!("jackdaw-rustc-wrapper: {e}");
        return ExitCode::from(1);
    }

    let status = Command::new(&rustc).args(&rustc_args).status();

    match status {
        Ok(s) => ExitCode::from(s.code().unwrap_or(1) as u8),
        Err(e) => {
            error!("jackdaw-rustc-wrapper: failed to spawn {rustc:?}: {e}");
            ExitCode::from(1)
        }
    }
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
/// Host-side units (no `--target` flag) pass through untouched. Every
/// target-side unit is rewritten uniformly, including compiles of
/// crates the plan itself replaces: a unit compiled vanilla could
/// consume a rewritten sibling and see two instances of one crate, so
/// coherence requires the SDK-preferred resolution to apply everywhere
/// or nowhere.
fn rewrite_args(argv: &mut Vec<OsString>, is_primary: bool, log: bool) -> Result<(), String> {
    let deps = env::var_os(ENV_SDK_DEPS)
        .ok_or_else(|| format!("{ENV_SDK_DEPS} not set; cannot point -L at deps/"))?;
    let static_mode = env::var_os(ENV_STATIC).is_some_and(|v| v == "1");
    // Redirect targets for the bevy facade and the injected jackdaw_api. In
    // the shared-dylib model both point at libjackdaw_sdk; in the static
    // model each points at its own prebuilt rlib, so rustc embeds one
    // shared bevy compilation instead of linking a dll.
    let (bevy_target, api_target) = if static_mode {
        (
            env::var_os(ENV_SDK_BEVY_RLIB)
                .ok_or_else(|| format!("{ENV_SDK_BEVY_RLIB} not set in static mode"))?,
            env::var_os(ENV_SDK_API_RLIB)
                .ok_or_else(|| format!("{ENV_SDK_API_RLIB} not set in static mode"))?,
        )
    } else {
        let dylib = env::var_os(ENV_SDK_DYLIB)
            .ok_or_else(|| format!("{ENV_SDK_DYLIB} not set; cannot redirect --extern"))?;
        (dylib.clone(), dylib)
    };
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

    let mut redirected = false;
    let mut i = 0;
    while i < argv.len() {
        if argv[i] == "--extern" && i + 1 < argv.len() {
            if let Some(new_value) =
                rewrite_extern(&argv[i + 1], &bevy_target, &extern_map, &consumer)
            {
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

    // `-C prefer-dynamic` links through the SDK dll. In the static model
    // there is no dll: the redirected rlibs are embedded, so it is omitted
    // (and would otherwise pull the toolchain's dynamic std/test crates).
    if !static_mode && (redirected || is_primary) {
        argv.push(OsString::from("-C"));
        argv.push(OsString::from("prefer-dynamic"));
        if log {
            error!("jackdaw-rustc-wrapper: appended -C prefer-dynamic");
        }
    }

    Ok(())
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
        if let Some((edge, artifact)) = line.split_once('=')
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
}

impl ExternMap {
    fn edge_artifact(&self, consumer: &str, alias: &str) -> Option<&OsString> {
        self.edges
            .iter()
            .find(|(c, a, _)| c == consumer && a == alias)
            .map(|(_, _, artifact)| artifact)
    }
}

/// If `value` is `<alias>=<path>` with a redirect target, return the
/// redirected form. The `bevy` facade goes to `bevy_target` (the SDK dll
/// in the shared model, the prebuilt bevy rlib in the static model); an
/// edge listed in the plan for this consumer goes to its recorded
/// artifact. Otherwise `None`, and the caller leaves the flag alone.
fn rewrite_extern(
    value: &OsString,
    bevy_target: &OsString,
    extern_map: &ExternMap,
    consumer: &str,
) -> Option<OsString> {
    let s = value.to_str()?;
    let (alias, _rest) = s.split_once('=')?;
    if REDIRECTED_CRATES.contains(&alias) {
        let mut out = OsString::from(alias);
        out.push("=");
        out.push(bevy_target);
        return Some(out);
    }
    let artifact = extern_map.edge_artifact(consumer, alias)?;
    let mut out = OsString::from(alias);
    out.push("=");
    out.push(artifact);
    Some(out)
}
