//! Standalone subprocess play mode.
//!
//! When `PlayMode::Standalone`, clicking Play shells out
//! `cargo run` against the project root. The child process opens
//! its own OS window for the game; the editor stays as the editor.
//! Stop sends `SIGTERM` (Unix) / `Child::kill` (Windows).
//!
//! Subprocess stdout/stderr are piped into the editor's tracing
//! subscriber (each line `tracing::info!("[game] {line}")` for
//! stdout, `tracing::warn!("[game] {line}")` for stderr). A future
//! Log Window panel can capture these via a `tracing-subscriber` layer.

use bevy::prelude::*;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// Tracks the in-flight `cargo run` subprocess for `PlayMode::Standalone`.
///
/// `child` is shared via `Arc<Mutex<_>>` so the editor can both
/// poll/kill it and read its exit status without contending with
/// the stdout/stderr pump threads (which only borrow the piped
/// stream readers, not the `Child` itself).
#[derive(Resource, Default)]
pub struct StandalonePlayState {
    pub child: Option<Arc<Mutex<Child>>>,
    pub stdout_pump: Option<JoinHandle<()>>,
    pub stderr_pump: Option<JoinHandle<()>>,
}

pub fn start_standalone_play(world: &mut World) {
    let project_root = match world.get_resource::<crate::project::ProjectRoot>() {
        Some(r) => r.root.clone(),
        None => {
            warn!("standalone_play: no project open");
            return;
        }
    };

    // Refuse to spawn a second child while one is already running.
    if let Some(state) = world.get_resource::<StandalonePlayState>()
        && state.child.is_some()
    {
        warn!("standalone_play: a game subprocess is already running");
        return;
    }

    let mut cmd = Command::new("cargo");
    cmd.current_dir(&project_root)
        .arg("run")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            warn!("standalone_play: failed to spawn cargo: {e}");
            return;
        }
    };

    let stdout = child.stdout.take().expect("stdout was piped");
    let stderr = child.stderr.take().expect("stderr was piped");

    let stdout_pump = std::thread::spawn(move || {
        let buffered = BufReader::new(stdout);
        for line in buffered.lines().map_while(Result::ok) {
            info!("[game] {line}");
        }
    });
    let stderr_pump = std::thread::spawn(move || {
        let buffered = BufReader::new(stderr);
        for line in buffered.lines().map_while(Result::ok) {
            warn!("[game] {line}");
        }
    });

    let child = Arc::new(Mutex::new(child));
    world.insert_resource(StandalonePlayState {
        child: Some(child),
        stdout_pump: Some(stdout_pump),
        stderr_pump: Some(stderr_pump),
    });
    info!("standalone_play: started");
}

pub fn stop_standalone_play(world: &mut World) {
    // Pull the running child + pump handles out of the resource in
    // a tight scope so the `Mut<_>` borrow on the world ends before
    // we start blocking on the pump joins below.
    let (child_arc, stdout_pump, stderr_pump) = {
        let Some(mut state) = world.get_resource_mut::<StandalonePlayState>() else {
            return;
        };
        let Some(child_arc) = state.child.take() else {
            return;
        };
        (
            child_arc,
            state.stdout_pump.take(),
            state.stderr_pump.take(),
        )
    };

    let mut child = match child_arc.lock() {
        Ok(g) => g,
        Err(e) => {
            warn!("standalone_play: child mutex poisoned: {e}");
            return;
        }
    };

    #[cfg(unix)]
    {
        // Best-effort SIGTERM so the game has a chance to run its
        // own shutdown logic (close window, flush logs) before we
        // fall back to SIGKILL below.
        // SAFETY: `child.id()` is a valid PID for as long as we hold
        // the `Child`; `kill(2)` with `SIGTERM` has no preconditions
        // beyond a valid PID and matching UID, both of which we
        // satisfy by virtue of having spawned this process ourselves.
        unsafe {
            libc::kill(child.id() as i32, libc::SIGTERM);
        }
    }
    #[cfg(windows)]
    {
        let _ = child.kill();
    }

    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(200);
    while std::time::Instant::now() < deadline {
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    if matches!(child.try_wait(), Ok(None)) {
        let _ = child.kill();
        let _ = child.wait();
    }

    drop(child);
    if let Some(p) = stdout_pump {
        let _ = p.join();
    }
    if let Some(p) = stderr_pump {
        let _ = p.join();
    }
    info!("standalone_play: stopped");
}
