//! Bevy-free game runner and schema extractor implementation.

#![expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "the runner prints the extracted schema JSON to stdout as its output"
)]

use std::os::raw::c_char;

pub fn run() {
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

fn run_game(dylib: &str) {
    let lib = unsafe { libloading::Library::new(dylib) }
        .unwrap_or_else(|err| panic!("failed to load {dylib}: {err}"));
    let run: libloading::Symbol<extern "C" fn()> = unsafe { lib.get(b"jackdaw_run_game") }
        .unwrap_or_else(|err| panic!("no jackdaw_run_game in {dylib}: {err}"));
    run();
    std::mem::forget(lib);
}
