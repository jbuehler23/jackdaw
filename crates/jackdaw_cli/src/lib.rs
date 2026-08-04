//! Headless command implementations used by the public `jd` executable.
//!
//! This remains an internal library so the public executable and release
//! tooling share the bevy-light
//! [`jackdaw_project_build`] pipeline, so it stays small.
//!
//! Commands:
//!
//! ```text
//! build [--project <path>]   Build the project so a running editor
//!                            picks up new or changed components.
//! run   [--project <path>]   Build, then launch the game standalone.
//! setup                      Build the SDK into the cache (one-time).
//! doctor                     Report SDK build prerequisites.
//! ```

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "jd is a user-facing terminal tool; stdout/stderr is its output channel"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use jackdaw_project_build::bootstrap::{self, SetupProgress};
use jackdaw_project_build::{BuildEvent, build_project_binary, sdk_paths};
use jackdaw_schema::{read_schema, schema_path};

pub mod package;

/// Dispatch the bevy-light subset of a `jd` invocation.
pub fn run_args(args: &[String]) -> ExitCode {
    match args.first().map(String::as_str) {
        Some("build") => cmd_build(&args[1..]),
        Some("run") => cmd_run(&args[1..]),
        Some("setup") => cmd_setup(),
        Some("doctor") => cmd_doctor(&args[1..]),
        Some("--version" | "-V" | "version") => {
            println!(
                "jd {} (targets bevy {})",
                jackdaw_project_build::VERSION,
                jackdaw_project_build::BEVY_VERSION
            );
            ExitCode::SUCCESS
        }
        // Help and unknown commands belong to the `jd` executable,
        // which owns the real command list. A second usage string here
        // listed neither `new` nor `import`, and the fallthrough made it
        // the one users saw after a typo.
        other => {
            eprintln!(
                "jd: `{}` is not one of this library's commands; run `jd --help`",
                other.unwrap_or_default()
            );
            ExitCode::FAILURE
        }
    }
}

/// `jd setup`: build the SDK into `~/.jackdaw/sdk/...` so builds
/// work without a jackdaw source checkout. Requires a binary built with
/// the `embed-recipe` feature (packaged releases).
fn cmd_setup() -> ExitCode {
    // Print phase headers; cargo's own progress reaches the terminal on
    // inherited stderr, so per-crate events are left for the editor's bar.
    let report = |event: SetupProgress| {
        if let SetupProgress::Phase(phase) = event {
            println!("jd setup: {phase}");
        }
    };
    match bootstrap::ensure_sdk(report) {
        Ok(cache) => {
            println!("jd setup: SDK ready at {}", cache.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("jd setup: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Report which of the three install paths supplied the SDK, and
/// whether it is usable. Returns false only for a genuinely broken one.
fn report_sdk() -> bool {
    let sdk = resolve_sdk(dev_workspace().as_deref());
    let problems = sdk.problems();
    if problems.is_empty() && (sdk.manifest.is_file() || bootstrap::needs_setup()) {
        println!(
            "  [ ok ] SDK: {} ({})",
            sdk.origin.describe(),
            sdk.dylib.display()
        );
    } else if problems.is_empty() {
        println!("  [fail] SDK: not found at {}", sdk.manifest.display());
        println!("         fix: jd setup");
        return false;
    } else {
        println!(
            "  [fail] SDK: {} at {} is not usable",
            sdk.origin.describe(),
            sdk.dylib.display()
        );
        for problem in problems {
            println!("         {problem}");
        }
        println!("         fix: {}", jackdaw_project_build::sdk_remedy(&sdk));
        return false;
    }
    true
}

/// `jd doctor [--project <path>]`: report the tools an SDK build needs,
/// so a prerequisite gap is visible before a long compile rather than
/// after, followed by the state of a project when one is in scope. The
/// single "why is my project not working" command.
fn cmd_doctor(args: &[String]) -> ExitCode {
    let mut ok = true;

    println!("environment");
    for check in &bootstrap::check_prerequisites() {
        if check.ok {
            println!("  [ ok ] {}: {}", check.name, check.detail);
        } else {
            ok = false;
            println!("  [fail] {}: {}", check.name, check.detail);
            if let Some(fix) = &check.fix {
                println!("         fix: {fix}");
            }
        }
    }

    // The SDK belongs in the environment section, not the project one:
    // it is a property of the install, it is the same for every project,
    // and `jd doctor` with no project is exactly how the book tells
    // people to verify an install. Reporting it only when a project
    // happened to be in scope let a bare run say "all checks passed"
    // with no usable SDK at all.
    ok &= report_sdk();

    // A project is in scope when asked for explicitly, or when the
    // working directory is one. Running `jd doctor` anywhere else stays
    // a pure environment report.
    let requested = parse_project_arg(args);
    let explicit = requested.is_some();
    let root = requested
        .or_else(|| std::env::current_dir().ok())
        .and_then(|dir| dunce::canonicalize(dir).ok())
        .filter(|dir| explicit || dir.join("Cargo.toml").is_file());
    match root {
        Some(root) => {
            println!("\nproject: {}", root.display());
            ok &= report_project(&root);
        }
        // Say so, rather than leaving "all checks passed" to imply the
        // project was inspected and found healthy.
        None => println!(
            "\nproject: none here. Run this inside a project, or pass --project <path>, to \
             check one."
        ),
    }

    if ok {
        println!("\njd doctor: all checks passed");
        ExitCode::SUCCESS
    } else {
        eprintln!("\njd doctor: some checks failed (see above)");
        ExitCode::FAILURE
    }
}

/// Report whether a project is set up, what jackdaw resolved from it,
/// and whether its extracted type schema is current. Returns whether
/// everything that would block a build is in place.
fn report_project(root: &Path) -> bool {
    use jackdaw_project_build::project_manifest::{self, PinStatus};

    let mut ok = true;
    let pass = |detail: String| println!("  [ ok ] {detail}");

    if !root.join("Cargo.toml").is_file() {
        println!("  [fail] no Cargo.toml here");
        println!("         fix: run `jd new <name>` to create a project");
        return false;
    }

    let manifest = match project_manifest::ProjectManifest::read_checked(root) {
        Ok(manifest) if !project_manifest::ProjectManifest::exists(root) => {
            ok = false;
            println!("  [fail] jackdaw.toml: missing, this project is not set up yet");
            println!("         fix: jd import --apply {}", root.display());
            manifest
        }
        Ok(manifest) => {
            pass("jackdaw.toml: present".to_string());
            manifest
        }
        Err(detail) => {
            // A file that does not parse reads downstream as "no
            // overrides set", so without this the user gets a healthy
            // report while their plugin and package keys are ignored.
            ok = false;
            println!("  [fail] jackdaw.toml: does not parse: {detail}");
            println!("         fix: correct the file, or delete it and run `jd import --apply`");
            project_manifest::ProjectManifest::default()
        }
    };

    let package = jackdaw_project_build::cargo_meta::resolve_project_package(
        root,
        manifest.package.as_deref(),
    );
    match &package {
        Ok(package) => {
            pass(format!(
                "package: {} (crate `{}`)",
                package.name, package.crate_name
            ));
            if package.has_lib {
                pass("library target: present".to_string());
            } else {
                ok = false;
                println!("  [fail] library target: missing");
                println!(
                    "         fix: the editor only sees components in a library; \
                     run `jd import --apply` to add one"
                );
            }
            let extension = jackdaw_project_build::detect::detect_extension(&package.dir);
            // Check any configured plugin against what the crate really
            // declares. Echoing the key back would certify a name that
            // only fails later in doctor re-runs or setup notes.
            let found = jackdaw_project_build::detect::plugin_paths(&package.dir);
            let configured = manifest.plugin.as_deref();
            // A candidate not reachable from the crate root is not a
            // usable plugin name, so requiring a `crate_path` here keeps
            // doctor from blessing a key that cannot be referenced.
            let resolved = configured.and_then(|name| {
                found.iter().find(|candidate| {
                    candidate.crate_path.is_some()
                        && (candidate.type_name == name
                            || candidate.crate_path.as_deref() == Some(name))
                })
            });
            let detected =
                jackdaw_project_build::detect::detect_plugin(&package.dir, &package.crate_name)
                    .and_then(|path| path.split_once("::").map(|(_, name)| name.to_string()));
            match (&extension, configured, &resolved) {
                (Some((_, name)), ..) => pass(format!("editor extension: {name}")),
                (None, Some(name), None) => {
                    ok = false;
                    let unreachable = found.iter().any(|candidate| {
                        candidate.type_name == name && candidate.crate_path.is_none()
                    });
                    if unreachable {
                        println!(
                            "  [fail] game plugin: `{name}` is declared but not reachable from \
                             outside the crate"
                        );
                        println!(
                            "         fix: make its module `pub mod`, or re-export it from \
                             lib.rs with `pub use`"
                        );
                    } else {
                        println!(
                            "  [fail] game plugin: jackdaw.toml names `{name}`, which {} does \
                             not declare",
                            package.crate_name
                        );
                        let usable: Vec<&str> = found
                            .iter()
                            .filter(|candidate| candidate.crate_path.is_some())
                            .map(|candidate| candidate.type_name.as_str())
                            .collect();
                        if usable.is_empty() {
                            println!(
                                "         fix: this crate declares no reachable \
                                 `impl Plugin for ...`; add one, or remove the `plugin` key"
                            );
                        } else {
                            println!(
                                "         fix: set `plugin` to one of {}, or remove the key",
                                usable.join(", ")
                            );
                        }
                    }
                }
                (None, Some(_), Some(candidate)) => {
                    pass(format!("game plugin: {}", candidate.type_name));
                }
                (None, None, _) => match detected {
                    Some(name) => pass(format!("game plugin: {name}")),
                    None => println!(
                        "  [warn] game plugin: none declared; the editor still reads this \
                         crate's components. Play launches your cargo binary, so add a Bevy \
                         Plugin in the library and wire it from `main.rs` when you want \
                         gameplay under Play"
                    ),
                },
            }
        }
        Err(error) => {
            ok = false;
            println!("  [fail] package: {error}");
        }
    }

    match project_manifest::compare_project(root, &manifest) {
        PinStatus::Match => pass(format!(
            "versions: jackdaw {} / bevy {}",
            jackdaw_project_build::VERSION,
            jackdaw_project_build::BEVY_VERSION
        )),
        PinStatus::Unpinned => println!(
            "  [warn] versions: this project records none; it will be pinned on the next setup"
        ),
        PinStatus::JackdawDiffers { pinned, running } => println!(
            "  [warn] versions: set up with jackdaw {pinned}, running {running} (same bevy, so \
             builds still work)"
        ),
        PinStatus::BevyDiffers { pinned, running } => {
            ok = false;
            println!("  [fail] versions: set up for bevy {pinned}, this jackdaw targets {running}");
            println!(
                "         fix: install the jackdaw release for bevy {pinned}, or migrate the \
                 project to bevy {running}"
            );
        }
    }

    let jackdaw_dir = root.join(".jackdaw");
    match read_schema(&jackdaw_dir) {
        Some(schema) => pass(format!(
            "project types: {} components known ({})",
            schema.components.len(),
            schema_path(&jackdaw_dir).display()
        )),
        None => println!(
            "  [warn] project types: not built yet; run `jd build` so the editor sees your \
             components"
        ),
    }

    // A project whose requirements cannot resolve never reaches a
    // compiler, so every other check above can pass while nothing
    // builds. This is the one check that exercises the manifest a
    // scaffold or import actually produced.
    match resolve_dependencies(root) {
        Ok(()) => pass("dependencies: resolve".to_string()),
        Err(detail) => {
            ok = false;
            println!("  [fail] dependencies: {detail}");
            println!(
                "         fix: check the versions in Cargo.toml; `cargo update` refreshes the \
                 index if a release is newer than your cached copy"
            );
        }
    }

    ok
}

/// Ask cargo to resolve the project's full dependency graph, reporting
/// its first error line. Unlike the `--no-deps` metadata call used to
/// find the package, this one does the version solving, which is where
/// an unpublished or mistyped requirement surfaces.
fn resolve_dependencies(root: &Path) -> Result<(), String> {
    let output = Command::new("cargo")
        .current_dir(root)
        .args(["metadata", "--format-version", "1"])
        .output()
        .map_err(|error| format!("could not run cargo: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("error"))
        .unwrap_or("cargo could not resolve this project's dependencies")
        .to_string())
}

/// `jd build [--project <path>]`: run the same pipeline the
/// editor runs and persist `.jackdaw/schema.json`, which a running editor
/// watches and reloads. Defaults to the current directory.
fn cmd_build(args: &[String]) -> ExitCode {
    let root = match resolve_root(args) {
        Ok(root) => root,
        Err(code) => return code,
    };
    build_project(&root)
}

/// `jd run [--project <path>]`: build the game (refreshing the schema a
/// running editor reloads), then launch it. A build failure aborts
/// before running, since the game would fail to compile anyway.
fn cmd_run(args: &[String]) -> ExitCode {
    let root = match resolve_root(args) {
        Ok(root) => root,
        Err(code) => return code,
    };
    let built = build_project(&root);
    if !matches!(built, ExitCode::SUCCESS) {
        return built;
    }
    println!("jackdaw run: launching {}", root.display());
    match Command::new("cargo").arg("run").current_dir(&root).status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1)),
        Err(err) => {
            eprintln!("jackdaw run: failed to launch the game: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Resolve and canonicalize the target project directory from
/// `--project`/`-p` or a bare path, defaulting to the current directory.
fn resolve_root(args: &[String]) -> Result<PathBuf, ExitCode> {
    let root =
        parse_project_arg(args).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    dunce::canonicalize(&root).map_err(|err| {
        eprintln!("jd: cannot resolve project path {}: {err}", root.display());
        ExitCode::FAILURE
    })
}

/// Build the game for `root` and persist its schema.
fn build_project(root: &Path) -> ExitCode {
    let spec = match jackdaw_project_build::shim_spec_for_project(root) {
        Ok(spec) => spec,
        Err(error) => {
            eprintln!("jackdaw build: {error}");
            return ExitCode::FAILURE;
        }
    };

    let jackdaw_dir = root.join(".jackdaw");

    println!("jackdaw build: building {}", root.display());
    // Rendered diagnostics stream through the reporter; cargo's own
    // "Compiling" status reaches the terminal on inherited stderr.
    let mut report = |event: BuildEvent| {
        if let BuildEvent::Log(line) = event {
            eprintln!("{line}");
        }
    };
    match build_project_binary(&spec, &jackdaw_dir, &mut report) {
        Ok(build) => {
            let components = build
                .schema
                .as_ref()
                .map(|s| s.components.len())
                .unwrap_or(0);
            println!(
                "jackdaw build: ok ({components} components); binary at {}, schema at {}",
                build.binary.display(),
                schema_path(&jackdaw_dir).display()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("jackdaw build: failed: {err}");
            ExitCode::FAILURE
        }
    }
}

/// The jackdaw workspace this CLI was built from, two levels up from
/// the crate. Present in a dev checkout (even after `cargo install`,
/// the baked path still points at the real source tree); absent on a
/// machine that only has a packaged install.
fn dev_workspace() -> Option<PathBuf> {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .filter(|p| p.exists())
}

/// Resolve the SDK the build pipeline needs.
///
/// The ordinary resolution runs first, so `jd doctor` reports the SDK
/// the editor would actually use. Taking the workspace scan first
/// instead made the two disagree: the scan has no view of whether a
/// checkout's SDK is current, so `doctor` would report one SDK as fine
/// while the editor built against another.
///
/// The scan remains as a fallback, for an installed `jd` on `PATH`
/// whose own directory holds nothing: the exe-relative default cannot
/// find a checkout it does not live in.
fn resolve_sdk(dev_workspace: Option<&Path>) -> sdk_paths::SdkPaths {
    let computed = sdk_paths::SdkPaths::compute();
    if computed.dylib_exists() {
        return computed;
    }
    if let Some(workspace) = dev_workspace
        && let Some(sdk) = sdk_paths::SdkPaths::for_workspace_detect(workspace)
    {
        return sdk;
    }
    computed
}

fn parse_project_arg(args: &[String]) -> Option<PathBuf> {
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--project" | "-p" => return it.next().map(PathBuf::from),
            other if !other.starts_with('-') => return Some(PathBuf::from(other)),
            _ => {}
        }
    }
    None
}
