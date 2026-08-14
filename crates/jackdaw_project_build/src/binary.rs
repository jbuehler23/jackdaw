//! Builds a project as its own cargo binary. The project is a normal Bevy
//! app, so `cargo build` in the project root is the whole build.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use jackdaw_env::rust_env_command;
use jackdaw_schema::{ProjectSchema, SCHEMA_FLAG, parse_from_stdout, write_schema};

use crate::build::{BuildEvent, ProjectBuildError, parse_build_line};
use crate::shim::ShimSpec;

/// A completed binary build.
pub struct ProjectBinaryBuild {
    /// The game executable cargo produced. This is the same artifact a
    /// plain `cargo run` in the project would produce.
    pub binary: PathBuf,
    /// The project's type schema, extracted by running the binary in
    /// schema mode. `None` when extraction could not run; the editor
    /// keeps its previous schema in that case.
    pub schema: Option<ProjectSchema>,
}

/// Build a project's game binary and extract its type schema.
///
/// `jackdaw_dir` is the project's `.jackdaw/`, where the schema is
/// persisted for the editor's watcher to pick up. The build itself runs
/// in the project root against the user's own `Cargo.toml`, `Cargo.lock`,
/// `target/`, and toolchain, so it shares a cache with whatever the user
/// runs from their terminal.
pub fn build_project_binary(
    spec: &ShimSpec,
    jackdaw_dir: &Path,
    report: &mut dyn FnMut(BuildEvent),
) -> Result<ProjectBinaryBuild, ProjectBuildError> {
    report(BuildEvent::Log(format!(
        "building {} as a cargo binary in {}",
        spec.package_name,
        spec.project_root.display()
    )));

    let mut command = rust_env_command("cargo");
    command
        .arg("build")
        .args(["-p", &spec.package_name])
        .arg("--message-format=json-render-diagnostics")
        .current_dir(&spec.project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    detach_from_host_build(&mut command);
    let mut child = command.spawn()?;

    let stdout = child.stdout.take().expect("piped stdout");
    let mut done = 0u32;
    let mut log = String::new();
    let mut executables: Vec<PathBuf> = Vec::new();
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        if let Some(path) = executable_for_package(&line, &spec.package_name) {
            executables.push(path);
        }
        parse_build_line(&line, &mut done, &mut log, report);
    }
    let status = child.wait()?;
    if !status.success() {
        if log.trim().is_empty() {
            log.push_str("project failed to compile (no diagnostics captured)");
        }
        return Err(ProjectBuildError::Compile { log });
    }

    let binary = executables
        .pop()
        .ok_or_else(|| ProjectBuildError::Compile {
            log: no_binary_message(spec),
        })?;

    // Ask the freshly built game for its types. It is a throwaway run
    // that prints and exits, so the editor still never maps project
    // code. A failure here is not fatal: the editor keeps its prior
    // schema and Play still works.
    let schema = match run_binary_extractor(&binary, &spec.project_root) {
        Ok(schema) => Some(schema),
        Err(err) => {
            tracing::warn!("project schema extraction skipped: {err}");
            None
        }
    };

    if let Some(schema) = &schema
        && let Err(err) = write_schema(jackdaw_dir, schema)
    {
        tracing::warn!("failed to persist project schema: {err}");
    }

    Ok(ProjectBinaryBuild { binary, schema })
}

/// Ask a built project binary for its schema.
///
/// The binary is the same artifact Play runs; the flag makes it report
/// and exit before it opens a window. `cwd` is the project root so the
/// game resolves assets exactly as it would when played.
fn run_binary_extractor(binary: &Path, cwd: &Path) -> Result<ProjectSchema, String> {
    if !binary.is_file() {
        return Err(format!("game binary not found at {}", binary.display()));
    }
    let mut command = Command::new(binary);
    command.arg(SCHEMA_FLAG).current_dir(cwd);
    prepare_game_command(&mut command, binary);
    let output = command
        .output()
        .map_err(|e| format!("spawn extractor: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "extractor failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    parse_from_stdout(&output.stdout)
}

/// Variables that describe *this* process's build rather than the
/// project's, removed before invoking cargo on the project.
const HOST_BUILD_VARS: &[&str] = &[
    // The one that matters most: the editor runs under a pinned
    // toolchain, and an inherited `RUSTUP_TOOLCHAIN` overrides the
    // project's own `rust-toolchain.toml`.
    "RUSTUP_TOOLCHAIN",
    "RUSTC",
    "RUSTDOC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "RUSTFLAGS",
    "CARGO_ENCODED_RUSTFLAGS",
    "CARGO",
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET",
    "CARGO_BUILD_TARGET_DIR",
    "CARGO_MANIFEST_DIR",
    "CARGO_MANIFEST_PATH",
    "CARGO_CRATE_NAME",
    "CARGO_BIN_NAME",
    "CARGO_PRIMARY_PACKAGE",
    "CARGO_TARGET_TMPDIR",
    // Set by `cargo run` for the process it launches; inheriting them
    // into a build would put the editor's dependency libraries on the
    // project's search path.
    "LD_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
];

/// Prefixes of the per-crate variables cargo exports to a build.
const HOST_BUILD_PREFIXES: &[&str] = &["CARGO_PKG_", "CARGO_CFG_", "CARGO_FEATURE_"];

/// Detach `command` from the build environment the editor is running
/// under.
///
/// The editor is usually itself launched by cargo, which fills the
/// environment with variables describing the editor's build. Handing
/// those to a build of someone else's project is wrong. Jackdaw
/// pins its own toolchain, and `RUSTUP_TOOLCHAIN` outranks a project's
/// `rust-toolchain.toml`. Inheriting it silently compiles the project
/// with the editor's compiler, which shares no fingerprints with the
/// user's own builds - so every Play recompiles the world, and so does
/// the next `cargo build` in their terminal.
///
/// `CARGO_HOME`, `RUSTUP_HOME`, and `PATH` are deliberately kept: those
/// locate the toolchain and the shared package cache, which is exactly
/// what should be shared.
pub fn detach_from_host_build(command: &mut Command) {
    for variable in HOST_BUILD_VARS {
        command.env_remove(variable);
    }
    // The prefixed ones are per-crate and unbounded, so they have to be
    // discovered from this process's environment rather than listed.
    for (key, _) in std::env::vars_os() {
        if is_host_build_var(&key.to_string_lossy()) {
            command.env_remove(&key);
        }
    }
}

/// Whether a variable describes this process's build rather than the
/// project's, and so must not be inherited by a build or a game.
fn is_host_build_var(name: &str) -> bool {
    HOST_BUILD_VARS.contains(&name)
        || HOST_BUILD_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix))
}

/// Prepare a command that runs a built game: Play, and the schema
/// extractor.
///
/// Detaches from the editor's build environment, then puts the build's
/// `deps/` on the dynamic library search path. Order matters, which is
/// why this is one function rather than two calls at each site: the
/// detach clears the library-path variables, so setting them has to
/// come after.
pub fn prepare_game_command(command: &mut Command, binary: &Path) {
    detach_from_host_build(command);
    apply_dynamic_library_path(command, binary);
}

/// The environment variable holding the dynamic library search path.
fn dynamic_library_path_var() -> &'static str {
    if cfg!(windows) {
        "PATH"
    } else if cfg!(target_os = "macos") {
        "DYLD_FALLBACK_LIBRARY_PATH"
    } else {
        "LD_LIBRARY_PATH"
    }
}

/// Put the build's `deps/` directory on the dynamic library search path
/// for `command`.
///
/// `cargo run` does this before launching a binary, and a game built
/// with `bevy/dynamic_linking` does not start without it: the bevy dll
/// it needs sits in `deps/`, not beside the executable. Anything that
/// launches the artifact directly - Play, and the schema extractor -
/// has to reproduce it, or the game dies at load with an error that
/// names no cause.
fn apply_dynamic_library_path(command: &mut Command, binary: &Path) {
    let Some(deps) = binary.parent().map(|dir| dir.join("deps")) else {
        return;
    };
    if !deps.is_dir() {
        return;
    }
    let variable = dynamic_library_path_var();
    let mut paths = vec![deps];
    if let Some(existing) = std::env::var_os(variable) {
        paths.extend(std::env::split_paths(&existing));
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        command.env(variable, joined);
    }
}

/// The executable path from one `compiler-artifact` line, when that
/// artifact is a binary belonging to `package_name`.
///
/// cargo emits `executable` as null for every non-binary unit, and the
/// graph contains binaries from dependencies too, so both filters are
/// needed to land on the game rather than a build tool that happened to
/// compile alongside it.
fn executable_for_package(line: &str, package_name: &str) -> Option<PathBuf> {
    let value = serde_json::from_str::<serde_json::Value>(line).ok()?;
    if value.get("reason").and_then(serde_json::Value::as_str)? != "compiler-artifact" {
        return None;
    }
    let executable = value.get("executable")?.as_str()?;
    let package_id = value
        .get("package_id")
        .and_then(serde_json::Value::as_str)?;
    if !package_id_names(package_id, package_name) {
        return None;
    }
    Some(PathBuf::from(executable))
}

/// Whether a cargo package id refers to `package_name`. Ids spell the
/// name either in the fragment (`...#my-game@0.1.0`) or, for path
/// packages that carry only a version there, as the last path segment
/// (`path+file:///home/u/my-game#0.1.0`).
fn package_id_names(package_id: &str, package_name: &str) -> bool {
    let normalize = |s: &str| s.replace('-', "_");
    let Some((base, fragment)) = package_id.rsplit_once('#') else {
        return false;
    };
    let name = match fragment.rsplit_once('@') {
        Some((name, _)) => name.to_string(),
        None => base.rsplit(['/', '\\']).next().unwrap_or("").to_string(),
    };
    normalize(&name) == normalize(package_name)
}

/// The error text when the build succeeded but produced no executable.
/// Almost always means the crate is a library with no `src/main.rs`.
fn no_binary_message(spec: &ShimSpec) -> String {
    format!(
        "`{}` built, but produced no executable.\n\
         note: jackdaw runs your game as an ordinary cargo binary, so the package needs a \
         binary target.\n\
         note: add a `src/main.rs` that builds a bevy `App` and adds your game plugin, or run \
         `jd migrate` to have one written for you.",
        spec.package_name,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The editor pins its own toolchain; leaking that into the
    /// project's build compiles it with the wrong compiler, which shares
    /// no fingerprints with the user's own builds.
    #[test]
    fn the_editors_toolchain_and_crate_vars_do_not_reach_the_project() {
        assert!(is_host_build_var("RUSTUP_TOOLCHAIN"));
        assert!(is_host_build_var("CARGO_MANIFEST_DIR"));
        assert!(is_host_build_var("CARGO_TARGET_DIR"));
        assert!(is_host_build_var("CARGO_PKG_VERSION"));
        assert!(is_host_build_var("CARGO_FEATURE_DEFAULT"));
        assert!(is_host_build_var("LD_LIBRARY_PATH"));
    }

    /// Locating the toolchain and the shared package cache is exactly
    /// what should carry over, so a project build reuses what is
    /// already downloaded.
    #[test]
    fn the_shared_toolchain_and_cache_locations_are_kept() {
        assert!(!is_host_build_var("PATH"));
        assert!(!is_host_build_var("CARGO_HOME"));
        assert!(!is_host_build_var("RUSTUP_HOME"));
    }

    #[test]
    fn registry_and_path_package_ids_are_recognised() {
        assert!(package_id_names(
            "registry+https://github.com/rust-lang/crates.io-index#my-game@0.1.0",
            "my-game"
        ));
        assert!(package_id_names(
            "path+file:///home/u/my-game#0.1.0",
            "my-game"
        ));
        assert!(package_id_names(
            "path+file:///home/u/my-game#0.1.0",
            "my_game"
        ));
        assert!(!package_id_names(
            "path+file:///home/u/other#0.1.0",
            "my-game"
        ));
    }

    #[test]
    fn only_binary_artifacts_of_the_game_package_are_taken() {
        let bin = r#"{"reason":"compiler-artifact","package_id":"path+file:///w/my-game#0.1.0","executable":"/w/target/debug/my-game"}"#;
        assert_eq!(
            executable_for_package(bin, "my-game"),
            Some(PathBuf::from("/w/target/debug/my-game"))
        );

        let lib = r#"{"reason":"compiler-artifact","package_id":"path+file:///w/my-game#0.1.0","executable":null}"#;
        assert_eq!(executable_for_package(lib, "my-game"), None);

        let other = r#"{"reason":"compiler-artifact","package_id":"path+file:///w/tool#0.1.0","executable":"/w/target/debug/tool"}"#;
        assert_eq!(executable_for_package(other, "my-game"), None);
    }
}
