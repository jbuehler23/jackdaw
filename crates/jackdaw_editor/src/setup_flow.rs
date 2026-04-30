//! First-time setup flow.
//!
//! When the launcher detects a missing `.warmed` sentinel for the
//! current jackdaw version, it runs this flow to populate the shared
//! `CARGO_TARGET_DIR` cache with `bevy` + `jackdaw_editor` + transitive
//! deps. After setup completes, every user project's first build is
//! incremental (just the user's gameplay code, ~30s) instead of cold
//! (~10 minutes).
//!
//! See the launcher architecture spec at
//! `docs/superpowers/specs/2026-04-30-jackdaw-launcher-architecture-design.md`.

use std::path::Path;
use std::process::Command;

use crate::cache_manager;

#[derive(Debug)]
pub struct SetupOutcome {
    pub success: bool,
    pub log_tail: String,
}

/// Run the first-time setup flow synchronously. Caller should invoke
/// this in a worker thread and surface progress via the existing
/// `BuildProgress` mechanism (see `ext_build`).
///
/// On success, writes the `.warmed` sentinel so subsequent runs skip
/// setup. On failure, leaves the cache partially populated; cargo's
/// incremental machinery resumes on retry.
pub fn run_setup() -> std::io::Result<SetupOutcome> {
    let scaffold_dir = cache_manager::setup_scaffold_dir();
    if !scaffold_dir.exists() {
        std::fs::create_dir_all(&scaffold_dir)?;
        scaffold_warmup_project(&scaffold_dir)?;
    } else {
        // Re-write the project files in case they got out of sync
        // (e.g. user manually edited them, or a partial setup left
        // stale content). Cheap; cargo fingerprinting handles the
        // unchanged-source case.
        scaffold_warmup_project(&scaffold_dir)?;
    }

    let target_dir = cache_manager::target_dir();
    let mut cmd = Command::new("cargo");
    cmd.current_dir(&scaffold_dir)
        .args(["build"])
        .env("CARGO_TARGET_DIR", &target_dir)
        // Stream cargo's "Compiling X (N/M)" output directly to the
        // launcher's stderr so the user sees real-time progress
        // through the ~10 minute warm-up rather than a single
        // post-mortem dump on completion.
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit());

    let status = cmd.status()?;
    let success = status.success();

    if success {
        cache_manager::mark_warmed()?;
    }

    // With inherited stdio we don't capture output, so the log_tail
    // is empty. The user already saw cargo's output stream live; no
    // need to repeat. The field is kept on `SetupOutcome` for future
    // UI work that may want to capture + display in a dialog.
    Ok(SetupOutcome { success, log_tail: String::new() })
}

/// Detect whether setup is needed. Returns `true` if `.warmed` is
/// absent for the current jackdaw version.
pub fn needs_setup() -> bool {
    !cache_manager::is_warmed()
}

/// Write the warm-up project's files. Depends on the same crates the
/// user's projects depend on (`bevy` + `jackdaw_editor` + `jackdaw_runtime`),
/// so cargo's content-addressable build store populates the shared
/// cache with everything every user project subsequently needs.
///
/// When the editor is running from a source checkout (detected via
/// `crate::new_project::jackdaw_dev_checkout`), the manifest also
/// gets a `[patch.crates-io]` section pointing at the local working
/// tree. Otherwise the `jackdaw_editor` / `jackdaw_runtime` version
/// references would fail to resolve since those crates aren't yet
/// published to crates.io.
fn scaffold_warmup_project(dir: &Path) -> std::io::Result<()> {
    let mut cargo_toml = format!(
        r#"[package]
name = "jackdaw_setup_warmup"
version = "0.1.0"
edition = "2024"
publish = false

[lib]
crate-type = ["cdylib", "rlib"]
path = "src/lib.rs"

[dependencies]
bevy = "0.18"
jackdaw_editor = "{version}"
jackdaw_runtime = "{version}"
"#,
        version = env!("CARGO_PKG_VERSION"),
    );

    if let Some(checkout) = crate::new_project::jackdaw_dev_checkout() {
        let checkout_str = checkout.display();
        cargo_toml.push_str(&format!(
            "\n# Auto-injected by jackdaw setup flow running from a source\n\
             # checkout. Routes the warm-up build at the local working tree\n\
             # so the shared cache populates with the same artifacts user\n\
             # projects will reference. Released editor binaries skip this.\n\
             [patch.crates-io]\n\
             jackdaw_editor = {{ path = \"{checkout_str}/crates/jackdaw_editor\" }}\n\
             jackdaw_api = {{ path = \"{checkout_str}/crates/jackdaw_api\" }}\n\
             jackdaw_api_internal = {{ path = \"{checkout_str}/crates/jackdaw_api_internal\" }}\n\
             jackdaw_runtime = {{ path = \"{checkout_str}/crates/jackdaw_runtime\" }}\n",
        ));
    }

    std::fs::write(dir.join("Cargo.toml"), cargo_toml)?;

    let src_dir = dir.join("src");
    std::fs::create_dir_all(&src_dir)?;
    std::fs::write(
        src_dir.join("lib.rs"),
        "// Empty placeholder; the only point of this crate is to make\n\
         // cargo populate the shared target dir with bevy + jackdaw_editor\n\
         // compilations.\n",
    )?;

    Ok(())
}

