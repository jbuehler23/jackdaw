//! `jackdaw-cli`: terminal commands for Jackdaw projects, shipped as a
//! separate binary from the editor GUI so they never clash on `PATH`.
//! It links only the bevy-light [`jackdaw_project_build`] pipeline, so it
//! stays small and installs cleanly on its own.
//!
//! Commands:
//!   build [--project <path>]   Build the project so a running editor
//!                              picks up new or changed components.
//!   run   [--project <path>]   Build, then launch the game standalone.
//!
//! `new` / `init` / `migrate` / `doctor` will migrate here from the
//! editor binary next.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use jackdaw_project_build::{ProjectBuildError, build_project_dylib, schema, sdk_paths};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("build") => cmd_build(&args[2..]),
        Some("run") => cmd_run(&args[2..]),
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
         run   [--project <path>]   Build, then launch the game standalone"
    );
}

/// `jackdaw-cli build [--project <path>]`: run the same pipeline the
/// editor runs and persist `.jackdaw/schema.json`, which a running editor
/// watches and reloads. Defaults to the current directory.
fn cmd_build(args: &[String]) -> ExitCode {
    let root = match resolve_root(args) {
        Ok(root) => root,
        Err(code) => return code,
    };
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
    match build_project_dylib(&spec, &jackdaw_dir, &sdk, dev_workspace.as_deref()) {
        Ok(build) => {
            let components = build.schema.as_ref().map(|s| s.components.len()).unwrap_or(0);
            println!(
                "jackdaw build: ok ({} redirect edges, {components} components); schema at {}",
                build.edges,
                schema::schema_path(&jackdaw_dir).display()
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("jackdaw build: failed: {err}");
            if let ProjectBuildError::Compile { log } = &err {
                eprintln!("{log}");
            }
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
