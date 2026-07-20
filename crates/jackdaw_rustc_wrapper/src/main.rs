//! Standalone `jackdaw-rustc-wrapper` binary; a shim around [`jackdaw_rustc_wrapper::run`].

use std::process::ExitCode;

fn main() -> ExitCode {
    jackdaw_rustc_wrapper::run()
}
