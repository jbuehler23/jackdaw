//! Environment preflight checks.
//!
//! Run before building a user project so a missing toolchain or native build
//! tool (cmake, a C/C++ compiler, MSVC, the Xcode Command Line Tools, Bevy's
//! Linux libraries) surfaces in seconds (with a fix) instead of failing a
//! multi-minute build at the end. The checks shell out or read the
//! environment, so run them off the main thread. `run_all_checks` batches
//! them for the launcher.

use std::process::Command;

use jackdaw_env::rust_env_command;

pub use jackdaw_project_build::build_tools::CheckStatus;

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
    let mut out = vec![check_rust_toolchain()];
    out.extend(
        jackdaw_project_build::build_tools::check_build_tools()
            .into_iter()
            .map(|check| CheckResult {
                label: check.name.to_string(),
                status: check.status,
                detail: check.detail,
                fix: check.fix,
            }),
    );
    out
}

/// `rustup` and `rustc` present. Extensions build with the toolchain that
/// built this editor, which jackdaw installs through rustup on first use.
pub fn check_rust_toolchain() -> CheckResult {
    let rustup = first_line("rustup", &["--version"]).is_some();
    let editor = first_line("rustc", &["--version"]);
    let ambient = Command::new("rustc")
        .env_remove("RUSTUP_TOOLCHAIN")
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .map(|l| l.trim().to_string())
        });
    let (status, detail, fix) =
        rust_toolchain_status(rustup, editor, ambient, jackdaw_env::RUSTUP_TOOLCHAIN);
    CheckResult::new("Rust toolchain", status, detail, fix.as_deref())
}

/// Pure logic for [`check_rust_toolchain`]: `editor` is `rustc --version`
/// under the editor's toolchain, `ambient` under the user's default.
fn rust_toolchain_status(
    rustup: bool,
    editor: Option<String>,
    ambient: Option<String>,
    toolchain: &str,
) -> (CheckStatus, String, Option<String>) {
    match (editor, ambient) {
        (Some(version), _) | (None, Some(version)) if !rustup => (
            CheckStatus::Fail,
            format!("{version} without rustup; jackdaw installs {toolchain} through rustup"),
            Some("Install rustup from https://rustup.rs".to_string()),
        ),
        (Some(version), _) => (CheckStatus::Ok, version, None),
        (None, Some(version)) => (
            CheckStatus::Warn,
            format!("{version}; extensions need {toolchain}, which is not installed yet"),
            Some(format!(
                "jackdaw installs {toolchain} when an extension first builds, or run \
                 rustup toolchain install {toolchain} --profile minimal"
            )),
        ),
        (None, None) => (
            CheckStatus::Fail,
            "rustc not found".to_string(),
            Some("Install Rust via https://rustup.rs".to_string()),
        ),
    }
}

/// Run `cmd args` and return its first stdout line, or `None` if it cannot run.
fn first_line(cmd: &str, args: &[&str]) -> Option<String> {
    let out = rust_env_command(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_editor_toolchain_warns_until_installed() {
        let installed = rust_toolchain_status(true, Some("rustc 1.99.0".into()), None, "1.99.0");
        assert_eq!(installed.0, CheckStatus::Ok);
        let pending = rust_toolchain_status(true, None, Some("rustc 1.100.0".into()), "1.99.0");
        assert_eq!(pending.0, CheckStatus::Warn);
        assert!(pending.2.is_some_and(|fix| fix.contains("1.99.0")));
        let absent = rust_toolchain_status(true, None, None, "1.99.0");
        assert_eq!(absent.0, CheckStatus::Fail);
    }

    #[test]
    fn rustc_without_rustup_fails() {
        let distro = rust_toolchain_status(false, None, Some("rustc 1.98.0".into()), "1.99.0");
        assert_eq!(distro.0, CheckStatus::Fail);
        assert!(distro.2.is_some_and(|fix| fix.contains("rustup.rs")));
    }
}
