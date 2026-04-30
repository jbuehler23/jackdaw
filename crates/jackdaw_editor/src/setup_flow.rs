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
        .args(["build", "--message-format=json-render-diagnostics"])
        .env("CARGO_TARGET_DIR", &target_dir);

    let output = cmd.output()?;
    let success = output.status.success();
    let stderr_text = String::from_utf8_lossy(&output.stderr);
    let log_tail = last_n_lines(&stderr_text, 20);

    if success {
        cache_manager::mark_warmed()?;
    }

    Ok(SetupOutcome { success, log_tail })
}

/// Detect whether setup is needed. Returns `true` if `.warmed` is
/// absent for the current jackdaw version.
pub fn needs_setup() -> bool {
    !cache_manager::is_warmed()
}

fn last_n_lines(s: &str, n: usize) -> String {
    let mut lines: Vec<&str> = s.lines().collect();
    if lines.len() > n {
        lines = lines.split_off(lines.len() - n);
    }
    lines.join("\n")
}

/// Write the warm-up project's files. Depends on the same crates the
/// user's projects depend on (`bevy` + `jackdaw_editor` + `jackdaw_runtime`),
/// so cargo's content-addressable build store populates the shared
/// cache with everything every user project subsequently needs.
fn scaffold_warmup_project(dir: &Path) -> std::io::Result<()> {
    let cargo_toml = format!(
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_n_lines_short_input() {
        assert_eq!(last_n_lines("a\nb\nc", 5), "a\nb\nc");
    }

    #[test]
    fn last_n_lines_truncates() {
        assert_eq!(last_n_lines("a\nb\nc\nd\ne\nf", 3), "d\ne\nf");
    }

    #[test]
    fn last_n_lines_empty_input() {
        assert_eq!(last_n_lines("", 5), "");
    }
}
