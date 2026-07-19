//! The game runner: a bevy-free thin launcher.
//!
//! It `dlopen`s a project dylib and drives it entirely over FFI. Both Play
//! and schema extraction run *inside* the dylib (which shares the SDK's
//! bevy through `libjackdaw_sdk`), so this binary links no bevy at all.
//! That is what lets it link on every platform, including Windows, where a
//! secondary binary cannot resolve a dylib's private bevy statics.
//!
//! Two modes:
//!
//! * `jackdaw-runner <project-dylib>` - Play: call the dylib's
//!   `jackdaw_run_game`, which builds and runs the engine app.
//! * `jackdaw-runner --extract-schema <project-dylib>` - call the dylib's
//!   `jackdaw_extract_schema`, print the schema JSON, and exit.
//!
//! The dylib is never unloaded: unloading live Rust code is undefined
//! behavior, and the process exits to reclaim it.

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the runner prints the extracted schema JSON to stdout as its output"
)]

use std::os::raw::c_char;

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
/// export produces. The reflection runs inside the dylib; this process
/// exits immediately after printing, so teardown reclaims the string.
fn extract_schema(dylib: &str) {
    let lib = unsafe { libloading::Library::new(dylib) }
        .unwrap_or_else(|err| panic!("failed to load {dylib}: {err}"));
    let extract: libloading::Symbol<extern "C" fn() -> *mut c_char> =
        unsafe { lib.get(b"jackdaw_extract_schema") }
            .unwrap_or_else(|err| panic!("no jackdaw_extract_schema in {dylib}: {err}"));
    let ptr = extract();
    if ptr.is_null() {
        eprintln!("schema extraction failed");
        std::process::exit(1);
    }
    let json = unsafe { std::ffi::CStr::from_ptr(ptr) }.to_string_lossy();
    println!("{json}");
    std::mem::forget(lib);
}

/// Dlopen the dylib and call its `jackdaw_run_game` export, which builds
/// and runs the engine app inside the dylib. Blocks until the app exits.
fn run_game(dylib: &str) {
    let lib = unsafe { libloading::Library::new(dylib) }
        .unwrap_or_else(|err| panic!("failed to load {dylib}: {err}"));
    let run: libloading::Symbol<extern "C" fn()> = unsafe { lib.get(b"jackdaw_run_game") }
        .unwrap_or_else(|err| panic!("no jackdaw_run_game in {dylib}: {err}"));
    run();
    std::mem::forget(lib);
}
