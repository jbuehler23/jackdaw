//! Builds a project as its own cargo binary. The project is a normal Bevy
//! app, so `cargo build` in the project root is the whole build.

use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use jackdaw_env::process::StderrCapture;
use jackdaw_env::user_toolchain_command;
use jackdaw_schema::{
    PROJECT_CRATES_ENV, ProjectSchema, SCHEMA_FLAG, parse_from_stdout, write_schema,
};

use crate::build::{BuildEvent, ProjectBuildError, parse_build_line};
use crate::shim::ShimSpec;

/// A completed binary build.
pub struct ProjectBinaryBuild {
    /// The game executable cargo produced. The same program a plain `cargo run`
    /// in the project would produce, with its dependencies optimised unless the
    /// project chose otherwise.
    pub binary: PathBuf,
    /// The project's type schema, extracted by running the binary in
    /// schema mode. `None` when extraction could not run; the editor
    /// keeps its previous schema in that case.
    pub schema: Option<ProjectSchema>,
}

/// How much of the machine a build may take. A build the user asked for gets
/// everything; one the editor started on its own competes with the editor's own
/// frame and with the user's terminal.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum BuildLoad {
    /// All cores at normal priority.
    #[default]
    Foreground,
    /// Half the cores, de-prioritized.
    Background,
}

/// Cores a [`BuildLoad::Background`] build may use on a machine with
/// `total_threads` of them: half, and never fewer than one. Split out from the
/// command builder because the process-wide core count is not testable.
pub fn background_jobs(total_threads: usize) -> usize {
    (total_threads / 2).max(1)
}

/// The `CARGO_BUILD_JOBS` value for a background build, honouring a cap
/// this process already carries: an ambient one lower than half the cores
/// was chosen deliberately and must not be raised.
fn background_jobs_env() -> usize {
    let cores = std::thread::available_parallelism()
        .map(std::num::NonZero::get)
        .unwrap_or(1);
    let ours = background_jobs(cores);
    match std::env::var("CARGO_BUILD_JOBS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
    {
        Some(ambient) if ambient > 0 => ours.min(ambient),
        _ => ours,
    }
}

/// Apply `load` to a cargo invocation. Call after
/// [`detach_from_host_build`], which clears inherited cargo variables.
fn apply_load(command: &mut Command, load: BuildLoad) {
    if load == BuildLoad::Foreground {
        return;
    }
    command.env("CARGO_BUILD_JOBS", background_jobs_env().to_string());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // The whole cargo process group, since `setpriority` is inherited
        // by the rustc children cargo spawns.
        unsafe {
            command.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, 10);
                Ok(())
            });
        }
    }
}

/// Where the editor builds a project's game binary, which is never the
/// project's own `target/`.
pub fn game_target_dir(jackdaw_dir: &Path) -> PathBuf {
    jackdaw_dir.join("target/game")
}

/// Build a project's game binary and extract its type schema.
///
/// `jackdaw_dir` is the project's `.jackdaw/`, where the schema is persisted for
/// the editor's watcher and where the build's own artifacts land. It builds
/// against the user's own manifest, lockfile and toolchain, but into
/// [`game_target_dir`] rather than the project's `target/`: the editor builds
/// the plain package with optimised dependencies
/// while a developer's own build carries the features and profile their game
/// needs, and the two must not overwrite each other's binary. It travels as a
/// flag rather than as `CARGO_TARGET_DIR`, which `detach_from_host_build` strips
/// along with the rest of the editor's own build environment.
pub fn build_project_binary(
    spec: &ShimSpec,
    jackdaw_dir: &Path,
    report: &mut dyn FnMut(BuildEvent),
) -> Result<ProjectBinaryBuild, ProjectBuildError> {
    build_project_binary_with_load(spec, jackdaw_dir, BuildLoad::Foreground, report)
}

/// [`build_project_binary`], with a say in how much of the machine the
/// cargo run may take.
pub fn build_project_binary_with_load(
    spec: &ShimSpec,
    jackdaw_dir: &Path,
    load: BuildLoad,
    report: &mut dyn FnMut(BuildEvent),
) -> Result<ProjectBinaryBuild, ProjectBuildError> {
    build_project_binary_cancellable(spec, jackdaw_dir, load, &AtomicBool::new(false), report)
}

/// How often a running cargo build checks whether it was cancelled.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// [`build_project_binary_with_load`], killing cargo once `cancel` is set. A
/// cancelled build returns an `Interrupted` io error.
pub fn build_project_binary_cancellable(
    spec: &ShimSpec,
    jackdaw_dir: &Path,
    load: BuildLoad,
    cancel: &AtomicBool,
    report: &mut dyn FnMut(BuildEvent),
) -> Result<ProjectBinaryBuild, ProjectBuildError> {
    let target_dir = game_target_dir(jackdaw_dir);
    report(BuildEvent::Log(format!(
        "building {} as a cargo binary in {}, into {}",
        spec.package_name,
        spec.project_root.display(),
        target_dir.display()
    )));

    // A build on Bevy's main branch hands the game its own Bevy commit; a
    // lockfile at another commit would build a game whose types are not
    // the editor's.
    match crate::bevy_source::pin_project(&spec.project_root) {
        Ok(Some(pinned)) => report(BuildEvent::Log(pinned)),
        Ok(None) => {}
        Err(error) => report(BuildEvent::Log(format!("warning: {error}"))),
    }

    let mut command = game_build_command(spec, &target_dir, load);
    let mut child = command.spawn()?;

    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = StderrCapture::take(&mut child);
    let lines = lines_on_a_thread(stdout);
    let mut done = 0u32;
    let mut log = String::new();
    let mut executables: Vec<PathBuf> = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            report(BuildEvent::Log("build cancelled".to_string()));
            return Err(ProjectBuildError::Io(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "build cancelled",
            )));
        }
        match lines.recv_timeout(CANCEL_POLL) {
            Ok(line) => {
                if let Some(path) = executable_for_package(&line, &spec.package_name) {
                    executables.push(path);
                }
                parse_build_line(&line, &mut done, &mut log, report);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let status = child.wait()?;
    if !status.success() {
        let stderr = stderr.finish();
        // Cargo's own account of the failure never goes through the JSON stream.
        // `Display` for `Compile` is one line, so the detail has to reach the
        // panel line by line the way rustc's diagnostics do.
        for line in stderr.lines() {
            report(BuildEvent::Log(line.to_string()));
        }
        return Err(ProjectBuildError::Compile {
            log: with_toolchain_hint(
                compile_failure_log(log, &stderr),
                &stderr,
                &spec.project_root,
            ),
        });
    }

    let binary = executables
        .pop()
        .ok_or_else(|| ProjectBuildError::Compile {
            log: no_binary_message(spec),
        })?;

    // Ask the freshly built game for its types: a throwaway run that prints and
    // exits. A failure here is not fatal, and the editor keeps its prior schema.
    let schema = match run_binary_extractor(&binary, spec) {
        Ok(schema) => Some(schema),
        Err(err) => {
            tracing::warn!("project schema extraction skipped: {err}");
            report(BuildEvent::Log(format!(
                "project schema extraction skipped: {err}"
            )));
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

/// Profile overrides for a game the editor builds. A game spends its load and
/// its frames in Bevy and the runtime, which are many times slower unoptimised,
/// while the game's own crate stays at the profile's level so a rebuild is quick.
/// Line tables keep backtraces readable without the memory and link time of full
/// debug info on optimised code.
const GAME_DEPENDENCY_OPT_LEVEL: &str = "profile.dev.package.\"*\".opt-level=2";
const GAME_DEPENDENCY_DEBUG: &str = "profile.dev.package.\"*\".debug=\"line-tables-only\"";

/// The profile overrides a game's build takes. Config on the command line outranks
/// the manifest and every config file, so each override stands aside for a project
/// that says anything about it, wherever cargo would read that from. A project that
/// chose a dev optimisation of its own gets neither: line tables are only the right
/// trade on code built optimised. A dev debug setting, in a profile file or
/// `CARGO_PROFILE_DEV_DEBUG`, is a choice about debug info too.
fn game_dependency_profile(project_root: &Path) -> Vec<&'static str> {
    let cargo_home = std::env::var_os("CARGO_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(|home| std::path::PathBuf::from(home).join(".cargo"))
        });
    let env_opt_level = std::env::var_os("CARGO_PROFILE_DEV_OPT_LEVEL").is_some();
    let env_debug = std::env::var_os("CARGO_PROFILE_DEV_DEBUG").is_some();
    dependency_profile_for(
        project_root,
        cargo_home.as_deref(),
        DevProfileEnv {
            opt_level: env_opt_level,
            debug: env_debug,
        },
    )
}

/// Which `CARGO_PROFILE_DEV_*` variables are set.
#[derive(Clone, Copy)]
struct DevProfileEnv {
    opt_level: bool,
    debug: bool,
}

fn dependency_profile_for(
    project_root: &Path,
    cargo_home: Option<&Path>,
    env: DevProfileEnv,
) -> Vec<&'static str> {
    let files = profile_files(project_root, cargo_home);
    let dev_profiles: Vec<toml::Value> = files
        .iter()
        .filter_map(|file| read_toml(file))
        .filter_map(|toml| toml.get("profile")?.get("dev").cloned())
        .collect();
    let dependencies_set = |key: &str| {
        dev_profiles.iter().any(|dev| {
            dev.get("package")
                .and_then(|package| package.get("*"))
                .and_then(|dependencies| dependencies.get(key))
                .is_some()
        })
    };
    let chose_optimisation = env.opt_level
        || dependencies_set("opt-level")
        || dev_profiles
            .iter()
            .any(|dev| dev.get("opt-level").is_some());
    if chose_optimisation {
        return Vec::new();
    }
    let mut settings = vec![GAME_DEPENDENCY_OPT_LEVEL];
    let chose_debug = env.debug
        || dependencies_set("debug")
        || dev_profiles.iter().any(|dev| dev.get("debug").is_some());
    if !chose_debug {
        settings.push(GAME_DEPENDENCY_DEBUG);
    }
    settings
}

/// Where cargo reads a dev profile from for `project_root`: the manifest it takes
/// profiles from, the config files of the project's folder and every parent, and
/// the one in cargo's home.
fn profile_files(project_root: &Path, cargo_home: Option<&Path>) -> Vec<std::path::PathBuf> {
    let root_manifest = project_root
        .ancestors()
        .map(|dir| dir.join("Cargo.toml"))
        .filter(|manifest| manifest.is_file())
        .find(|manifest| read_toml(manifest).is_some_and(|toml| toml.get("workspace").is_some()))
        .unwrap_or_else(|| project_root.join("Cargo.toml"));
    let mut files = vec![root_manifest];
    let config_dirs = project_root
        .ancestors()
        .map(|dir| dir.join(".cargo"))
        .chain(cargo_home.map(Path::to_path_buf));
    for dir in config_dirs {
        files.push(dir.join("config.toml"));
        files.push(dir.join("config"));
    }
    files
}

fn read_toml(file: &Path) -> Option<toml::Value> {
    toml::from_str(&std::fs::read_to_string(file).ok()?).ok()
}

/// The `cargo build` that compiles the game, run by the project's own
/// toolchain and detached from the editor's build environment.
fn game_build_command(spec: &ShimSpec, target_dir: &Path, load: BuildLoad) -> Command {
    let profile = game_dependency_profile(&spec.project_root);
    game_build_command_with(spec, target_dir, load, &profile)
}

fn game_build_command_with(
    spec: &ShimSpec,
    target_dir: &Path,
    load: BuildLoad,
    profile: &[&str],
) -> Command {
    let mut command = user_toolchain_command("cargo");
    command
        .arg("build")
        .args(["-p", &spec.package_name])
        .arg("--target-dir")
        .arg(target_dir);
    for setting in profile {
        command.arg("--config").arg(setting);
    }
    command
        // Plain `json`, not `json-render-diagnostics`: that variant renders
        // diagnostics to cargo's own stderr and emits no `compiler-message`
        // records, leaving the parser below with nothing to report.
        .arg("--message-format=json")
        .current_dir(&spec.project_root)
        .stdout(Stdio::piped())
        // Captured rather than inherited: cargo's own failures, such as an
        // unresolvable dependency, are never in the JSON stream, and inherited
        // they reach only the terminal the editor was launched from.
        .stderr(Stdio::piped());
    detach_from_host_build(&mut command);
    apply_load(&mut command, load);
    command
}

/// What a failed build reports: rustc's diagnostics, then cargo's own stderr.
///
/// rustc's arrive as `compiler-message` records on stdout; cargo's are on stderr
/// and are the only account of a failure before any code was compiled. Either
/// can be empty, so both are carried.
pub(crate) fn compile_failure_log(diagnostics: String, stderr: &str) -> String {
    let mut log = diagnostics;
    let stderr = stderr.trim();
    if !stderr.is_empty() {
        if !log.trim().is_empty() {
            log.push('\n');
        }
        log.push_str(stderr);
        log.push('\n');
    }
    if log.trim().is_empty() {
        log.push_str("project failed to compile, and cargo reported no diagnostics");
    }
    log
}

const TOOLCHAIN_TOO_OLD: &str = "is not supported by the following package";

/// Cargo's rust-version refusal names the packages but not the way out, which
/// depends on whether the project picks its own toolchain.
fn with_toolchain_hint(mut log: String, stderr: &str, project: &Path) -> String {
    if stderr.contains(TOOLCHAIN_TOO_OLD) {
        log.push_str(&toolchain_too_old_hint(
            jackdaw_env::project_toolchain_file(project).as_deref(),
        ));
        log.push('\n');
    }
    log
}

fn toolchain_too_old_hint(toolchain_file: Option<&Path>) -> String {
    match toolchain_file {
        Some(file) => format!(
            "hint: this project builds with the toolchain {} selects, which is older than its \
             dependencies need. Raise the channel in that file.",
            file.display()
        ),
        None => "hint: this project builds with the rustup default toolchain, which is older \
                 than its dependencies need. Run `rustup update stable`."
            .to_string(),
    }
}

/// How long the extractor may run before it is killed. This path compiles
/// nothing, so only a stuck process reaches the deadline.
const EXTRACTOR_TIMEOUT: Duration = Duration::from_secs(120);

/// Overrides [`EXTRACTOR_TIMEOUT`], in whole seconds.
const EXTRACTOR_TIMEOUT_VAR: &str = "JACKDAW_SCHEMA_EXTRACT_TIMEOUT_SECS";

/// The deadline to run the extractor under, from `value` (the raw environment
/// override). Anything unparseable or zero falls back to the default rather than
/// disabling the deadline.
fn extractor_timeout_from(value: Option<&str>) -> Duration {
    value
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .filter(|seconds| *seconds > 0)
        .map_or(EXTRACTOR_TIMEOUT, Duration::from_secs)
}

fn extractor_timeout() -> Duration {
    extractor_timeout_from(std::env::var(EXTRACTOR_TIMEOUT_VAR).ok().as_deref())
}

/// Ask a built project binary for its schema.
///
/// The binary is the same artifact Play runs; the flag makes it report and exit
/// before it opens a window. `cwd` is the project root so the game resolves
/// assets as it would when played.
///
/// The run is under a deadline, because the extractor answers from inside the
/// game's own `App` build and a project whose plugins block would otherwise hang
/// the editor's build. A kill becomes an ordinary extractor failure.
fn run_binary_extractor(binary: &Path, spec: &ShimSpec) -> Result<ProjectSchema, String> {
    if !binary.is_file() {
        return Err(format!("game binary not found at {}", binary.display()));
    }
    let project_crates =
        crate::cargo_meta::project_crates(&spec.project_root, &spec.package_name, &spec.crate_name);
    let child = extractor_command(binary, spec, &project_crates)
        .spawn()
        .map_err(|e| format!("spawn extractor: {e}"))?;
    let output = wait_with_deadline(child, extractor_timeout())?;
    if !output.status.success() {
        return Err(format!(
            "extractor failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let mut schema = parse_from_stdout(&output.stdout)?;
    schema.project_crates = project_crates;
    Ok(schema)
}

fn extractor_command(binary: &Path, spec: &ShimSpec, project_crates: &[String]) -> Command {
    let mut command = Command::new(binary);
    command
        .arg(SCHEMA_FLAG)
        .current_dir(&spec.project_root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    prepare_game_command(&mut command, binary);
    command.env(PROJECT_CRATES_ENV, project_crates.join(","));
    command
}

/// Each line `stdout` gives, read on a thread of its own so the reader can stop
/// waiting on it.
fn lines_on_a_thread(stdout: impl Read + Send + 'static) -> mpsc::Receiver<String> {
    let (send, lines) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if send.send(line).is_err() {
                return;
            }
        }
    });
    lines
}

/// Collect a child's output, killing it if it outlives `timeout`.
///
/// `Child::wait_with_output` has no deadline and `Child::wait` alone
/// would deadlock against a full pipe, so the pipes are drained on their
/// own threads while the main one polls for exit.
fn wait_with_deadline(mut child: Child, timeout: Duration) -> Result<Output, String> {
    let stdout = child.stdout.take().map(drain_on_a_thread);
    let stderr = child.stderr.take().map(drain_on_a_thread);
    let deadline = Instant::now() + timeout;

    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err(format!("wait for extractor: {e}")),
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "extractor did not finish within {}s (set {EXTRACTOR_TIMEOUT_VAR} to allow longer)",
                timeout.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(25));
    };

    let collect = |handle: Option<std::thread::JoinHandle<Vec<u8>>>| {
        handle.and_then(|h| h.join().ok()).unwrap_or_default()
    };
    Ok(Output {
        status,
        stdout: collect(stdout),
        stderr: collect(stderr),
    })
}

fn drain_on_a_thread(mut pipe: impl Read + Send + 'static) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = pipe.read_to_end(&mut buffer);
        buffer
    })
}

/// Variables that describe *this* process's build rather than the project's,
/// removed before invoking cargo on the project.
const HOST_BUILD_VARS: &[&str] = &[
    // The one that matters most: the editor runs under a pinned toolchain, and
    // an inherited `RUSTUP_TOOLCHAIN` overrides the project's own
    // `rust-toolchain.toml`.
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
    // Set by `cargo run` for the process it launches; inheriting them would put
    // the editor's dependency libraries on the project's search path.
    "LD_LIBRARY_PATH",
    "DYLD_LIBRARY_PATH",
    "DYLD_FALLBACK_LIBRARY_PATH",
];

/// Prefixes of the per-crate variables cargo exports to a build.
const HOST_BUILD_PREFIXES: &[&str] = &["CARGO_PKG_", "CARGO_CFG_", "CARGO_FEATURE_"];

/// Detach `command` from the build environment the editor is running under.
///
/// The editor is usually itself launched by cargo, which fills the environment
/// with variables describing the editor's build. `RUSTUP_TOOLCHAIN` outranks a
/// project's `rust-toolchain.toml`, so inheriting it compiles the project with
/// the editor's compiler, which shares no fingerprints with the user's own
/// builds.
///
/// `CARGO_HOME`, `RUSTUP_HOME` and `PATH` are deliberately kept: they locate the
/// toolchain and the shared package cache.
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

/// Prepare a command that runs a built game: Play, and the schema extractor.
///
/// Keeps it from opening a console window of its own, detaches it from the
/// editor's build environment, then puts the build's `deps/`
/// on the dynamic library search path. One function rather than two calls at
/// each site, because the detach clears the library-path variables.
pub fn prepare_game_command(command: &mut Command, binary: &Path) {
    jackdaw_env::without_console_window(command);
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

/// Put the build's `deps/` directory on the dynamic library search path for
/// `command`.
///
/// `cargo run` does this before launching a binary, and a game built with
/// `bevy/dynamic_linking` does not start without it: the bevy dll it needs sits
/// in `deps/`, not beside the executable.
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

/// The executable path from one `compiler-artifact` line, when that artifact is
/// a binary belonging to `package_name`.
///
/// cargo emits `executable` as null for every non-binary unit, and the graph
/// contains binaries from dependencies too, so both filters are needed.
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

    #[test]
    fn the_editor_builds_the_game_away_from_the_one_a_developer_builds() {
        let project = Path::new("/game");

        let editor = game_target_dir(&project.join(".jackdaw"));

        assert!(
            !editor.starts_with(project.join("target")),
            "an editor build must not land where a developer's own build does, got {}",
            editor.display()
        );
    }

    /// A background build leaves the user half the machine, and never
    /// asks for zero jobs on a single-core box.
    #[test]
    fn a_background_build_takes_half_the_cores() {
        assert_eq!(background_jobs(1), 1);
        assert_eq!(background_jobs(2), 1);
        assert_eq!(background_jobs(12), 6);
        assert_eq!(background_jobs(64), 32);
    }

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

    #[test]
    fn the_extractor_is_told_the_project_s_crates() {
        let spec = ShimSpec {
            package_name: "bevy-jam".to_string(),
            crate_name: "bevy_jam".to_string(),
            project_root: PathBuf::from("/game"),
            extension_type: None,
        };

        let crates = ["bevy_jam".to_string(), "bevy_jam_core".to_string()];
        let command = extractor_command(Path::new("/game/bin"), &spec, &crates);

        let told = command
            .get_envs()
            .find(|(key, _)| *key == PROJECT_CRATES_ENV)
            .and_then(|(_, value)| value)
            .expect("the crate list is set");
        assert_eq!(told, "bevy_jam,bevy_jam_core");
    }

    #[test]
    fn a_game_command_uses_the_projects_toolchain() {
        let spec = ShimSpec {
            package_name: "my-game".to_string(),
            crate_name: "my_game".to_string(),
            project_root: PathBuf::from("/game"),
            extension_type: None,
        };
        let command = game_build_command(&spec, Path::new("/target"), BuildLoad::Foreground);
        let toolchain: Vec<_> = command
            .get_envs()
            .filter(|(key, _)| *key == "RUSTUP_TOOLCHAIN")
            .collect();
        assert_eq!(
            toolchain,
            [(std::ffi::OsStr::new("RUSTUP_TOOLCHAIN"), None)],
            "the editor's toolchain must be removed so the project's own applies"
        );
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

    const NO_ENV: DevProfileEnv = DevProfileEnv {
        opt_level: false,
        debug: false,
    };

    fn build_args(project: &Path) -> Vec<String> {
        let spec = ShimSpec {
            package_name: "game".to_string(),
            crate_name: "game".to_string(),
            project_root: project.to_path_buf(),
            extension_type: None,
        };
        let profile = dependency_profile_for(project, None, NO_ENV);
        game_build_command_with(&spec, Path::new("/target"), BuildLoad::Foreground, &profile)
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn project_with_manifest(manifest: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("Cargo.toml"), manifest).expect("a manifest");
        dir
    }

    const PLAIN: &str = "[package]\nname = \"game\"\n[workspace]\n";

    /// An existing project's manifest stays as it is, and still gets a game
    /// that loads a large scene at a usable speed.
    #[test]
    fn a_plain_project_builds_with_optimised_dependencies() {
        let project = project_with_manifest(PLAIN);
        assert_eq!(
            dependency_profile_for(project.path(), None, NO_ENV),
            vec![GAME_DEPENDENCY_OPT_LEVEL, GAME_DEPENDENCY_DEBUG]
        );
        let args = build_args(project.path());
        for setting in [GAME_DEPENDENCY_OPT_LEVEL, GAME_DEPENDENCY_DEBUG] {
            let position = args.iter().position(|arg| arg == setting);
            let position = position.expect("the override is on the command line");
            assert_eq!(args[position - 1], "--config");
        }
        assert!(!args.iter().any(|arg| arg.contains("profile.dev.opt-level")));
    }

    #[test]
    fn a_project_that_picks_its_own_optimisation_keeps_it() {
        for profile in [
            "[profile.dev]\nopt-level = 1\n",
            "[profile.dev.package.\"*\"]\nopt-level = 3\n",
        ] {
            let project = project_with_manifest(&format!("{PLAIN}{profile}"));
            let args = build_args(project.path());
            assert!(!args.iter().any(|arg| arg == "--config"), "{profile}");
        }

        let project = project_with_manifest(PLAIN);
        std::fs::create_dir(project.path().join(".cargo")).expect("a folder");
        std::fs::write(
            project.path().join(".cargo/config.toml"),
            "[profile.dev.package.\"*\"]\nopt-level = 1\n",
        )
        .expect("a config");
        assert!(
            !build_args(project.path())
                .iter()
                .any(|arg| arg == "--config")
        );
    }

    /// Cargo reads config from every parent folder and its home, and from the
    /// environment, and a setting in any of them is the project's choice.
    #[test]
    fn a_dev_profile_in_any_config_cargo_reads_is_kept() {
        const CHOSEN: &str = "[profile.dev.package.\"*\"]\nopt-level = 1\n";
        let write_config = |folder: &Path| {
            std::fs::create_dir_all(folder).expect("a folder");
            std::fs::write(folder.join("config.toml"), CHOSEN).expect("a config");
        };

        let parent = tempfile::tempdir().expect("a temp dir");
        let project = parent.path().join("game");
        std::fs::create_dir(&project).expect("a project");
        std::fs::write(project.join("Cargo.toml"), PLAIN).expect("a manifest");
        assert_eq!(dependency_profile_for(&project, None, NO_ENV).len(), 2);
        assert!(
            dependency_profile_for(
                &project,
                None,
                DevProfileEnv {
                    opt_level: true,
                    ..NO_ENV
                }
            )
            .is_empty(),
            "the environment"
        );
        assert_eq!(
            dependency_profile_for(
                &project,
                None,
                DevProfileEnv {
                    debug: true,
                    ..NO_ENV
                }
            ),
            vec![GAME_DEPENDENCY_OPT_LEVEL],
            "the debug environment variable"
        );

        let home = tempfile::tempdir().expect("a temp dir");
        write_config(home.path());
        assert!(
            dependency_profile_for(&project, Some(home.path()), NO_ENV).is_empty(),
            "cargo's home"
        );

        write_config(&parent.path().join(".cargo"));
        assert!(
            dependency_profile_for(&project, None, NO_ENV).is_empty(),
            "a parent folder"
        );
    }

    #[test]
    fn a_project_that_sets_only_dependency_debug_info_keeps_it() {
        let project = project_with_manifest(&format!(
            "{PLAIN}[profile.dev.package.\"*\"]\ndebug = true\n"
        ));
        assert_eq!(
            dependency_profile_for(project.path(), None, NO_ENV),
            vec![GAME_DEPENDENCY_OPT_LEVEL]
        );
    }

    #[test]
    fn a_project_that_sets_the_dev_debug_level_keeps_it() {
        let project = project_with_manifest(&format!("{PLAIN}[profile.dev]\ndebug = 0\n"));
        assert_eq!(
            dependency_profile_for(project.path(), None, NO_ENV),
            vec![GAME_DEPENDENCY_OPT_LEVEL]
        );
    }

    #[test]
    fn a_workspace_members_profile_comes_from_the_workspace_root() {
        let root = project_with_manifest(
            "[workspace]\nmembers = [\"game\"]\n[profile.dev.package.\"*\"]\nopt-level = 3\n",
        );
        let member = root.path().join("game");
        std::fs::create_dir(&member).expect("a member");
        std::fs::write(member.join("Cargo.toml"), "[package]\nname = \"game\"\n")
            .expect("a manifest");
        assert!(!build_args(&member).iter().any(|arg| arg == "--config"));
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
    fn a_broken_timeout_override_cannot_reintroduce_a_hang() {
        assert_eq!(extractor_timeout_from(None), EXTRACTOR_TIMEOUT);
        assert_eq!(
            extractor_timeout_from(Some("300")),
            Duration::from_secs(300)
        );
        assert_eq!(
            extractor_timeout_from(Some(" 45 ")),
            Duration::from_secs(45)
        );
        assert_eq!(extractor_timeout_from(Some("0")), EXTRACTOR_TIMEOUT);
        assert_eq!(extractor_timeout_from(Some("forever")), EXTRACTOR_TIMEOUT);
        assert_eq!(extractor_timeout_from(Some("-1")), EXTRACTOR_TIMEOUT);
    }

    /// A game that never exits must not hold the editor's build open.
    #[cfg(unix)]
    #[test]
    fn an_extractor_that_never_exits_is_killed_at_the_deadline() {
        let child = Command::new("sleep")
            .arg("120")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn sleep");

        let started = Instant::now();
        let result = wait_with_deadline(child, Duration::from_millis(200));

        let message = result.expect_err("a hung extractor must not succeed");
        assert!(message.contains("did not finish"), "got {message}");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the deadline must fire, not the process"
        );
    }

    #[cfg(unix)]
    #[test]
    fn output_still_reaches_the_caller_when_the_extractor_exits_in_time() {
        let child = Command::new("echo")
            .arg("hello")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn echo");

        let output = wait_with_deadline(child, Duration::from_secs(30)).expect("echo finishes");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hello");
    }

    /// A build cancelled while cargo runs stops at once rather than finishing.
    #[test]
    fn a_cancelled_build_stops_without_finishing() {
        let root = std::env::temp_dir().join(format!("jackdaw-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).expect("fixture dirs");
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"cancelled-game\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [workspace]\n\n[dependencies]\n",
        )
        .expect("fixture manifest");
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("fixture source");
        let spec = ShimSpec {
            package_name: "cancelled-game".to_string(),
            crate_name: "cancelled_game".to_string(),
            project_root: root.clone(),
            extension_type: None,
        };
        let jackdaw_dir = root.join(".jackdaw");
        std::fs::create_dir_all(&jackdaw_dir).expect("jackdaw dir");

        let result = build_project_binary_cancellable(
            &spec,
            &jackdaw_dir,
            BuildLoad::Foreground,
            &AtomicBool::new(true),
            &mut |_| {},
        );
        let Err(ProjectBuildError::Io(error)) = result else {
            panic!("a cancelled build must not succeed");
        };
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert!(
            !game_target_dir(&jackdaw_dir)
                .join("debug/cancelled-game")
                .exists(),
            "cargo was stopped before it linked the game"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A project that does not compile fails with the compiler's own errors
    /// in it. The editor shows this text and nothing else, so a build that
    /// drops them leaves only "project failed to compile".
    #[test]
    fn a_project_that_does_not_compile_reports_the_compilers_own_errors() {
        let root = std::env::temp_dir().join(format!("jackdaw-broken-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).expect("fixture dirs");
        std::fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"broken-game\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [workspace]\n\n[dependencies]\n",
        )
        .expect("fixture manifest");
        // No dependencies, so this compiles (and fails) in about a second.
        std::fs::write(
            root.join("src/main.rs"),
            "fn main() { let _: u32 = \"not a number\"; }\n",
        )
        .expect("fixture source");

        let spec = ShimSpec {
            package_name: "broken-game".to_string(),
            crate_name: "broken_game".to_string(),
            project_root: root.clone(),
            extension_type: None,
        };
        let jackdaw_dir = root.join(".jackdaw");
        std::fs::create_dir_all(&jackdaw_dir).expect("jackdaw dir");

        let mut reported: Vec<String> = Vec::new();
        let result = build_project_binary(&spec, &jackdaw_dir, &mut |event| {
            if let BuildEvent::Log(line) = event {
                reported.push(line);
            }
        });
        let Err(ProjectBuildError::Compile { log }) = result else {
            panic!("a project that does not compile must fail as a compile error");
        };
        assert!(
            log.contains("E0308") && log.contains("mismatched types"),
            "the compiler's own diagnostic has to survive the trip:\n{log}"
        );
        assert!(
            !log.contains("no diagnostics"),
            "and it must not be reported as having said nothing:\n{log}"
        );
        // The panel shows what the reporter is handed, and `Display` for a
        // compile failure is one line, so cargo's own account has to arrive
        // here rather than only inside the error.
        let reported = reported.join("\n");
        assert!(
            reported.contains("E0308"),
            "the diagnostic reaches the build panel:\n{reported}"
        );
        assert!(
            reported.contains("could not compile"),
            "and so does cargo's own account of the failure:\n{reported}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_build_that_says_nothing_at_all_still_says_so() {
        assert_eq!(
            compile_failure_log(String::new(), "   \n"),
            "project failed to compile, and cargo reported no diagnostics"
        );
    }

    /// A failure before compilation starts is cargo's alone: nothing reaches
    /// the JSON stream, so the stderr is the whole account of it.
    #[test]
    fn cargos_own_failure_is_carried_when_no_diagnostic_is() {
        let log = compile_failure_log(
            String::new(),
            "error: failed to select a version for `serde`\n",
        );
        assert!(log.contains("failed to select a version"), "got {log}");
        assert!(!log.contains("no diagnostics"), "got {log}");
    }

    #[test]
    fn both_accounts_are_kept_when_both_have_something_to_say() {
        let log = compile_failure_log(
            "error[E0308]: mismatched types\n".to_string(),
            "error: could not compile `broken-game` due to 1 previous error",
        );
        assert!(log.contains("E0308"), "got {log}");
        assert!(log.contains("could not compile"), "got {log}");
    }

    #[test]
    fn an_old_toolchain_failure_names_the_fix() {
        let dir = temp_project_dir("old_toolchain");
        for stderr in [
            "error: rustc 1.96.0 is not supported by the following packages:\n",
            "error: rustc 1.96.0 is not supported by the following package:\n",
        ] {
            let log = with_toolchain_hint(String::new(), stderr, &dir);
            assert!(log.contains("rustup update stable"), "got {log}");
        }
        let other = with_toolchain_hint(String::new(), "error: could not compile `x`\n", &dir);
        assert!(other.is_empty(), "got {other}");

        std::fs::write(
            dir.join("rust-toolchain.toml"),
            "[toolchain]\nchannel = \"1.90.0\"\n",
        )
        .unwrap();
        let pinned = with_toolchain_hint(
            String::new(),
            "rustc 1.90.0 is not supported by the following packages",
            &dir,
        );
        assert!(pinned.contains("Raise the channel"), "got {pinned}");
        assert!(!pinned.contains("rustup update"), "got {pinned}");
    }

    fn temp_project_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("jackdaw_hint_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
