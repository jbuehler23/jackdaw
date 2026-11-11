//! The editor process under test, and stopping it when the probe is
//! interrupted.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{logs, system};

/// How long the editor gets to exit on its own before it is killed.
const EXIT_GRACE: Duration = Duration::from_secs(10);

static RUNNING_EDITOR: AtomicU32 = AtomicU32::new(0);
static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Stop the running editor on the first Ctrl-C, so the run ends through its
/// usual path and puts the project back; kill it on the second.
pub fn handle_interrupts() {
    let installed = ctrlc::set_handler(|| {
        let again = INTERRUPTED.swap(true, Ordering::SeqCst);
        let pid = RUNNING_EDITOR.load(Ordering::SeqCst);
        if pid != 0 {
            signal(pid, if again { "KILL" } else { "INT" });
        }
    });
    if let Err(err) = installed {
        eprintln!("perf: Ctrl-C will not stop the editor cleanly: {err}");
    }
}

/// Whether Ctrl-C ended the run.
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::SeqCst)
}

/// What the editor is started with.
pub struct Launch<'a> {
    pub editor: &'a Path,
    pub project: &'a Path,
    pub port: u16,
    pub config: &'a Path,
    pub log: &'a Path,
}

pub struct Editor {
    child: Child,
    pub pid: u32,
    pub started: Instant,
    lines: logs::Lines,
    project: PathBuf,
    running: bool,
}

impl Editor {
    pub fn launch(launch: &Launch) -> Result<Self, String> {
        let copy = std::fs::File::create(launch.log)
            .map_err(|err| format!("create {}: {err}", launch.log.display()))?;
        let copy = Arc::new(Mutex::new(copy));
        let mut command = Command::new(launch.editor);
        command
            .current_dir(launch.project)
            .env("JACKDAW_OPEN_PROJECT", launch.project)
            .env("JACKDAW_REMOTE_PORT", launch.port.to_string())
            .env("JACKDAW_CONFIG_DIR", launch.config)
            .env("JACKDAW_PIE_PREBUILD", "0")
            .env_remove("RUST_LOG")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Its own process group, so a Ctrl-C in the terminal reaches the probe
        // alone and the probe decides how the editor stops.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        let started = Instant::now();
        let mut child = command
            .spawn()
            .map_err(|err| format!("launch {}: {err}", launch.editor.display()))?;
        RUNNING_EDITOR.store(child.id(), Ordering::SeqCst);
        let lines = logs::Lines::default();
        if let Some(stdout) = child.stdout.take() {
            logs::capture(stdout, lines.clone(), copy.clone());
        }
        if let Some(stderr) = child.stderr.take() {
            logs::capture(stderr, lines.clone(), copy);
        }
        Ok(Self {
            pid: child.id(),
            child,
            started,
            lines,
            project: launch.project.to_path_buf(),
            running: true,
        })
    }

    /// Why the editor is gone, if it is.
    pub fn exited(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            Ok(None) => None,
            Err(err) => Some(err.to_string()),
        }
    }

    pub fn lines_from(&self, from: usize) -> Vec<logs::Line> {
        self.lines
            .lock()
            .map(|lines| lines.get(from..).map(<[_]>::to_vec).unwrap_or_default())
            .unwrap_or_default()
    }

    pub fn line_count(&self) -> usize {
        self.lines.lock().map_or(0, |lines| lines.len())
    }

    /// The game processes Play started, by their executables under the
    /// project's build folder.
    pub fn games(&self) -> Vec<u32> {
        system::descendants_running_from(self.pid, &self.project.join(".jackdaw/target"))
            .into_iter()
            .map(|(pid, _)| pid)
            .collect()
    }

    /// How Play started the game it runs, when one is running.
    pub fn played_game(&self) -> Option<super::plain::Launch> {
        system::descendants_running_from(self.pid, &self.project.join(".jackdaw/target"))
            .into_iter()
            .filter_map(|(pid, _)| super::plain::Launch::of(pid))
            .find(super::plain::Launch::is_played_game)
    }

    /// Ask the editor to quit as Ctrl-C would, which closes its games' links
    /// so they exit too, and kill it and anything it left running if it has
    /// not gone within [`EXIT_GRACE`].
    pub fn shut_down(&mut self) {
        if !self.running {
            return;
        }
        self.running = false;
        let started = system::descendants_of(self.pid);
        signal(self.pid, "INT");
        let deadline = Instant::now() + EXIT_GRACE;
        while self.exited().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        RUNNING_EDITOR.store(0, Ordering::SeqCst);
        for pid in started {
            if system::memory_of(pid).is_some() {
                signal(pid, "KILL");
            }
        }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.shut_down();
    }
}

/// Send `name` to `pid`. Where there are no signals the editor is killed by
/// [`Editor::shut_down`] instead.
fn signal(pid: u32, name: &str) {
    if cfg!(unix) {
        let _ = Command::new("kill")
            .arg(format!("-{name}"))
            .arg(pid.to_string())
            .stderr(Stdio::null())
            .status();
    }
}
