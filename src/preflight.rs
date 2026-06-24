//! Environment preflight checks.
//!
//! Run before building a user project so a missing toolchain, a missing cmake,
//! or a Windows linker misconfiguration surfaces in seconds (with a fix) instead
//! of failing a multi-minute build at the end. The individual `check_*`
//! functions each shell out or read the environment, so run them off the main
//! thread and report results as they complete for live reporting in the
//! launcher. `run_all_checks` batches them for the `jackdaw doctor` CLI.

use bevy::app::AppExit;

/// Outcome of a single check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    /// Good to go.
    Ok,
    /// Build can proceed but may misbehave; the fix is advisory.
    Warn,
    /// The build will fail until the fix is applied.
    Fail,
}

/// A single preflight check result.
#[derive(Debug, Clone)]
pub struct CheckResult {
    pub label: String,
    pub status: CheckStatus,
    pub detail: String,
    pub fix: Option<String>,
}

impl CheckResult {
    fn new(label: &str, status: CheckStatus, detail: impl Into<String>, fix: Option<&str>) -> Self {
        Self {
            label: label.to_string(),
            status,
            detail: detail.into(),
            fix: fix.map(str::to_string),
        }
    }
}

/// Every check that applies on this platform. Call off the main thread.
pub fn run_all_checks() -> Vec<CheckResult> {
    let out = vec![check_rust_toolchain(), check_cmake()];
    // The Windows linker check is the only platform-specific entry; shadow with
    // a mutable binding there so non-Windows builds keep an immutable `out`.
    #[cfg(windows)]
    let out = {
        let mut out = out;
        out.push(check_windows_linker());
        out
    };
    out
}

/// `rustc` present, and ideally a nightly channel (jackdaw targets nightly).
pub fn check_rust_toolchain() -> CheckResult {
    match first_line("rustc", &["--version"]) {
        Some(version) => {
            let (status, fix) = classify_rustc(&version);
            CheckResult::new("Rust toolchain", status, version, fix)
        }
        None => CheckResult::new(
            "Rust toolchain",
            CheckStatus::Fail,
            "rustc not found",
            Some("Install Rust via https://rustup.rs"),
        ),
    }
}

/// `cmake` present. Building the editor compiles jackdaw's CSG kernel
/// (`manifold-csg-sys`), which builds a C++ library with cmake.
pub fn check_cmake() -> CheckResult {
    match first_line("cmake", &["--version"]) {
        Some(version) => CheckResult::new("cmake", CheckStatus::Ok, version, None),
        None => CheckResult::new(
            "cmake",
            CheckStatus::Fail,
            "cmake not found",
            Some("Install cmake (https://cmake.org/download) and ensure it is on PATH"),
        ),
    }
}

/// On Windows, warn if a MinGW `gcc` is on PATH without `CMAKE_GENERATOR`
/// forcing MSVC: cmake then builds `manifold-csg-sys` with MinGW and the object
/// files fail to link (`LNK1143`).
#[cfg(windows)]
pub fn check_windows_linker() -> CheckResult {
    let gcc_found = first_line("gcc", &["--version"]).is_some();
    let generator = std::env::var("CMAKE_GENERATOR").ok();
    let (status, detail, fix) = windows_linker_status(gcc_found, generator.as_deref());
    CheckResult::new("Windows C++ toolchain", status, detail, fix.as_deref())
}

/// Classify a `rustc --version` line. Nightly is the supported channel.
fn classify_rustc(version: &str) -> (CheckStatus, Option<&'static str>) {
    if version.contains("nightly") {
        (CheckStatus::Ok, None)
    } else {
        (
            CheckStatus::Warn,
            Some("jackdaw targets a nightly toolchain: `rustup default nightly`"),
        )
    }
}

/// Pure logic for the Windows linker check, split out for testing. Compiled on
/// Windows (where the check runs) and under `test` (where it is unit-tested).
#[cfg(any(windows, test))]
fn windows_linker_status(
    gcc_found: bool,
    cmake_generator: Option<&str>,
) -> (CheckStatus, String, Option<String>) {
    let forces_msvc = cmake_generator
        .map(|g| g.contains("Visual Studio"))
        .unwrap_or(false);
    if !gcc_found {
        (
            CheckStatus::Ok,
            "No MinGW gcc on PATH; cmake will use MSVC".to_string(),
            None,
        )
    } else if forces_msvc {
        (
            CheckStatus::Ok,
            "MinGW gcc is on PATH but CMAKE_GENERATOR forces Visual Studio".to_string(),
            None,
        )
    } else {
        (
            CheckStatus::Warn,
            "MinGW gcc is on PATH; cmake may pick it over MSVC and fail to link (LNK1143)"
                .to_string(),
            Some(
                "Set CMAKE_GENERATOR=\"Visual Studio 17 2022\" before building (see the install guide)"
                    .to_string(),
            ),
        )
    }
}

/// Run `cmd args` and return its first stdout line, or `None` if it cannot run.
fn first_line(cmd: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
}

/// CLI entry point for `jackdaw doctor`.
#[expect(
    clippy::print_stdout,
    reason = "CLI subcommand writes its report to the terminal"
)]
pub fn run_doctor_cli() -> AppExit {
    let results = run_all_checks();
    println!("jackdaw doctor:");
    let mut any_fail = false;
    for r in &results {
        let tag = match r.status {
            CheckStatus::Ok => "ok  ",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "FAIL",
        };
        println!("  [{tag}] {}: {}", r.label, r.detail);
        if let Some(fix) = &r.fix {
            println!("         fix: {fix}");
        }
        any_fail |= r.status == CheckStatus::Fail;
    }
    if any_fail {
        AppExit::error()
    } else {
        AppExit::Success
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nightly_rustc_is_ok_stable_warns() {
        let (status, _) = classify_rustc("rustc 1.90.0-nightly (abcdef 2026-03-05)");
        assert_eq!(status, CheckStatus::Ok);
        let (status, fix) = classify_rustc("rustc 1.89.0 (stable)");
        assert_eq!(status, CheckStatus::Warn);
        assert!(fix.is_some());
    }

    #[test]
    fn windows_linker_warns_only_on_unguarded_mingw() {
        // No gcc -> fine.
        assert_eq!(windows_linker_status(false, None).0, CheckStatus::Ok);
        // gcc present, no generator forcing MSVC -> warn with a fix.
        let (status, _, fix) = windows_linker_status(true, None);
        assert_eq!(status, CheckStatus::Warn);
        assert!(fix.is_some());
        // gcc present but generator forces Visual Studio -> fine.
        assert_eq!(
            windows_linker_status(true, Some("Visual Studio 17 2022")).0,
            CheckStatus::Ok
        );
    }
}
