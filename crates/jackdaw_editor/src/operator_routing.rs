//! CLI dispatch for `jackdaw <op-id> <json>` invocations. Launcher-
//! scope ops run inline (e.g. project.new); editor-scope ops are
//! delegated to the per-project editor binary as a subprocess.

use std::path::PathBuf;
use std::process::ExitCode;

use crate::editor_resolver;

/// Operator IDs the launcher handles inline (no per-project editor
/// needed). All other op-ids are delegated to the per-project editor.
pub const LAUNCHER_OPERATORS: &[&str] = &[
    "project.new",
    "project.build",
    "project.open",
    "setup.warm_cache",
    "setup.clean_cache",
];

#[derive(Debug)]
pub enum Mode {
    /// No args. Open the launcher GUI.
    Gui,
    /// Launcher-scope op. Run inline against a minimal Bevy App.
    LauncherOp { op_id: String, json: String },
    /// Editor-scope op. Delegate to the per-project editor binary.
    EditorOp {
        op_id: String,
        json: String,
        project: PathBuf,
    },
}

/// Parse argv into a [`Mode`].
///
/// `argv[0]` is the binary name (ignored). `argv[1]` may be an op-id
/// or a CLI flag. Subsequent args may be JSON params or `--project=...`.
pub fn parse_argv(argv: &[String]) -> Result<Mode, String> {
    if argv.len() <= 1 {
        return Ok(Mode::Gui);
    }

    let op_id = argv[1].clone();
    let mut json = String::new();
    let mut project: Option<PathBuf> = None;

    let mut i = 2;
    while i < argv.len() {
        let arg = &argv[i];
        if let Some(rest) = arg.strip_prefix("--project=") {
            project = Some(PathBuf::from(rest));
            i += 1;
        } else if !arg.starts_with("--") && json.is_empty() {
            json = arg.clone();
            i += 1;
        } else {
            return Err(format!("unrecognized arg: {arg}"));
        }
    }

    if LAUNCHER_OPERATORS.contains(&op_id.as_str()) {
        Ok(Mode::LauncherOp { op_id, json })
    } else {
        let project = project
            .or_else(|| std::env::current_dir().ok())
            .ok_or_else(|| {
                format!(
                    "operator '{op_id}' is editor-scope and needs a project; \
                     cd to one or pass --project=<path>"
                )
            })?;
        Ok(Mode::EditorOp {
            op_id,
            json,
            project,
        })
    }
}

/// Run a launcher-scope op inline. The dispatch path is identical to
/// the per-project editor's `--headless` mode: build a minimal Bevy
/// `App` with the editor's operator catalog, parse JSON params, and
/// invoke the named operator. The launcher / editor split is purely a
/// routing decision (inline vs subprocess); the dispatch itself is
/// shared.
pub fn dispatch_launcher_op(op_id: &str, json: &str) -> ExitCode {
    crate::run_headless_operator(op_id, json)
}

/// Delegate an editor-scope op to the project's per-project editor
/// binary as a subprocess. Triggers a build first if the binary is
/// missing or stale.
#[expect(
    clippy::print_stderr,
    reason = "CLI mode is a stderr-driven shell tool"
)]
pub fn dispatch_editor_op(op_id: &str, json: &str, project: &std::path::Path) -> ExitCode {
    let Some(editor_bin) = editor_resolver::editor_binary_path(project) else {
        eprintln!(
            "error: could not resolve editor binary for {}",
            project.display()
        );
        return ExitCode::FAILURE;
    };
    if !editor_bin.exists() || !editor_resolver::editor_binary_is_current(project) {
        eprintln!("editor binary missing or stale; building...");
        if let Err(e) = crate::ext_build::build_editor_for_project(project) {
            eprintln!("editor build failed: {e}");
            return ExitCode::FAILURE;
        }
    }
    let status = std::process::Command::new(&editor_bin)
        .args(["--headless", op_id, json])
        .current_dir(project)
        .status();
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => ExitCode::from(s.code().unwrap_or(1) as u8),
        Err(e) => {
            eprintln!("failed to spawn editor binary: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_args_is_gui() {
        let argv = vec!["jackdaw".to_string()];
        assert!(matches!(parse_argv(&argv).unwrap(), Mode::Gui));
    }

    #[test]
    fn launcher_op_is_inline() {
        let argv = vec!["jackdaw".into(), "project.new".into(), "{}".into()];
        assert!(matches!(
            parse_argv(&argv).unwrap(),
            Mode::LauncherOp { .. }
        ));
    }

    #[test]
    fn editor_op_with_explicit_project() {
        let argv = vec![
            "jackdaw".into(),
            "scene.import_gltf".into(),
            "{}".into(),
            "--project=/tmp/foo".into(),
        ];
        let mode = parse_argv(&argv).unwrap();
        if let Mode::EditorOp { op_id, project, .. } = mode {
            assert_eq!(op_id, "scene.import_gltf");
            assert_eq!(project, PathBuf::from("/tmp/foo"));
        } else {
            panic!("expected EditorOp, got {mode:?}");
        }
    }

    #[test]
    fn launcher_op_does_not_need_project() {
        // Even without --project, project.new (a launcher-scope op)
        // should parse to LauncherOp without errors.
        let argv = vec!["jackdaw".into(), "project.new".into(), "{}".into()];
        assert!(matches!(
            parse_argv(&argv).unwrap(),
            Mode::LauncherOp { .. }
        ));
    }
}
