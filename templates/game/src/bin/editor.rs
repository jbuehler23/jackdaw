//! Per-project editor binary. The launcher orchestrates `cargo build`
//! of this target before spawning it. The editor dlopens this
//! project's cdylib at startup and runs the user's plugin in-process.
//!
//! In `--headless` mode the binary skips the GUI startup and routes
//! the named operator through `jackdaw_editor::run_headless_operator`.
//! That path is the editor side of the launcher's
//! `dispatch_editor_op`.

use std::process::ExitCode;

use bevy::prelude::*;

fn main() -> ExitCode {
    let mut argv = std::env::args().skip(1);
    if let Some(arg) = argv.next() {
        if arg == "--headless" {
            let Some(op_id) = argv.next() else {
                eprintln!("error: --headless requires an op-id");
                return ExitCode::FAILURE;
            };
            let json = argv.next().unwrap_or_default();
            return jackdaw_editor::run_headless_operator(&op_id, &json);
        }
    }

    // `DefaultPlugins` must precede `EditorPlugins` because
    // `EditorCorePlugin` calls `app.init_state::<AppState>()`, which
    // needs the `StateTransition` schedule that `StatesPlugin` (part
    // of `DefaultPlugins`) installs.
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(jackdaw_editor::EditorPlugins::default())
        .run();
    ExitCode::SUCCESS
}
