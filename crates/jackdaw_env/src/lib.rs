use std::{ffi::OsStr, process::Command};

pub mod paths;

pub const RUSTUP_TOOLCHAIN: &str = "nightly-2026-03-05";

pub fn rust_env_command<S: AsRef<OsStr>>(command: S) -> std::process::Command {
    let mut command = Command::new(command);
    command.env("RUSTUP_TOOLCHAIN", RUSTUP_TOOLCHAIN);
    command
}
