//! Locate the SDK dylib, its deps/ dir, and the rustc wrapper.
//!
//! Both [`ext_build`](crate::ext_build) (for invoking cargo) and
//! [`new_project`](crate::new_project) (for writing the scaffolded
//! project's `.cargo/config.toml`) need these paths. The path
//! computation lives here so the two sites can't drift. Callers
//! perform their own existence checks and raise whatever error
//! type makes sense for their surface.
//!
//! Resolution order:
//!
//! 1. `JACKDAW_SDK_DIR` env var, if set.
//! 2. The directory containing the currently-running executable.
//!    With `cargo run --features dylib`, that lands in
//!    `<workspace>/target/debug/` alongside `libjackdaw_sdk.so`.
//!    For installed distributions the layout is expected to match.
//! 3. `.` as a last resort; existence checks at the call site will
//!    then almost certainly fail.

use std::path::PathBuf;

/// Everything `ext_build` and `new_project` need to point
/// cargo-spawned rustc at the editor's SDK. Paths are _computed_,
/// not _verified_; call [`Self::dylib_exists`] / [`Self::wrapper_exists`]
/// or an equivalent before relying on them.
pub struct SdkPaths {
    /// Absolute path to `libjackdaw_sdk.{so,dylib,dll}`.
    pub dylib: PathBuf,
    /// Absolute path to the sibling `deps/` directory (what
    /// `-L dependency=` should point at).
    pub deps: PathBuf,
    /// Absolute path to `jackdaw-rustc-wrapper(.exe)`.
    pub wrapper: PathBuf,
}

impl SdkPaths {
    pub fn compute() -> Self {
        let base = resolve_base_dir();
        Self {
            dylib: base.join(dylib_name()),
            deps: base.join("deps"),
            wrapper: base.join(wrapper_name()),
        }
    }

    pub fn dylib_exists(&self) -> bool {
        self.dylib.is_file()
    }

    pub fn wrapper_exists(&self) -> bool {
        self.wrapper.is_file()
    }
}

fn resolve_base_dir() -> PathBuf {
    if let Ok(from_env) = std::env::var("JACKDAW_SDK_DIR") {
        return PathBuf::from(from_env);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(ToOwned::to_owned))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn dylib_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "jackdaw_sdk.dll"
    } else if cfg!(target_os = "macos") {
        "libjackdaw_sdk.dylib"
    } else {
        "libjackdaw_sdk.so"
    }
}

fn wrapper_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "jackdaw-rustc-wrapper.exe"
    } else {
        "jackdaw-rustc-wrapper"
    }
}
