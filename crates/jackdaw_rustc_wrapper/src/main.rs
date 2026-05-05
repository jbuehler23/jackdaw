//! Standalone `jackdaw-rustc-wrapper` binary built from this crate.
//!
//! Used by dev contributors via
//! `cargo build -p jackdaw_rustc_wrapper`. The top-level `jackdaw`
//! package also produces a `jackdaw-rustc-wrapper` binary (so
//! `cargo install jackdaw` installs the wrapper alongside the
//! editor); both binaries call into the same [`jackdaw_rustc_wrapper::run`]
//! entry point.

use std::process::ExitCode;

fn main() -> ExitCode {
    jackdaw_rustc_wrapper::run()
}
