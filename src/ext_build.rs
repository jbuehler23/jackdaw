//! Editor-driven build pipeline for extension and game projects.
//!
//! User-scaffolded projects are plain single-crate cargo projects
//! with `bevy = "0.18"` in `[dependencies]` and `crate-type =
//! ["cdylib"]` on the library. Jackdaw compiles them via `cargo
//! build` with `RUSTC_WRAPPER` pointing at `jackdaw-rustc-wrapper`,
//! which intercepts rustc and rewrites `--extern bevy=<user>.rlib`
//! to `--extern bevy=libjackdaw_sdk.so`. That keeps the user's
//! cdylib `TypeIds` in sync with the editor.
//!
//! Why not `bevy build`? The bevy CLI's build subcommand requires
//! a binary target and errors on library-only projects ("No
//! binaries available!"). Scaffolded jackdaw projects are cdylibs
//! so the editor can `dlopen` them, so `bevy build` can't drive
//! them. We still use `bevy new` for scaffolding; that part of
//! the toolchain fits cleanly.
//!
//! [`build_extension_project`] is the simple entry point.
//! [`build_extension_project_with_progress`] additionally streams
//! per-crate progress + tailing log lines into a shared sink the
//! UI can read each frame. The function blocks until cargo exits;
//! use an `AsyncComputeTaskPool` task for non-blocking builds.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::new_project::TemplateLinkage;
use crate::sdk_paths::SdkPaths;

/// Everything that can go wrong while building an extension/game
/// project.
#[derive(Debug)]
pub enum BuildError {
    NotADirectory(PathBuf),
    MissingCargoToml(PathBuf),
    SdkNotFound {
        expected_path: PathBuf,
        hint: &'static str,
    },
    WrapperNotFound {
        expected_path: PathBuf,
        hint: &'static str,
    },
    BuildSpawn(std::io::Error),
    BuildFailed {
        status: std::process::ExitStatus,
        stderr_tail: String,
    },
    OutputNotProduced {
        expected: PathBuf,
    },
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotADirectory(p) => write!(f, "{} is not a directory", p.display()),
            Self::MissingCargoToml(p) => {
                write!(f, "{} has no Cargo.toml", p.display())
            }
            Self::SdkNotFound {
                expected_path,
                hint,
            } => write!(
                f,
                "SDK dylib not found at {}. {}",
                expected_path.display(),
                hint
            ),
            Self::WrapperNotFound {
                expected_path,
                hint,
            } => write!(
                f,
                "rustc wrapper not found at {}. {}",
                expected_path.display(),
                hint
            ),
            Self::BuildSpawn(e) => write!(f, "failed to spawn cargo: {e}"),
            Self::BuildFailed {
                status,
                stderr_tail,
            } => {
                write!(f, "cargo exited with {status}\n{stderr_tail}")
            }
            Self::OutputNotProduced { expected } => write!(
                f,
                "cargo succeeded but no .so was produced at {}",
                expected.display()
            ),
        }
    }
}

impl std::error::Error for BuildError {}

/// Capacity of the rolling log-tail buffer surfaced in progress UI.
const LOG_TAIL_CAPACITY: usize = 20;

/// Live progress from a running cargo build. Writers: the build
/// helper's stdout/stderr reader threads. Reader: the UI poller
/// that renders the progress bar + log tail each frame. Wrap in
/// `Arc<Mutex<_>>` when handing to a long-running task.
///
/// `artifacts_total` is `Some` once we've run `cargo metadata` to
/// compute the expected number of compile units; until then the UI
/// should render an indeterminate bar (or just the counter).
#[derive(Debug, Default, Clone)]
pub struct BuildProgress {
    pub current_crate: Option<String>,
    pub artifacts_done: u32,
    pub artifacts_total: Option<u32>,
    pub recent_log_lines: VecDeque<String>,
    /// Set to `true` by the helper once cargo exits (success or
    /// failure). The UI can use this to flip the bar to 100%.
    pub finished: bool,
}

impl BuildProgress {
    pub fn push_log(&mut self, line: String) {
        if self.recent_log_lines.len() >= LOG_TAIL_CAPACITY {
            self.recent_log_lines.pop_front();
        }
        self.recent_log_lines.push_back(line);
    }

    /// 0.0 when unknown, 1.0 when done.
    pub fn fraction(&self) -> Option<f32> {
        if self.finished {
            return Some(1.0);
        }
        let total = self.artifacts_total? as f32;
        if total <= 0.0 {
            return None;
        }
        Some((self.artifacts_done as f32 / total).clamp(0.0, 1.0))
    }
}

/// Discover `libjackdaw_sdk` + `jackdaw-rustc-wrapper` on disk, or
/// surface a typed error the Build-and-Install dialog can translate
/// into a user-actionable message.
fn discover_sdk() -> Result<SdkPaths, BuildError> {
    let paths = SdkPaths::compute();
    if !paths.dylib_exists() {
        return Err(BuildError::SdkNotFound {
            expected_path: paths.dylib,
            hint: "Rebuild the editor with `--features dylib` so \
                   libjackdaw_sdk is emitted, or set JACKDAW_SDK_DIR \
                   to the directory that contains it.",
        });
    }
    if !paths.wrapper_exists() {
        return Err(BuildError::WrapperNotFound {
            expected_path: paths.wrapper,
            hint: "Run `cargo build -p jackdaw_rustc_wrapper` so the \
                   wrapper binary is emitted next to the editor.",
        });
    }
    Ok(paths)
}

/// Build the extension or game project rooted at `project_dir`.
///
/// Convenience wrapper around
/// [`build_extension_project_with_progress`] that ignores progress.
pub fn build_extension_project(
    project_dir: &Path,
    linkage: TemplateLinkage,
) -> Result<PathBuf, BuildError> {
    build_extension_project_with_progress(project_dir, None, linkage)
}

/// Build the project and (optionally) stream progress into `sink`.
///
/// While cargo runs, a reader thread parses its stdout (JSON
/// records from `--message-format=json-render-diagnostics`) and
/// updates `sink.artifacts_done` + `sink.current_crate` on each
/// `compiler-artifact` message. A separate thread tails stderr
/// (which carries `json-render-diagnostics`' human-readable lines)
/// into `sink.recent_log_lines`.
pub fn build_extension_project_with_progress(
    project_dir: &Path,
    sink: Option<Arc<Mutex<BuildProgress>>>,
    linkage: TemplateLinkage,
) -> Result<PathBuf, BuildError> {
    let project_dir = project_dir
        .canonicalize()
        .map_err(|_| BuildError::NotADirectory(project_dir.to_path_buf()))?;

    if !project_dir.is_dir() {
        return Err(BuildError::NotADirectory(project_dir));
    }
    let manifest = project_dir.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(BuildError::MissingCargoToml(project_dir));
    }

    // The wrapper only applies to dylib builds. A static project
    // depends on `jackdaw` directly; rewriting `--extern bevy` to the
    // SDK dylib gives it a bevy whose hash doesn't match what cargo
    // compiled, and the build fails with `can't find crate for jackdaw`.
    let sdk = match linkage {
        TemplateLinkage::Dylib => Some(discover_sdk()?),
        TemplateLinkage::Static => None,
    };

    // Best-effort: probe the expected artifact count via cargo
    // metadata before kicking off the real build. Runs in the
    // current thread because it's usually <1s and we want the
    // total to be present by the first frame the UI polls.
    if let Some(ref s) = sink
        && let Some(total) = estimate_total_artifacts(&project_dir)
        && let Ok(mut g) = s.lock()
    {
        g.artifacts_total = Some(total);
    }

    let mut cmd = Command::new("cargo");
    cmd.current_dir(&project_dir);
    cmd.args([
        "build",
        "--manifest-path",
        manifest
            .to_str()
            .expect("Cargo.toml path must be valid UTF-8"),
        "--message-format=json-render-diagnostics",
    ]);
    if let Some(sdk) = sdk.as_ref() {
        cmd.env("RUSTC_WRAPPER", &sdk.wrapper);
        cmd.env("JACKDAW_SDK_DYLIB", &sdk.dylib);
        cmd.env("JACKDAW_SDK_DEPS", &sdk.deps);
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let mut child = cmd.spawn().map_err(BuildError::BuildSpawn)?;

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let stdout_sink = sink.clone();
    let stdout_handle = thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            parse_json_line(&line, stdout_sink.as_ref());
        }
    });

    let stderr_sink = sink.clone();
    let stderr_tail: Arc<Mutex<VecDeque<String>>> =
        Arc::new(Mutex::new(VecDeque::with_capacity(LOG_TAIL_CAPACITY)));
    let stderr_tail_for_thread = Arc::clone(&stderr_tail);
    let stderr_handle = thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            if let Some(ref s) = stderr_sink
                && let Ok(mut g) = s.lock()
            {
                g.push_log(line.clone());
            }
            if let Ok(mut tail) = stderr_tail_for_thread.lock() {
                if tail.len() >= LOG_TAIL_CAPACITY {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        }
    });

    let status = child.wait().map_err(BuildError::BuildSpawn)?;
    let _ = stdout_handle.join();
    let _ = stderr_handle.join();

    if let Some(ref s) = sink
        && let Ok(mut g) = s.lock()
    {
        g.finished = true;
    }

    if !status.success() {
        let tail = stderr_tail
            .lock()
            .map(|t| t.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default();
        return Err(BuildError::BuildFailed {
            status,
            stderr_tail: tail,
        });
    }

    match linkage {
        TemplateLinkage::Dylib => {
            let artifact_name = artifact_file_name(&project_dir);
            let artifact = project_dir.join("target/debug").join(&artifact_name);
            if !artifact.is_file() {
                return Err(BuildError::OutputNotProduced { expected: artifact });
            }
            Ok(artifact)
        }
        // Static builds have no cdylib to install; return the project
        // dir so the caller can `enter_project(..)` on it.
        TemplateLinkage::Static => Ok(project_dir),
    }
}

/// Parse a single line from `cargo --message-format=json-…`. On a
/// `compiler-artifact` record, bump `artifacts_done` + update
/// `current_crate`. Errors are swallowed; cargo sometimes emits
/// non-JSON prefix lines, which we ignore.
fn parse_json_line(line: &str, sink: Option<&Arc<Mutex<BuildProgress>>>) {
    let Some(sink) = sink else { return };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
        return;
    };
    let reason = value.get("reason").and_then(|v| v.as_str()).unwrap_or("");
    if reason == "compiler-artifact" {
        let name = value
            .get("target")
            .and_then(|t| t.get("name"))
            .and_then(|n| n.as_str())
            .map(std::string::ToString::to_string);
        if let Ok(mut g) = sink.lock() {
            g.artifacts_done = g.artifacts_done.saturating_add(1);
            if let Some(n) = name {
                g.current_crate = Some(n);
            }
        }
    } else if reason == "compiler-message" {
        // Human-readable rendered text for warnings/errors comes in
        // the `message.rendered` field. Forward those lines into the
        // tail buffer alongside stderr's rendered output.
        if let Some(rendered) = value
            .get("message")
            .and_then(|m| m.get("rendered"))
            .and_then(|r| r.as_str())
            && let Ok(mut g) = sink.lock()
        {
            for l in rendered.lines().take(LOG_TAIL_CAPACITY) {
                g.push_log(l.to_string());
            }
        }
    }
}

/// Run `cargo metadata` to count the packages in the resolve set.
/// Returns `None` on any failure; the progress UI will render an
/// indeterminate bar instead.
///
/// On a fresh bevy project the first `cargo metadata` call takes a
/// few seconds (it resolves the dep graph and hits the registry).
/// Subsequent calls use cargo's cache and finish in <200 ms. The
/// caller runs this on the main thread before spawning the real
/// build, so the progress bar can render a denominator from the
/// first frame onward.
fn estimate_total_artifacts(project_dir: &Path) -> Option<u32> {
    let output = Command::new("cargo")
        .current_dir(project_dir)
        .args(["metadata", "--format-version=1"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let packages = value.get("packages")?.as_array()?;
    // Each package produces roughly one artifact; build scripts and
    // proc-macros add a few more. Close enough for a progress bar.
    Some(packages.len() as u32)
}

/// Run `cargo clean -p <package>` for the project rooted at
/// `project_dir`. Used by the auto-recovery path: when the editor
/// SDK is rebuilt, the user's project `.so` cached in
/// `<project>/target/debug/` still references the old SDK symbol
/// hashes. Cleaning just the user's package (not `-p bevy`) drops
/// that stale artifact without forcing a multi-minute bevy
/// rebuild; the bevy rlib cache stays.
///
/// Blocks until cargo exits. Call from a task pool.
pub fn cargo_clean_project(project_dir: &Path) -> Result<(), BuildError> {
    let project_dir = project_dir
        .canonicalize()
        .map_err(|_| BuildError::NotADirectory(project_dir.to_path_buf()))?;
    let manifest = project_dir.join("Cargo.toml");
    if !manifest.is_file() {
        return Err(BuildError::MissingCargoToml(project_dir));
    }
    let package_name = package_name_from_manifest(&project_dir);

    let mut cmd = Command::new("cargo");
    cmd.current_dir(&project_dir);
    cmd.args([
        "clean",
        "--manifest-path",
        manifest
            .to_str()
            .expect("Cargo.toml path must be valid UTF-8"),
        "-p",
        &package_name,
    ]);
    let output = cmd.output().map_err(BuildError::BuildSpawn)?;
    if !output.status.success() {
        return Err(BuildError::BuildFailed {
            status: output.status,
            stderr_tail: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(())
}

/// Parse `name = "..."` out of a project's `Cargo.toml`. Shared
/// with [`artifact_file_name`]; when the manifest doesn't declare
/// a name (shouldn't happen for anything cargo accepted), returns
/// `"unnamed"`.
fn package_name_from_manifest(project_dir: &Path) -> String {
    std::fs::read_to_string(project_dir.join("Cargo.toml"))
        .ok()
        .and_then(|contents| {
            contents.lines().find_map(|line| {
                let trimmed = line.trim();
                trimmed
                    .strip_prefix("name")
                    .and_then(|rest| rest.trim().strip_prefix('='))
                    .map(|rest| rest.trim().trim_matches('"').trim_matches('\'').to_owned())
            })
        })
        .unwrap_or_else(|| "unnamed".to_string())
}

/// Quick scan of a project's `Cargo.toml` to decide whether
/// `cargo build` would actually produce a cdylib. Used at project-
/// open time: if the manifest is a plain binary crate (e.g., the
/// editor's own source tree, or a user opening any non-extension
/// cargo project) we skip the build pipeline entirely and let them
/// in; otherwise `cargo build` compiles the whole dep tree only to
/// fail the artifact check at the end.
///
/// Same line-based parsing style as [`package_name_from_manifest`]
/// to avoid pulling in a toml dep. Handles the two shapes scaffolded
/// projects use: `crate-type = ["cdylib"]` and
/// `crate-type = ["rlib", "cdylib"]`.
pub(crate) fn manifest_declares_cdylib(project_dir: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(project_dir.join("Cargo.toml")) else {
        return false;
    };
    contents.lines().any(|line| {
        let trimmed = line.trim();
        let Some(rest) = trimmed.strip_prefix("crate-type") else {
            return false;
        };
        let Some(rest) = rest.trim_start().strip_prefix('=') else {
            return false;
        };
        rest.contains("\"cdylib\"") || rest.contains("'cdylib'")
    })
}

/// Derive the expected cdylib filename from the project's package
/// name. Falls back to `libunnamed.<ext>` if the manifest doesn't
/// declare a name (which cargo would have rejected anyway, but it
/// keeps this helper infallible).
pub(crate) fn artifact_file_name(project_dir: &Path) -> String {
    let package_name = package_name_from_manifest(project_dir);

    if cfg!(target_os = "windows") {
        format!("{package_name}.dll")
    } else if cfg!(target_os = "macos") {
        format!("lib{package_name}.dylib")
    } else {
        format!("lib{package_name}.so")
    }
}
