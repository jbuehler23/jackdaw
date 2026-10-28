use std::{ffi::OsStr, process::Command};

pub mod editor_endpoint;
pub mod extensions_config;
pub mod paths;
pub mod process;

/// Environment variable naming the project the editor should open,
/// set by `jd open <path>` on the process it spawns.
pub const ENV_OPEN_PROJECT: &str = "JACKDAW_OPEN_PROJECT";

/// The rustup toolchain that built this crate. The SDK and extensions build with
/// it, because a Rust dylib only loads into a process built by the same compiler.
pub const RUSTUP_TOOLCHAIN: &str = env!("JACKDAW_TOOLCHAIN");

/// The target triple this crate was compiled for. The editor runs natively,
/// so this is its host, known without running a toolchain that may be missing.
pub const HOST_TRIPLE: &str = env!("JACKDAW_HOST_TRIPLE");

/// A command run by [`RUSTUP_TOOLCHAIN`], the toolchain that built this editor.
///
/// Only for building the SDK and extensions and for reading what they built,
/// which must use this exact compiler. Anything that compiles, resolves or
/// runs the user's own project takes [`user_toolchain_command`]. Call
/// [`ensure_sdk_toolchain`] before a build, so a missing toolchain is
/// installed in the open instead of failing or downloading unannounced.
pub fn sdk_toolchain_command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut command = Command::new(program);
    command.env("RUSTUP_TOOLCHAIN", RUSTUP_TOOLCHAIN);
    without_console_window(&mut command);
    command
}

/// A command run by the user's own toolchain: the project's
/// `rust-toolchain.toml` when `current_dir` has one, otherwise the rustup
/// default.
///
/// For the user's project and game, and for rustup itself. An inherited
/// `RUSTUP_TOOLCHAIN`, which rustup sets on an editor started through cargo,
/// is removed so it cannot stand in for the project's choice.
pub fn user_toolchain_command<S: AsRef<OsStr>>(program: S) -> Command {
    let mut command = Command::new(program);
    command.env_remove("RUSTUP_TOOLCHAIN");
    without_console_window(&mut command);
    command
}

/// Whether rustup has [`RUSTUP_TOOLCHAIN`] installed, or `None` when rustup
/// cannot be run.
///
/// Asks rustup for its list rather than running the toolchain, which rustup
/// would otherwise download on the spot.
pub fn sdk_toolchain_installed() -> Option<bool> {
    let output = user_toolchain_command("rustup")
        .args(["toolchain", "list"])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    Some(lists_toolchain(
        &String::from_utf8_lossy(&output.stdout),
        RUSTUP_TOOLCHAIN,
        HOST_TRIPLE,
    ))
}

/// Whether `rustup toolchain list` output names `toolchain` for `host`.
fn lists_toolchain(list: &str, toolchain: &str, host: &str) -> bool {
    let qualified = format!("{toolchain}-{host}");
    list.lines()
        .filter_map(|line| line.split_whitespace().next())
        .any(|name| name == toolchain || name == qualified)
}

/// Install [`RUSTUP_TOOLCHAIN`] through rustup unless it is installed already.
///
/// Without rustup there is nothing to install, and this succeeds: the rustc
/// on `PATH` builds, and a mismatch surfaces as that build's own error.
pub fn ensure_sdk_toolchain() -> Result<(), String> {
    if sdk_toolchain_installed() != Some(false) {
        return Ok(());
    }
    process::run_captured(user_toolchain_command("rustup").args([
        "toolchain",
        "install",
        RUSTUP_TOOLCHAIN,
        "--profile",
        "minimal",
    ]))
    .map(drop)
    .map_err(|error| format!("could not install the {RUSTUP_TOOLCHAIN} toolchain: {error}"))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_user_toolchain_command_drops_the_editors_toolchain() {
        let command = user_toolchain_command("cargo");
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "RUSTUP_TOOLCHAIN" && value.is_none()),
            "the inherited toolchain must be removed, not only left unset"
        );
    }

    #[test]
    fn an_sdk_toolchain_command_uses_the_editors_toolchain() {
        let command = sdk_toolchain_command("cargo");
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "RUSTUP_TOOLCHAIN"
                    && value == Some(OsStr::new(RUSTUP_TOOLCHAIN)))
        );
    }

    #[test]
    fn an_installed_toolchain_is_found_by_its_full_or_short_name() {
        let host = "x86_64-unknown-linux-gnu";
        let list = "stable-x86_64-unknown-linux-gnu (default)\n1.99.0-x86_64-unknown-linux-gnu\n";
        assert!(lists_toolchain(list, "1.99.0", host));
        assert!(lists_toolchain(list, "stable", host));
        assert!(!lists_toolchain(list, "1.99", host));
        assert!(!lists_toolchain(list, "1.98.0", host));
        assert!(lists_toolchain("my-local\n", "my-local", host));
    }
}
