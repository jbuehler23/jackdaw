use std::{ffi::OsStr, process::Command};

pub mod editor_endpoint;
pub mod paths;

/// The rustup toolchain that built this crate. The SDK and extensions build with
/// it, because a Rust dylib only loads into a process built by the same compiler.
pub const RUSTUP_TOOLCHAIN: &str = env!("JACKDAW_TOOLCHAIN");

pub fn rust_env_command<S: AsRef<OsStr>>(command: S) -> std::process::Command {
    let mut command = Command::new(command);
    command.env("RUSTUP_TOOLCHAIN", RUSTUP_TOOLCHAIN);
    without_console_window(&mut command);
    command
}

/// Stops a console program started from a process with no console, such as
/// the editor's release build on Windows, from opening a console window of its
/// own. Its output still reaches whatever handles it is given. Leaves the
/// command unchanged elsewhere and when this process writes to a terminal.
pub fn without_console_window(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::io::IsTerminal;
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        if !std::io::stdout().is_terminal() && !std::io::stderr().is_terminal() {
            command.creation_flags(CREATE_NO_WINDOW);
        }
    }
    command
}
