//! The game runner: a prebuilt binary that turns a project dylib into a
//! running game with zero play-time compilation, and the editor's
//! out-of-process schema extractor.
//!
//! Two modes:
//!
//! * `jackdaw-runner <project-dylib>` assembles the engine app,
//!   dlopens the dylib, and hands the app to the shim's
//!   `jackdaw_runner_entry` so the game's plugin installs itself.
//!   Assets resolve relative to the working directory the launcher
//!   set. This is Play.
//!
//! * `jackdaw-runner --extract-schema <project-dylib>` dlopens the
//!   dylib, drains its reflected types, prints the project schema (see
//!   [`jackdaw::project_build::schema`]) as JSON to stdout, and exits.
//!   The editor runs this per build so it learns the project's
//!   component types WITHOUT mapping project code into its own process
//!   (a loaded dylib can never be unmapped, so an in-editor load would
//!   leak on every refresh). This process's mapping dies when it exits.
//!
//! The dylib is never unloaded: unloading live Rust code is undefined
//! behavior, and the process exits to reclaim it.

use bevy::prelude::*;

fn main() {
    let mut args = std::env::args().skip(1);
    let first = args
        .next()
        .expect("usage: jackdaw-runner [--extract-schema] <project-dylib>");

    if first == "--extract-schema" {
        let dylib = args
            .next()
            .expect("usage: jackdaw-runner --extract-schema <project-dylib>");
        extract_schema(&dylib);
        return;
    }

    run_game(&first);
}

/// Dlopen the dylib, drain its reflected types, and print the project
/// schema as JSON. No engine, no window: just reflection.
fn extract_schema(dylib: &str) {
    let lib = unsafe { libloading::Library::new(dylib) }
        .unwrap_or_else(|err| panic!("failed to load {dylib}: {err}"));
    // dlopen ran the dylib's constructors, submitting its
    // `#[derive(Reflect)]` types into the shared auto-register
    // inventory. Draining picks them up alongside bevy's own; the
    // editor filters to the types it does not already know.
    let mut registry = bevy::reflect::TypeRegistry::default();
    registry.register_derived_types();
    std::mem::forget(lib);

    let schema = jackdaw::project_build::schema::extract_from_registry(&registry);
    match serde_json::to_string(&schema) {
        Ok(json) => println!("{json}"),
        Err(err) => {
            eprintln!("schema serialization failed: {err}");
            std::process::exit(1);
        }
    }
}

/// Assemble the engine, load the project's plugin, and run.
fn run_game(dylib: &str) {
    let mut app = App::new();
    app.add_plugins(jackdaw_runtime::maybe_windowless(DefaultPlugins));
    app.add_plugins(jackdaw_runtime::JackdawPlugin);
    // Spike-only camera so the frame stream has something to render;
    // real projects spawn their own cameras and this system goes away
    // once scene documents ride along.
    app.add_systems(Startup, spawn_probe_camera);

    let lib = unsafe { libloading::Library::new(dylib) }
        .unwrap_or_else(|err| panic!("failed to load {dylib}: {err}"));
    {
        let entry: libloading::Symbol<fn(&mut App)> =
            unsafe { lib.get(b"jackdaw_runner_entry") }
                .unwrap_or_else(|err| panic!("no jackdaw_runner_entry in {dylib}: {err}"));
        entry(&mut app);
    }
    std::mem::forget(lib);

    app.run();
}

fn spawn_probe_camera(mut commands: Commands) {
    commands.spawn(Camera3d::default());
}
