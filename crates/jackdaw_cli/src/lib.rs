//! `jackdaw-cli`: terminal commands for Jackdaw projects.
//!
//! Shipped as its own `jackdaw-cli` binary and staged beside the editor in a
//! downloaded release bundle. The logic links only the bevy-light
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
//! package-sdk                Stage a relocatable SDK.
//! bundle --out <dir>         Stage editor, tools, dylibs and SDK.
//! ```

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "jackdaw-cli is a user-facing terminal tool; stdout/stderr is its output channel"
)]

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use jackdaw_project_build::bootstrap::{self, SetupProgress};
use jackdaw_project_build::{BuildEvent, build_project_dylib, schema, sdk_paths};

mod package;

/// Dispatch a `jackdaw-cli` invocation. Reads `std::env::args`, so the
/// two thin `main`s (the standalone binary and the one bundled in the
/// editor package) are one line each.
pub fn run() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("build") => cmd_build(&args[2..]),
        Some("run") => cmd_run(&args[2..]),
        Some("setup") => cmd_setup(),
        Some("doctor") => cmd_doctor(),
        Some("package-sdk") => package::cmd_package_sdk(&args[2..]),
        Some("bundle") => package::cmd_bundle(&args[2..]),
        Some("--version" | "-V" | "version") => {
            println!(
                "jackdaw-cli {} (targets bevy {})",
                jackdaw_project_build::VERSION,
                jackdaw_project_build::BEVY_VERSION
            );
            ExitCode::SUCCESS
        }
        Some("--help" | "-h" | "help") => {
            usage();
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("jackdaw-cli: unknown command '{other}'");
            usage();
            ExitCode::FAILURE
        }
        None => {
            usage();
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "usage: jackdaw-cli <command>\n\n\
         commands:\n  \
         build [--project <path>]   Build the project so the editor picks up new or changed components\n  \
         run   [--project <path>]   Build, then launch the game standalone\n  \
         setup                      Build the SDK into the cache (one-time; needs rustup)\n  \
         doctor                     Report whether the SDK build prerequisites are satisfied\n  \
         package-sdk --out <dir>    Stage a relocatable SDK install layout from a release build (release tooling)\n  \
         bundle --out <dir>         Stage the full downloadable bundle: editor + tools + dylibs + SDK (release tooling)"
    );
}

/// `jackdaw-cli setup`: build the SDK into `~/.jackdaw/sdk/...` so builds
/// work without a jackdaw source checkout. Requires a binary built with
/// the `embed-recipe` feature (packaged releases).
fn cmd_setup() -> ExitCode {
    // Print phase headers; cargo's own progress reaches the terminal on
    // inherited stderr, so per-crate events are left for the editor's bar.
    let report = |event: SetupProgress| {
        if let SetupProgress::Phase(phase) = event {
            println!("jackdaw setup: {phase}");
        }
    };
    match bootstrap::ensure_sdk(report) {
        Ok(cache) => {
            println!("jackdaw setup: SDK ready at {}", cache.display());
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("jackdaw setup: {err}");
            ExitCode::FAILURE
        }
    }
}

/// `jackdaw-cli doctor`: report the tools an SDK build needs, so a
/// prerequisite gap is visible before a long compile rather than after.
fn cmd_doctor() -> ExitCode {
    let checks = bootstrap::check_prerequisites();
    let mut all_ok = true;
    for check in &checks {
        if check.ok {
            println!("[ ok ] {}: {}", check.name, check.detail);
        } else {
            all_ok = false;
            println!("[fail] {}: {}", check.name, check.detail);
            if let Some(fix) = &check.fix {
                println!("       fix: {fix}");
            }
        }
    }
    if all_ok {
        println!("jackdaw doctor: prerequisites satisfied");
        ExitCode::SUCCESS
    } else {
        eprintln!("jackdaw doctor: some prerequisites are missing (see above)");
        ExitCode::FAILURE
    }
}

/// `jackdaw-cli build [--project <path>]`: run the same pipeline the
/// editor runs and persist `.jackdaw/schema.json`, which a running editor
/// watches and reloads. Defaults to the current directory.
fn cmd_build(args: &[String]) -> ExitCode {
    let root = match resolve_root(args) {
        Ok(root) => root,
        Err(code) => return code,
    };
    if let Err(code) = ensure_sdk_ready() {
        return code;
    }
    build_project(&root)
}

/// `jackdaw-cli run [--project <path>]`: build the editor-compatible
/// dylib (refreshing the schema a running editor reloads), then launch
/// the game standalone through the project's own binary. A build failure
/// aborts before running, since the game would fail to compile anyway.
fn cmd_run(args: &[String]) -> ExitCode {
    let root = match resolve_root(args) {
        Ok(root) => root,
        Err(code) => return code,
    };
    if let Err(code) = ensure_sdk_ready() {
        return code;
    }
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

/// Build the SDK into the cache first when this binary carries a recipe
/// and no usable cache exists yet, so `build`/`run` work on a fresh
/// install without a separate `setup`. A no-op in a dev checkout (no
/// embedded recipe) and once the cache is warm.
fn ensure_sdk_ready() -> Result<(), ExitCode> {
    if !bootstrap::needs_setup() {
        return Ok(());
    }
    let report = |event: SetupProgress| {
        if let SetupProgress::Phase(phase) = event {
            println!("jackdaw: {phase}");
        }
    };
    bootstrap::ensure_sdk(report).map(|_| ()).map_err(|err| {
        eprintln!("jackdaw: SDK setup failed: {err}");
        ExitCode::FAILURE
    })
}

/// Resolve and canonicalize the target project directory from
/// `--project`/`-p` or a bare path, defaulting to the current directory.
fn resolve_root(args: &[String]) -> Result<PathBuf, ExitCode> {
    let root =
        parse_project_arg(args).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    root.canonicalize().map_err(|err| {
        eprintln!(
            "jackdaw-cli: cannot resolve project path {}: {err}",
            root.display()
        );
        ExitCode::FAILURE
    })
}

/// Build the project dylib for `root` and persist its schema.
fn build_project(root: &Path) -> ExitCode {
    let Some(spec) = jackdaw_project_build::shim_spec_for_project(root, None) else {
        eprintln!(
            "jackdaw build: {} is not a jackdaw project (no lib crate in Cargo metadata)",
            root.display()
        );
        return ExitCode::FAILURE;
    };

    let jackdaw_dir = root.join(".jackdaw");
    // The jackdaw workspace this CLI was built from, two levels up from
    // the crate. Present in a dev checkout (even after `cargo install`,
    // the baked path still points at the real source tree); absent on a
    // machine that only has a packaged install.
    let dev_workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .filter(|p| p.exists());
    let sdk = resolve_sdk(dev_workspace.as_deref());

    println!("jackdaw build: building {}", root.display());
    // Rendered diagnostics stream through the reporter; cargo's own
    // "Compiling" status reaches the terminal on inherited stderr.
    let mut report = |event: BuildEvent| {
        if let BuildEvent::Log(line) = event {
            eprintln!("{line}");
        }
    };
    match build_project_dylib(
        &spec,
        &jackdaw_dir,
        &sdk,
        dev_workspace.as_deref(),
        &mut report,
    ) {
        Ok(build) => {
            let components = build
                .schema
                .as_ref()
                .map(|s| s.components.len())
                .unwrap_or(0);
            println!(
                "jackdaw build: ok ({} redirect edges, {components} components); schema at {}",
                build.edges,
                schema::schema_path(&jackdaw_dir).display()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("jackdaw build: failed: {err}");
            ExitCode::FAILURE
        }
    }
}

/// Resolve the SDK the build pipeline needs. `JACKDAW_SDK_DIR` (a
/// packaged install) wins; otherwise, in a dev checkout, find the SDK
/// under the jackdaw workspace's `target/` (release or debug) so an
/// installed `jackdaw-cli` uses the SDK the editor was built with rather
/// than looking next to its own binary; last resort is the exe-relative
/// default, which then surfaces a clear "SDK missing" error.
fn resolve_sdk(dev_workspace: Option<&Path>) -> sdk_paths::SdkPaths {
    if std::env::var_os("JACKDAW_SDK_DIR").is_some() {
        return sdk_paths::SdkPaths::compute();
    }
    if let Some(workspace) = dev_workspace
        && let Some(sdk) = sdk_paths::SdkPaths::for_workspace_detect(workspace)
    {
        return sdk;
    }
    sdk_paths::SdkPaths::compute()
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
