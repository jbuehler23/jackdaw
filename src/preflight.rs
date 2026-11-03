//! Environment preflight checks.
//!
//! Run before building a user project so a missing toolchain or native build
//! tool (a C/C++ compiler, MSVC, the Xcode Command Line Tools, Bevy's
//! Linux libraries) surfaces in seconds (with a fix) instead of failing a
//! multi-minute build at the end. The checks shell out or read the
//! environment, so run them off the main thread. `run_all_checks` batches
//! them for the launcher.

use std::{
    path::{Path, PathBuf},
    process::Command,
};

use jackdaw_env::{sdk_toolchain_command, user_toolchain_command};

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

/// The Rust version jackdaw's own crates need, from the workspace manifest.
const MIN_RUST_VERSION: &str = env!("CARGO_PKG_RUST_VERSION");

/// Every check that applies on this platform. `open_project` is the project
/// in the editor; without one the most recently opened project is used, and
/// without any the toolchain is checked as the editor sees it. Call off the
/// main thread.
pub fn run_all_checks(open_project: Option<PathBuf>) -> Vec<CheckResult> {
    let project = open_project.or_else(|| {
        crate::project::read_recent_projects()
            .projects
            .into_iter()
            .next()
            .map(|entry| entry.path)
    });
    let mut out = vec![check_rust_toolchain(project.as_deref())];
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
/// A game builds with whatever its own folder selects, so when `project` is
/// known its compiler is held to the version jackdaw needs.
pub fn check_rust_toolchain(project: Option<&Path>) -> CheckResult {
    let rustup = first_line(&mut user_toolchain_command("rustup"), &["--version"]).is_some();
    // Asked only once rustup lists it: running a toolchain that is not
    // installed has rustup download it on the spot.
    let editor = (jackdaw_env::sdk_toolchain_installed() != Some(false))
        .then(|| first_line(&mut sdk_toolchain_command("rustc"), &["--version"]))
        .flatten();
    let ambient = first_line(&mut user_toolchain_command("rustc"), &["--version"]);
    let (status, detail, fix) =
        rust_toolchain_status(rustup, editor, ambient, jackdaw_env::RUSTUP_TOOLCHAIN);
    let probe = project.filter(|dir| dir.is_dir()).map(probe_project);
    let (status, detail, fix) =
        with_project_probe((status, detail, fix), probe.as_ref(), MIN_RUST_VERSION);
    CheckResult::new("Rust toolchain", status, detail, fix.as_deref())
}

/// What a project's own folder selects for building.
struct ProjectProbe {
    name: String,
    toolchain_file: Option<PathBuf>,
    outcome: ProbeOutcome,
}

enum ProbeOutcome {
    /// `rustc --version` under the project's toolchain.
    Version(String),
    /// The project asks for a toolchain rustup does not have; running it
    /// would have rustup download it.
    NotInstalled(String),
    Unknown,
}

fn probe_project(dir: &Path) -> ProjectProbe {
    let name = dir.file_name().map_or_else(
        || dir.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let toolchain_file = jackdaw_env::project_toolchain_file(dir);
    let wanted = toolchain_file
        .as_deref()
        .and_then(|file| std::fs::read_to_string(file).ok())
        .and_then(|contents| jackdaw_env::toolchain_file_channel(&contents));
    let outcome = match wanted {
        Some(channel) if jackdaw_env::toolchain_installed(&channel) == Some(false) => {
            ProbeOutcome::NotInstalled(channel)
        }
        _ => {
            let mut command = user_toolchain_command("rustc");
            command.current_dir(dir);
            first_line(&mut command, &["--version"])
                .map_or(ProbeOutcome::Unknown, ProbeOutcome::Version)
        }
    };
    ProjectProbe {
        name,
        toolchain_file,
        outcome,
    }
}

/// Fold what the project builds with into the editor-wide result. A base that
/// already failed stands; otherwise a project compiler below `minimum`, or
/// one that is not installed, is added to it.
fn with_project_probe(
    base: (CheckStatus, String, Option<String>),
    probe: Option<&ProjectProbe>,
    minimum: &str,
) -> (CheckStatus, String, Option<String>) {
    let Some(probe) = probe.filter(|_| base.0 != CheckStatus::Fail) else {
        return base;
    };
    let name = &probe.name;
    let (status, note, fix) = match &probe.outcome {
        ProbeOutcome::Unknown => return base,
        ProbeOutcome::NotInstalled(channel) => (
            CheckStatus::Warn,
            format!("{name} wants the {channel} toolchain, which is not installed"),
            format!("run rustup toolchain install {channel} --profile minimal"),
        ),
        ProbeOutcome::Version(line) => {
            let Some(version) = rustc_release(line) else {
                return base;
            };
            if version_tuple(version) >= version_tuple(minimum) {
                return (base.0, format!("{}; {name} uses {line}", base.1), base.2);
            }
            let fix = match &probe.toolchain_file {
                Some(file) => format!(
                    "raise the channel in {} to at least {minimum}",
                    file.display()
                ),
                None => "run `rustup update stable`".to_string(),
            };
            (
                CheckStatus::Fail,
                format!(
                    "{name} uses Rust {version}, older than the {minimum} minimum jackdaw needs"
                ),
                fix,
            )
        }
    };
    let fix = match base.2 {
        Some(base_fix) => format!("{fix}; {base_fix}"),
        None => fix,
    };
    (status, format!("{note}; {}", base.1), Some(fix))
}

/// `1.96.0` from `rustc 1.96.0 (abcdef 2026-01-01)`.
fn rustc_release(line: &str) -> Option<&str> {
    line.strip_prefix("rustc ")?.split_whitespace().next()
}

/// Major, minor, patch of a release, ignoring any pre-release suffix.
fn version_tuple(version: &str) -> (u64, u64, u64) {
    let mut parts = version
        .split(['-', '+'])
        .next()
        .unwrap_or_default()
        .split('.')
        .map(|part| part.parse().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
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

/// Run `command args` and return its first stdout line, or `None` if it
/// cannot run.
fn first_line(command: &mut Command, args: &[&str]) -> Option<String> {
    let out = command.args(args).output().ok()?;
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

    fn ok() -> (CheckStatus, String, Option<String>) {
        (CheckStatus::Ok, "rustc 1.99.0".into(), None)
    }

    fn probe(outcome: ProbeOutcome, file: Option<&str>) -> ProjectProbe {
        ProjectProbe {
            name: "csg-test".into(),
            toolchain_file: file.map(PathBuf::from),
            outcome,
        }
    }

    fn version(line: &str, file: Option<&str>) -> ProjectProbe {
        probe(ProbeOutcome::Version(line.into()), file)
    }

    #[test]
    fn an_old_project_compiler_fails_with_the_fix() {
        let old = version("rustc 1.96.0 (3f4e5a6b7 2026-01-01)", None);
        let (status, detail, fix) = with_project_probe(ok(), Some(&old), "1.97.1");
        assert_eq!(status, CheckStatus::Fail);
        assert!(detail.starts_with("csg-test uses Rust 1.96.0, older than the 1.97.1 minimum"));
        assert!(
            detail.ends_with("rustc 1.99.0"),
            "editor detail kept: {detail}"
        );
        assert_eq!(fix.as_deref(), Some("run `rustup update stable`"));

        let pinned = version(
            "rustc 1.96.0 (x 2026-01-01)",
            Some("/p/rust-toolchain.toml"),
        );
        let (_, _, fix) = with_project_probe(ok(), Some(&pinned), "1.97.1");
        assert_eq!(
            fix.as_deref(),
            Some("raise the channel in /p/rust-toolchain.toml to at least 1.97.1")
        );
    }

    #[test]
    fn an_adequate_project_compiler_is_reported() {
        let good = version("rustc 1.97.1 (abc 2026-01-01)", None);
        let (status, detail, fix) = with_project_probe(ok(), Some(&good), "1.97.1");
        assert_eq!(status, CheckStatus::Ok);
        assert_eq!(
            detail,
            "rustc 1.99.0; csg-test uses rustc 1.97.1 (abc 2026-01-01)"
        );
        assert!(fix.is_none());
    }

    #[test]
    fn a_missing_project_toolchain_warns_without_running_it() {
        let missing = probe(ProbeOutcome::NotInstalled("1.90.0".into()), Some("/p/t"));
        let (status, detail, fix) = with_project_probe(ok(), Some(&missing), "1.97.1");
        assert_eq!(status, CheckStatus::Warn);
        assert!(detail.contains("csg-test wants the 1.90.0 toolchain"));
        assert!(fix.is_some_and(|fix| fix.contains("rustup toolchain install 1.90.0")));
    }

    #[test]
    fn a_failed_base_diagnosis_stands() {
        let failed = (
            CheckStatus::Fail,
            "rustc not found".to_string(),
            Some("Install Rust via https://rustup.rs".to_string()),
        );
        let old = version("rustc 1.90.0 (x 2026-01-01)", None);
        let (status, detail, _) = with_project_probe(failed, Some(&old), "1.97.1");
        assert_eq!(
            (status, detail.as_str()),
            (CheckStatus::Fail, "rustc not found")
        );
    }

    #[test]
    fn an_unprobed_or_garbled_project_leaves_the_result_alone() {
        let (status, detail, fix) = with_project_probe(ok(), None, "1.97.1");
        assert_eq!(
            (status, detail.as_str(), fix),
            (CheckStatus::Ok, "rustc 1.99.0", None)
        );
        let garbled = version("garbled", None);
        let (_, detail, _) = with_project_probe(ok(), Some(&garbled), "1.97.1");
        assert_eq!(detail, "rustc 1.99.0");
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(version_tuple("1.9.0") < version_tuple("1.10.0"));
        assert!(version_tuple("1.97.1") > version_tuple("1.97"));
        assert_eq!(version_tuple("1.99.0-nightly"), (1, 99, 0));
        assert_eq!(rustc_release("rustc 1.96.0 (x 2026-01-01)"), Some("1.96.0"));
    }
}
