//! Thin rustc wrapper for jackdaw extension and game projects.
//!
//! # What it does
//!
//! Cargo invokes this binary as `RUSTC_WRAPPER`, so every rustc call
//! in the project passes through here. For the user's primary crate
//! (detected via `CARGO_PRIMARY_PACKAGE=1`) we rewrite the argv:
//!
//! * `--extern bevy=<anything>` becomes
//!   `--extern bevy=$JACKDAW_SDK_DYLIB`. The user's Cargo.toml still
//!   declares `bevy = "0.18"` so bevy's proc macros find it via
//!   `CARGO_MANIFEST_DIR` and emit `::bevy::…` paths. Cargo compiles
//!   real bevy into the user's target dir; the resulting rlib is
//!   ignored because the wrapper points the `--extern` at
//!   `libjackdaw_sdk.so`. The extra compile is a one-time cost that
//!   keeps the user's Cargo.toml normal (no patches, no stub crate).
//! * `--extern jackdaw_api=$JACKDAW_SDK_DYLIB` is injected. The user
//!   never declares `jackdaw_api`; the wrapper makes
//!   `use jackdaw_api::…` work anyway.
//! * `-L dependency=$JACKDAW_SDK_DEPS` is appended so rustc can find
//!   transitive rlib metadata when resolving re-exported types.
//! * `-C prefer-dynamic` is appended so rustc links through the SDK
//!   dylib rather than statically embedding its rlib form.
//!
//! Every other rustc invocation (build scripts, dependency
//! compilation, etc.) passes through untouched.
//!
//! # Why the wrapper exists as a library plus a binary
//!
//! Two binaries call this logic: the standalone
//! `jackdaw-rustc-wrapper` produced by this crate's own `[[bin]]`
//! (used by dev contributors via `cargo build -p jackdaw_rustc_wrapper`),
//! and the `jackdaw-rustc-wrapper` produced by the top-level
//! `jackdaw` package's `[[bin]]` (so `cargo install jackdaw` ships
//! the wrapper alongside the editor binary). Both binaries are one-line
//! shims around [`run`].
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
//! | Var                     | Required       | Purpose                              |
//! |-------------------------|----------------|--------------------------------------|
//! | `JACKDAW_SDK_DYLIB`     | yes            | Absolute path to `libjackdaw_sdk.so` |
//! | `JACKDAW_SDK_DEPS`      | yes            | Absolute path to the `deps/` dir     |
//! | `JACKDAW_WRAPPER_LOG`   | no             | If `1`, log rewrites to stderr       |
//! | `CARGO_PRIMARY_PACKAGE` | (set by cargo) | `1` while compiling the user crate   |

use std::env;
use std::ffi::OsString;
use std::process::{Command, ExitCode};
use tracing::error;

const ENV_SDK_DYLIB: &str = "JACKDAW_SDK_DYLIB";
const ENV_SDK_DEPS: &str = "JACKDAW_SDK_DEPS";
const ENV_PRIMARY_PACKAGE: &str = "CARGO_PRIMARY_PACKAGE";
const ENV_LOG: &str = "JACKDAW_WRAPPER_LOG";

/// Crate aliases we redirect to `libjackdaw_sdk.so` whenever cargo
/// emits an `--extern` flag for them. User code writes
/// `use bevy::prelude::*;` and cargo passes `--extern bevy=<stub>.rlib`
/// to rustc; we rewrite the value here.
const REDIRECTED_CRATES: &[&str] = &["bevy"];

/// Crate aliases we inject unconditionally so `use jackdaw_api::…`
/// resolves without the user having to declare `jackdaw_api` in
/// their Cargo.toml. The rustc command picks up these `--extern`
/// flags exactly as cargo-emitted ones would be.
const INJECTED_CRATES: &[&str] = &["jackdaw_api"];

/// Entry point for both the standalone wrapper binary in this crate
/// and the wrapper binary shipped by the top-level `jackdaw` package.
/// Returns the exit code rustc produced (or 1 on a wrapper-side
/// failure).
pub fn run() -> ExitCode {
    tracing_subscriber::fmt::init();
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

    if is_primary && let Err(e) = rewrite_primary_args(&mut rustc_args, log) {
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

/// Rewrite the rustc argv for the user's primary-package compile.
/// Redirects `--extern bevy=...` and `--extern jackdaw_api=...` to
/// the SDK dylib, appends a `-L dependency=$JACKDAW_SDK_DEPS` so
/// rustc can find transitive rlib metadata, and adds
/// `-C prefer-dynamic` so the linker prefers the dylib form.
fn rewrite_primary_args(argv: &mut Vec<OsString>, log: bool) -> Result<(), String> {
    let dylib = env::var_os(ENV_SDK_DYLIB)
        .ok_or_else(|| format!("{ENV_SDK_DYLIB} not set; cannot redirect --extern"))?;
    let deps = env::var_os(ENV_SDK_DEPS)
        .ok_or_else(|| format!("{ENV_SDK_DEPS} not set; cannot point -L at deps/"))?;

    let mut i = 0;
    while i < argv.len() {
        if argv[i] == "--extern" && i + 1 < argv.len() {
            if let Some(new_value) = rewrite_extern(&argv[i + 1], &dylib) {
                if log {
                    error!(
                        "jackdaw-rustc-wrapper: rewrite --extern {:?} -> {:?}",
                        argv[i + 1],
                        new_value
                    );
                }
                argv[i + 1] = new_value;
            }
            i += 2;
            continue;
        }
        i += 1;
    }

    for alias in INJECTED_CRATES {
        let mut flag = OsString::from(alias);
        flag.push("=");
        flag.push(&dylib);
        argv.push(OsString::from("--extern"));
        argv.push(flag);
        if log {
            error!(
                "jackdaw-rustc-wrapper: injected --extern {}={}",
                alias,
                dylib.to_string_lossy()
            );
        }
    }

    let mut deps_flag = OsString::from("dependency=");
    deps_flag.push(&deps);
    argv.push(OsString::from("-L"));
    argv.push(deps_flag);
    argv.push(OsString::from("-C"));
    argv.push(OsString::from("prefer-dynamic"));

    if log {
        error!(
            "jackdaw-rustc-wrapper: appended -L dependency={} -C prefer-dynamic",
            deps.to_string_lossy()
        );
    }

    Ok(())
}

/// If `value` is `<alias>=<path>` with `<alias>` in
/// [`REDIRECTED_CRATES`], return the redirected form pointing at the
/// SDK dylib. Otherwise return `None` so the caller leaves it alone.
fn rewrite_extern(value: &OsString, sdk_dylib: &OsString) -> Option<OsString> {
    let s = value.to_str()?;
    let (alias, _rest) = s.split_once('=')?;
    if !REDIRECTED_CRATES.contains(&alias) {
        return None;
    }
    let mut out = OsString::from(alias);
    out.push("=");
    out.push(sdk_dylib);
    Some(out)
}
