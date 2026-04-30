//! Jackdaw launcher binary entry point.
//!
//! No args opens the launcher GUI (project picker, scaffold flow).
//! `<op-id> <json>` runs headless op dispatch:
//!   - Launcher-scope ops run inline (project.new, etc.).
//!   - Editor-scope ops delegate to the per-project editor binary.

use std::process::ExitCode;

#[expect(
    clippy::print_stderr,
    reason = "CLI mode is a stderr-driven shell tool"
)]
fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().collect();
    match jackdaw_editor::operator_routing::parse_argv(&argv) {
        Ok(jackdaw_editor::operator_routing::Mode::Gui) => {
            run_gui();
            ExitCode::SUCCESS
        }
        Ok(jackdaw_editor::operator_routing::Mode::LauncherOp { op_id, json }) => {
            jackdaw_editor::operator_routing::dispatch_launcher_op(&op_id, &json)
        }
        Ok(jackdaw_editor::operator_routing::Mode::EditorOp {
            op_id,
            json,
            project,
        }) => jackdaw_editor::operator_routing::dispatch_editor_op(&op_id, &json, &project),
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE
        }
    }
}

#[expect(
    clippy::print_stderr,
    reason = "first-time setup needs CLI output before the GUI can start"
)]
fn run_gui() {
    // First-time setup: if the shared cache for this jackdaw version
    // hasn't been warmed, run the setup compile inline before starting
    // the GUI. This blocks the launcher startup for ~10 minutes on a
    // fresh machine; subsequent launches skip this step.
    //
    // A more polished UI (progress dialog while compile runs) is a
    // follow-up; for v1 we just print to stderr.
    if jackdaw_editor::setup_flow::needs_setup() {
        eprintln!(
            "First-time setup: compiling shared dependencies for jackdaw {}.",
            env!("CARGO_PKG_VERSION")
        );
        eprintln!("This happens once per jackdaw version. ~5-15 minutes.");
        match jackdaw_editor::setup_flow::run_setup() {
            Ok(outcome) if outcome.success => {
                eprintln!("Setup complete.");
            }
            Ok(outcome) => {
                eprintln!("Setup failed:\n{}", outcome.log_tail);
                std::process::exit(1);
            }
            Err(e) => {
                eprintln!("Setup error: {e}");
                std::process::exit(1);
            }
        }
    }

    // The launcher's GUI is the project-select screen.
    // Per Phase 5 architecture, the launcher's main job is project
    // picker plus build orchestration. Once a project is opened, the
    // launcher spawns the per-project editor binary and exits.
    //
    // For Phase 5 the launcher still uses `EditorPlugins::default()`
    // because we haven't yet defined a separate `LauncherPlugins`
    // group. That's OK; the launcher's startup is in
    // `AppState::ProjectSelect`, and once `transition_to_editor`
    // runs (now spawn-and-exit), the launcher exits before
    // `AppState::Editor` would do anything.
    bevy::prelude::App::new()
        .add_plugins(jackdaw_editor::EditorPlugins::default())
        .run();
}
