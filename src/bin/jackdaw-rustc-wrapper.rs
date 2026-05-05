//! Wrapper binary shipped by the top-level `jackdaw` package so
//! `cargo install jackdaw` produces both `jackdaw` and
//! `jackdaw-rustc-wrapper` in the user's cargo bin directory.
//!
//! All logic lives in [`jackdaw_rustc_wrapper::run`]; this binary is
//! a one-line shim. The standalone `jackdaw_rustc_wrapper` crate
//! produces an identical binary for the dev workflow
//! (`cargo build -p jackdaw_rustc_wrapper`).

use std::process::ExitCode;

fn main() -> ExitCode {
    jackdaw_rustc_wrapper::run()
}
