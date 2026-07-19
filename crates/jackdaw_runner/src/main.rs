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
//!   [`jackdaw_project_build::schema`]) as JSON to stdout, and exits.
//!   The editor runs this per build so it learns the project's
//!   component types WITHOUT mapping project code into its own process
//!   (a loaded dylib can never be unmapped, so an in-editor load would
//!   leak on every refresh). This process's mapping dies when it exits.
//!
//! The dylib is never unloaded: unloading live Rust code is undefined
//! behavior, and the process exits to reclaim it.
//!
//! Kept out of the editor package so it links only the bevy-light build
//! pipeline plus the headless runtime, not the whole editor. That keeps
//! it a small, standalone artifact the SDK bootstrap can build on its
//! own.

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

/// Dlopen the dylib and print the schema its `jackdaw_extract_schema`
/// export produces. The reflection runs inside the dylib, which shares
/// the SDK's bevy, so this process links no bevy_reflect - it just calls
/// the export over FFI and prints the JSON. Keeping reflection out of this
/// binary is what lets it link on Windows, where a binary cannot resolve
/// the dylib's private reflect statics. The mapping dies on exit.
fn extract_schema(dylib: &str) {
    let lib = unsafe { libloading::Library::new(dylib) }
        .unwrap_or_else(|err| panic!("failed to load {dylib}: {err}"));
    let extract: libloading::Symbol<extern "C" fn() -> *mut std::os::raw::c_char> =
        unsafe { lib.get(b"jackdaw_extract_schema") }
            .unwrap_or_else(|err| panic!("no jackdaw_extract_schema in {dylib}: {err}"));
    let ptr = extract();
    if ptr.is_null() {
        eprintln!("schema extraction failed");
        std::process::exit(1);
    }
    // The dylib owns the string; this process exits immediately after
    // printing, so teardown reclaims it and no explicit free is needed.
    let json = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy();
    println!("{json}");
    std::mem::forget(lib);
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
        let entry: libloading::Symbol<fn(&mut App)> = unsafe { lib.get(b"jackdaw_runner_entry") }
            .unwrap_or_else(|err| panic!("no jackdaw_runner_entry in {dylib}: {err}"));
        entry(&mut app);
    }
    std::mem::forget(lib);

    app.run();
}

fn spawn_probe_camera(mut commands: Commands) {
    commands.spawn(Camera3d::default());
}
