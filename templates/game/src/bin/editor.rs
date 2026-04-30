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

    // The launcher spawns this binary with `cwd` set to the project
    // root, so `current_dir()` resolves to the project we should open.
    // `AutoOpenProjectPlugin` reads it on Startup, inserts `ProjectRoot`,
    // and transitions `AppState::ProjectSelect → Editor` so the binary
    // skips the project picker entirely.
    let project_root = std::env::current_dir().expect("CWD must be readable");

    // The project's cdylib lives in the shared jackdaw cache (the
    // launcher routes builds there via `CARGO_TARGET_DIR`). Hand the
    // explicit file path to `DylibLoaderPlugin` so it dlopens the
    // game plugin and registers it against the editor's `GameSubApp`.
    // We disable the default loader (which scans the per-user config
    // dirs for installed games) so the per-project editor only loads
    // its own project's plugin.
    let project_cdylib =
        jackdaw_editor::editor_resolver::cdylib_path(&project_root)
            .expect("project cdylib path resolves");

    // `DefaultPlugins` must precede `EditorPlugins` because
    // `EditorCorePlugin` calls `app.init_state::<AppState>()`, which
    // needs the `StateTransition` schedule that `StatesPlugin` (part
    // of `DefaultPlugins`) installs.
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(
            jackdaw_editor::EditorPlugins::default()
                .build()
                .disable::<jackdaw_editor::DylibLoaderPlugin>(),
        )
        .add_plugins(
            jackdaw_editor::DylibLoaderPlugin::default()
                .with_user_extension_dir(false)
                .with_extension_env_var(false)
                .with_extension_search_path(project_cdylib),
        )
        .add_plugins(jackdaw_editor::auto_open::AutoOpenProjectPlugin {
            root: project_root,
        })
        .run();
    ExitCode::SUCCESS
}
