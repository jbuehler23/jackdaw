#![expect(clippy::print_stderr, reason = "e2e test prints skip diagnostics")]
//! End-to-end scaffolding regression: scaffold a game project, build its
//! dylib against the SDK, and assert the template's component reaches the
//! extracted schema. This is the user's first-run loop (New Game -> build
//! -> component shows up in the editor) exercised headlessly, so it runs
//! on every platform in the onboarding CI matrix.
//!
//! It needs a built dev SDK (dylib + rustc wrapper + runner under
//! `target/<triple>/`). Where that is absent - an ordinary `cargo test`
//! without the SDK build - it skips rather than fails, so it is a no-op
//! locally and a real check in CI after the "Build the SDK" step.

use std::path::{Path, PathBuf};

use jackdaw::scaffold::{TemplateKind, scaffold_new_project};
use jackdaw_project_build::sdk_paths::SdkPaths;
use jackdaw_project_build::{BuildEvent, build_project_dylib, shim_spec_for_project};

#[test]
fn scaffold_game_builds_and_exposes_component() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Resolve the dev SDK the same way the CLI does. No SDK -> skip: this
    // test only means anything once the SDK, wrapper, and runner exist.
    let Some(sdk) = SdkPaths::for_workspace_detect(&workspace) else {
        eprintln!("scaffold_e2e: no dev SDK under target/; skipping");
        return;
    };
    if !sdk.wrapper_exists() || !sdk.runner.is_file() {
        eprintln!("scaffold_e2e: SDK wrapper/runner missing; skipping");
        return;
    }

    let work = unique_dir("jackdaw_scaffold_e2e");
    let _ = std::fs::remove_dir_all(&work);
    let dest = work.join("mygame");

    scaffold_new_project(&dest, "mygame", TemplateKind::Game).expect("scaffold a game project");

    let spec = shim_spec_for_project(&dest, None).expect("scaffolded project is a jackdaw project");
    let jackdaw_dir = dest.join(".jackdaw");
    let mut ignore_progress = |_: BuildEvent| {};
    let build = build_project_dylib(
        &spec,
        &jackdaw_dir,
        &sdk,
        Some(&workspace),
        &mut ignore_progress,
    )
    .expect("build the scaffolded project dylib");

    let schema = build.schema.expect("schema extracted from the built dylib");
    let has_spinning_cube = schema
        .components
        .iter()
        .any(|c| c.type_path.contains("SpinningCube"));
    assert!(
        has_spinning_cube,
        "template component `SpinningCube` missing from schema; got: {:?}",
        schema
            .components
            .iter()
            .map(|c| c.type_path.as_str())
            .collect::<Vec<_>>()
    );

    let _ = std::fs::remove_dir_all(&work);
}

/// A process-unique scratch dir under the system temp dir. `Instant`/PID
/// keep parallel test binaries from colliding without needing a temp-dir
/// crate.
fn unique_dir(prefix: &str) -> PathBuf {
    let mut base = std::env::temp_dir();
    base.push(format!("{prefix}_{}", std::process::id()));
    ensure_parent(&base);
    base
}

fn ensure_parent(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
}
