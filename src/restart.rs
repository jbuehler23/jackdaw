//! Respawn the editor binary from within a running instance.
//!
//! Used to close the "runtime-loaded game can't register systems"
//! gap: after a game is scaffolded + built + installed, we need the
//! user's next session to pick it up at startup via
//! `DylibLoaderPlugin::build`, which is the only place we have
//! `&mut App` to hand the game's plugin. A full process restart is
//! the simplest way to get there.
//!
//! Also how the editor opens a project other than the one it
//! launched for. `AssetPlugin` fixes the asset root when the app is
//! built, so a second project can only be opened by a second
//! process: [`relaunch_into_project`] names it in the environment
//! the new process reads at startup.
//!
//! # What a reopen does not carry
//!
//! Only the project path crosses. The launcher's override cards
//! ("Open without setup", "Open anyway", "Not now") come back up in
//! the new process and want the same click again; carrying an answer
//! across would mean naming it in the environment beside the project.
//!
//! # A reopen never waits for shutdown
//!
//! [`relaunch_into_project`] is called in place of the app's exit, not
//! after it. Shutting down first can destroy the window while the
//! render thread is inside a swapchain present, and the process dies
//! there with the reopen never honored. Handing the process over
//! directly ends every thread at once, so nothing is left to race.
//!
//! # Why `exec` on Unix, `spawn + exit` on Windows
//!
//! On Unix we use `std::os::unix::process::CommandExt::exec` so
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

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use bevy::log::{info, warn};
use bevy::prelude::Resource;

/// Env var the parent process sets before respawning, signalling
/// to the child "the game you're about to load was just rebuilt
/// and installed; skip the initial-build step in the launcher and
/// go straight to the editor." Prevents the scaffold -> build ->
/// restart -> auto-open -> build -> restart infinite loop.
pub const ENV_SKIP_INITIAL_BUILD: &str = "JACKDAW_SKIP_INITIAL_BUILD";

/// The project directory this process's asset root was built from.
/// Bevy resolves every relative asset load under `<0>/assets`, and
/// `AssetPlugin` fixes that path when the app is built, so nothing
/// running afterwards can move it.
#[derive(Resource, Clone, Debug)]
pub struct AssetProjectRoot(pub PathBuf);

/// The project this process should reopen once its event loop ends.
/// A global rather than a resource because the world is gone by the
/// time `main` acts on it.
static PENDING_PROJECT: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Whether `target` can be opened here, or needs a process of its
/// own. Only the project the asset root was built from can be opened
/// in place; everything else would load its textures, models and
/// scenes out of `asset_root`.
pub fn opens_in_process(asset_root: &Path, target: &Path) -> bool {
    resolve(asset_root) == resolve(target)
}

/// What opening a project amounts to for this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenPlan {
    /// The asset root already points at it: open it here.
    Here,
    /// Hand it to a process rooted at it.
    Reopen,
    /// Neither is possible: the binary to hand it to cannot be
    /// found, so opening here would silently read the wrong assets.
    Unreachable,
}

/// Decide what opening `target` means, given the project this
/// process's asset root was built from and the binary a reopen would
/// run (`can_restart`'s answer).
pub fn plan_open(asset_root: &Path, target: &Path, exe: Option<PathBuf>) -> OpenPlan {
    if opens_in_process(asset_root, target) {
        return OpenPlan::Here;
    }
    if exe.is_none() {
        return OpenPlan::Unreachable;
    }
    OpenPlan::Reopen
}

fn resolve(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| dunce::simplified(path).to_path_buf())
}

/// Record that `project` should be reopened once this process's event
/// loop ends; [`take_project_relaunch`] hands the request to `main`.
///
/// The launcher's open funnel calls [`relaunch_into_project`] outright
/// instead, since a request that waits for the app to shut down can be
/// lost to a crash on the way out. This path is for callers that must
/// let the app end normally.
pub fn request_project_relaunch(project: PathBuf) {
    if let Ok(mut pending) = PENDING_PROJECT.lock() {
        *pending = Some(project);
    }
}

/// Take the pending reopen request, if any. Called once, after the
/// app's event loop returns.
pub fn take_project_relaunch() -> Option<PathBuf> {
    PENDING_PROJECT.lock().ok().and_then(|mut p| p.take())
}

/// The argv and env the reopened editor is handed, built without
/// spawning anything.
pub fn relaunch_command(exe: &Path, args: &[OsString], project: &Path) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env(crate::project::ENV_OPEN_PROJECT, project)
        // Inherited from an earlier respawn, this would tell the new
        // process to skip a build the target may need.
        .env_remove(ENV_SKIP_INITIAL_BUILD)
        // One-shot switches, both spent by this process. Inheriting
        // them would replay a scripted run or take a screenshot
        // against the new project.
        .env_remove(crate::boot_ops::ENV_RUN_OP)
        .env_remove(crate::screenshot::ENV_SHOT);
    cmd
}

/// Replace this process with one whose asset root is `project`.
/// Never returns.
pub fn relaunch_into_project(project: &Path) -> ! {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            warn!("relaunch_into_project: current_exe failed ({e}); exiting without respawn");
            std::process::exit(1);
        }
    };
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    info!("Reopening jackdaw at {}", project.display());
    exec_or_spawn(relaunch_command(&exe, &args, project), &exe)
}

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

    let mut cmd = Command::new(&exe);
    cmd.args(&args).env(ENV_SKIP_INITIAL_BUILD, "1");
    exec_or_spawn(cmd, &exe)
}

/// Hand the process over to `cmd`: `exec` on Unix so the new image
/// keeps the PID (and with it the terminal's foreground group and
/// cargo's wait loop), spawn-and-exit everywhere else.
fn exec_or_spawn(mut cmd: Command, exe: &Path) -> ! {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // `exec` returns only on failure (ENOENT, EACCES); on success the new image starts
        // at `main`.
        let err = cmd.exec();
        warn!(
            "exec failed ({err}); falling back to spawn-and-exit. \
             The child will be orphaned from the terminal."
        );
    }
    match cmd.spawn() {
        Ok(_) => std::process::exit(0),
        Err(e) => {
            warn!(
                "failed to spawn {} ({e}); staying in current process",
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_project_the_asset_root_was_built_from_opens_in_place() {
        let dir = tempfile::tempdir().expect("temp dir");
        let a = dir.path().join("alpha");
        let b = dir.path().join("beta");
        std::fs::create_dir_all(&a).expect("alpha");
        std::fs::create_dir_all(&b).expect("beta");

        assert!(opens_in_process(&a, &a));
        assert!(opens_in_process(&a, &a.join("..").join("alpha")));
        assert!(!opens_in_process(&a, &b));
    }

    /// Paths that do not exist still compare, so a missing recent entry cannot be mistaken
    /// for the open project.
    #[test]
    fn absent_directories_still_compare() {
        assert!(opens_in_process(
            Path::new("/nowhere/alpha"),
            Path::new("/nowhere/alpha")
        ));
        assert!(!opens_in_process(
            Path::new("/nowhere/alpha"),
            Path::new("/nowhere/beta")
        ));
    }

    #[test]
    fn a_project_with_nowhere_to_open_is_neither_opened_nor_reopened() {
        let here = Path::new("/projects/alpha");
        let elsewhere = Path::new("/projects/beta");
        let exe = || Some(PathBuf::from("/usr/bin/jackdaw"));

        assert_eq!(plan_open(here, here, exe()), OpenPlan::Here);
        assert_eq!(plan_open(here, elsewhere, exe()), OpenPlan::Reopen);
        assert_eq!(plan_open(here, elsewhere, None), OpenPlan::Unreachable);
        // The binary is irrelevant when the project opens in place.
        assert_eq!(plan_open(here, here, None), OpenPlan::Here);
    }

    /// The new process is told which project to root itself at, without the build shortcut
    /// or the one-shot switches an earlier run may have left in the environment.
    #[test]
    fn the_reopened_editor_is_named_the_project_and_no_spent_switches() {
        let cmd = relaunch_command(
            Path::new("/usr/bin/jackdaw"),
            &[OsString::from("--flag")],
            Path::new("/projects/beta"),
        );

        assert_eq!(cmd.get_program(), OsString::from("/usr/bin/jackdaw"));
        assert_eq!(
            cmd.get_args().collect::<Vec<_>>(),
            vec![OsString::from("--flag").as_os_str()]
        );

        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.contains(&(
            OsString::from(crate::project::ENV_OPEN_PROJECT).as_os_str(),
            Some(OsString::from("/projects/beta").as_os_str())
        )));
        for spent in [
            ENV_SKIP_INITIAL_BUILD,
            crate::boot_ops::ENV_RUN_OP,
            crate::screenshot::ENV_SHOT,
        ] {
            assert!(
                envs.contains(&(OsString::from(spent).as_os_str(), None)),
                "{spent} must not survive into the new process"
            );
        }
    }
}
