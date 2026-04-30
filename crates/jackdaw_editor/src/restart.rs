//! Respawn the editor binary from within a running instance.
//!
//! Used to close the "runtime-loaded game can't register systems"
//! gap: after a game is scaffolded + built + installed, we need the
//! user's next session to pick it up at startup via
//! `DylibLoaderPlugin::build`, which is the only place we have
//! `&mut App` to hand the game's plugin. A full process restart is
//! the simplest way to get there.
//!
//! # Why `exec` on Unix, `spawn + exit` on Windows
//!
//! On Unix we use [`std::os::unix::process::CommandExt::exec`] so
//! the new jackdaw takes over the current PID. This preserves:
//!
//! - The process-group membership (the terminal's foreground
//!   group), so a later Ctrl+C still reaches the new instance.
//! - `cargo run`'s wait loop (cargo is watching the original PID;
//!   exec keeps the same PID, so cargo sees the new process as the
//!   "still-running child" and doesn't prematurely drop back to
//!   the shell prompt).
//!
//! Windows has no `exec` equivalent; we fall back to spawning a
//! detached child and exiting. Terminal attachment there is
//! handled via the OS's process-management UI.
//!
//! The respawn inherits the full env (so `LD_LIBRARY_PATH` etc.
//! survive). Because jackdaw persists "recent projects" via
//! [`crate::project`], the new instance reopens the same project.

use std::path::PathBuf;
use std::process::Command;

use bevy::log::{info, warn};

/// Env var the parent process sets before respawning, signalling
/// to the child "the game you're about to load was just rebuilt
/// and installed; skip the initial-build step in the launcher and
/// go straight to the editor." Prevents the scaffold → build →
/// restart → auto-open → build → restart infinite loop.
pub const ENV_SKIP_INITIAL_BUILD: &str = "JACKDAW_SKIP_INITIAL_BUILD";

/// Respawn the editor binary with the same argv and env. On Unix
/// this uses `exec` to replace the current process image; on
/// Windows it spawns a detached child and exits. Never returns.
pub fn restart_jackdaw() -> ! {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            warn!("restart_jackdaw: current_exe failed ({e}); exiting without respawn");
            std::process::exit(1);
        }
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();

    info!(
        "Respawning jackdaw as {} with {} arg(s)",
        exe.display(),
        args.len()
    );

    unix_exec_or_fallback(&exe, &args);
    // Only reached on Windows / exec-unsupported platforms.
    windows_spawn_and_exit(&exe, &args)
}

#[cfg(unix)]
fn unix_exec_or_fallback(exe: &std::path::Path, args: &[std::ffi::OsString]) {
    use std::os::unix::process::CommandExt;

    let mut cmd = Command::new(exe);
    cmd.args(args).env(ENV_SKIP_INITIAL_BUILD, "1");
    // `exec` replaces the current process image. It only returns
    // on failure (e.g., ENOENT, EACCES). On success, control never
    // comes back here; the new image starts at `main`.
    let err = cmd.exec();
    warn!(
        "restart_jackdaw: exec failed ({err}); falling back to spawn-and-exit. \
         The child will be orphaned from the terminal."
    );
}

#[cfg(not(unix))]
fn unix_exec_or_fallback(_exe: &std::path::Path, _args: &[std::ffi::OsString]) {
    // No-op on non-Unix platforms; control falls through to
    // windows_spawn_and_exit below.
}

fn windows_spawn_and_exit(exe: &std::path::Path, args: &[std::ffi::OsString]) -> ! {
    let child = Command::new(exe)
        .args(args)
        .env(ENV_SKIP_INITIAL_BUILD, "1")
        .spawn();
    match child {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            warn!(
                "restart_jackdaw: failed to spawn {} ({e}); staying in current process",
                exe.display()
            );
            std::process::exit(1);
        }
    }
}

/// Attempt to verify we *can* restart (binary path is discoverable)
/// before committing to flushing state. Does not spawn anything.
pub fn can_restart() -> Option<PathBuf> {
    std::env::current_exe().ok()
}
