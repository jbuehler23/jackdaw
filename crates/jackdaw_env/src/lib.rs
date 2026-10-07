use std::{ffi::OsStr, process::Command};

pub mod editor_endpoint;
pub mod paths;

/// The rustup toolchain that built this crate. The SDK and extensions build with
/// it, because a Rust dylib only loads into a process built by the same compiler.
pub const RUSTUP_TOOLCHAIN: &str = env!("JACKDAW_TOOLCHAIN");

pub fn rust_env_command<S: AsRef<OsStr>>(command: S) -> std::process::Command {
    let mut command = Command::new(command);
    command.env("RUSTUP_TOOLCHAIN", RUSTUP_TOOLCHAIN);
    command
}
