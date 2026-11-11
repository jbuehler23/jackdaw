//! The game Play ran, started again on its own and timed to its scene's spawn.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use super::logs;

/// How long the game keeps running after its spawn, so the frame showing it is
/// drawn before the game is stopped.
const AFTER_SPAWN: Duration = Duration::from_secs(1);

/// How long the run waits for the scene once the game has spawned others and
/// gone quiet, since a game that starts on another scene may never spawn it.
const OTHER_SCENES_GRACE: Duration = Duration::from_secs(15);

/// The variables only a game launched by Play holds: its link to the editor
/// and what the editor asked of it.
const PLAY_ONLY_VARS: [&str; 4] = [
    "JACKDAW_PIE",
    "JACKDAW_PIE_WINDOWLESS",
    "JACKDAW_PIE_SCENE",
    "JACKDAW_PIE_REPLACES",
];

/// How a running process was started.
#[derive(Clone, Debug)]
pub struct Launch {
    pub exe: PathBuf,
    pub cwd: PathBuf,
    /// The arguments after the program name.
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
}

impl Launch {
    /// How the process `pid` was started, read from `/proc`.
    pub fn of(pid: u32) -> Option<Self> {
        let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
        let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let environ = std::fs::read(format!("/proc/{pid}/environ")).ok()?;
        let args = nul_separated(&cmdline).skip(1).collect();
        let env = nul_separated(&environ)
            .filter_map(|entry| {
                let text = entry.to_string_lossy();
                let (key, value) = text.split_once('=')?;
                Some((OsString::from(key), OsString::from(value)))
            })
            .collect();
        Some(Self {
            exe,
            cwd,
            args,
            env,
        })
    }

    /// Whether this is a game Play started, rather than a build script or
    /// anything else running out of the build folder.
    pub fn is_played_game(&self) -> bool {
        !is_build_script(&self.exe) && self.env.iter().any(|(key, _)| key == "JACKDAW_PIE")
    }

    /// The same launch without the editor: no link and no windowless mode, and
    /// a log filter that shows the runtime's spawns.
    fn without_editor(&self) -> Command {
        let mut command = Command::new(&self.exe);
        command.current_dir(&self.cwd).args(&self.args).env_clear();
        let mut filter = None;
        for (key, value) in &self.env {
            if key == "RUST_LOG" {
                filter = Some(value.to_string_lossy().into_owned());
            } else if !PLAY_ONLY_VARS.iter().any(|play| key == play) {
                command.env(key, value);
            }
        }
        command.env("RUST_LOG", spawn_filter(filter.as_deref()));
        command
    }
}

fn nul_separated(bytes: &[u8]) -> impl Iterator<Item = OsString> + '_ {
    bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| OsString::from(String::from_utf8_lossy(part).into_owned()))
}

/// Whether `exe` is a cargo build script.
fn is_build_script(exe: &Path) -> bool {
    exe.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("build-script-"))
}

/// `filter` with the runtime's spawn lines let through.
fn spawn_filter(filter: Option<&str>) -> String {
    match filter.filter(|filter| !filter.trim().is_empty()) {
        Some(filter) => format!("{filter},jackdaw_runtime=debug"),
        None => "info,jackdaw_runtime=debug".to_string(),
    }
}

/// Start `launch` again with no editor, copy what it prints to `log`, and wait
/// up to `timeout` for it to spawn `scene`. Answers how long after launch.
pub fn run(launch: &Launch, scene: &str, log: &Path, timeout: Duration) -> Result<f64, String> {
    let copy =
        std::fs::File::create(log).map_err(|err| format!("create {}: {err}", log.display()))?;
    let copy = Arc::new(Mutex::new(copy));
    let mut command = launch.without_editor();
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let launched_wall = SystemTime::now();
    let launched = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|err| format!("launch {}: {err}", launch.exe.display()))?;
    let lines = logs::Lines::default();
    if let Some(stdout) = child.stdout.take() {
        logs::capture(stdout, lines.clone(), copy.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        logs::capture(stderr, lines.clone(), copy);
    }
    let mut read = 0;
    let mut others: Vec<String> = Vec::new();
    let mut last_other: Option<Instant> = None;
    let outcome = loop {
        if launched.elapsed() > timeout {
            break Err(format!(
                "{scene} did not spawn within {}",
                super::report::duration(timeout.as_secs_f64() * 1000.0)
            ));
        }
        if last_other.is_some_and(|at| at.elapsed() > OTHER_SCENES_GRACE) {
            break Err(format!(
                "the game spawned {} but not {scene}",
                others.join(", ")
            ));
        }
        if let Ok(Some(status)) = child.try_wait() {
            break Err(format!("the game exited ({status}) before {scene} spawned"));
        }
        let fresh: Vec<logs::Line> = lines
            .lock()
            .map(|lines| lines.get(read..).map(<[_]>::to_vec).unwrap_or_default())
            .unwrap_or_default();
        read += fresh.len();
        let mut found = None;
        for line in &fresh {
            let Some(spawned) = scene_spawned(&line.text) else {
                continue;
            };
            if same_scene(&spawned, scene) {
                found = Some(line_offset(line, launched, launched_wall));
                break;
            }
            if !others.contains(&spawned) {
                others.push(spawned);
            }
            last_other = Some(Instant::now());
        }
        if let Some(at) = found {
            std::thread::sleep(AFTER_SPAWN);
            break Ok(at);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = child.kill();
    let _ = child.wait();
    outcome
}

/// When `line` was printed, in milliseconds from launch, by the line's own
/// timestamp where it has one.
pub fn line_offset(line: &logs::Line, launched: Instant, launched_wall: SystemTime) -> f64 {
    logs::line_time(&line.text)
        .and_then(|at| at.duration_since(launched_wall).ok())
        .unwrap_or_else(|| line.at.saturating_duration_since(launched))
        .as_secs_f64()
        * 1000.0
}

/// The scene a runtime log line says has spawned.
fn scene_spawned(text: &str) -> Option<String> {
    let (_, rest) = text.split_once(" Spawned scene '")?;
    let (scene, _) = rest.rsplit_once('\'')?;
    Some(scene.to_string())
}

/// Whether two asset paths name the same scene, whichever separator each uses.
pub fn same_scene(a: &str, b: &str) -> bool {
    let normal = |path: &str| path.replace('\\', "/").trim_start_matches("./").to_string();
    normal(a) == normal(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawn_line_names_its_scene() {
        assert_eq!(
            scene_spawned(
                "2026-11-10T23:25:35.861541Z DEBUG jackdaw_runtime: Spawned scene 'levels/farm.bsn'"
            ),
            Some("levels/farm.bsn".to_string())
        );
        assert_eq!(
            scene_spawned("INFO jackdaw_runtime: Loaded 43 asset files"),
            None
        );
    }

    #[test]
    fn only_the_scene_asked_for_matches() {
        assert!(same_scene("levels/farm.bsn", "levels/farm.bsn"));
        assert!(same_scene("levels\\farm.bsn", "./levels/farm.bsn"));
        assert!(!same_scene("menu.bsn", "levels/farm.bsn"));
        assert!(!same_scene("prefabs/lamp.bsn", "levels/farm.bsn"));
    }

    #[test]
    fn a_spawn_is_timed_by_the_line_s_own_timestamp() {
        let launched = Instant::now();
        let launched_wall = logs::line_time("2026-11-10T23:25:32.500000Z").unwrap();
        let line = logs::Line {
            at: launched + Duration::from_secs(9),
            text: "2026-11-10T23:25:35.750000Z DEBUG jackdaw_runtime: Spawned scene 'a.bsn'"
                .to_string(),
        };
        let at = line_offset(&line, launched, launched_wall);
        assert!((at - 3250.0).abs() < 1.0, "{at}");
    }

    fn launch(exe: &str, env: &[(&str, &str)]) -> Launch {
        Launch {
            exe: PathBuf::from(exe),
            cwd: PathBuf::from("/project"),
            args: vec![OsString::from("--level"), OsString::from("2")],
            env: env
                .iter()
                .map(|(key, value)| (OsString::from(key), OsString::from(value)))
                .collect(),
        }
    }

    #[test]
    fn a_build_script_is_not_taken_for_the_game() {
        let pie = [("JACKDAW_PIE", "/tmp/rendezvous")];
        assert!(
            launch("/p/.jackdaw/target/game/debug/farm", &pie).is_played_game(),
            "the game Play launched"
        );
        assert!(
            !launch(
                "/p/.jackdaw/target/game/debug/build/libc-1a2b/build-script-build",
                &pie
            )
            .is_played_game()
        );
        assert!(
            !launch("/p/.jackdaw/target/game/debug/farm", &[("HOME", "/home/x")]).is_played_game(),
            "a process Play did not start"
        );
    }

    #[test]
    fn the_run_without_the_editor_keeps_play_s_launch_but_not_its_link() {
        let played = launch(
            "/p/.jackdaw/target/game/debug/farm",
            &[
                ("JACKDAW_PIE", "/tmp/rendezvous"),
                ("JACKDAW_PIE_WINDOWLESS", "1"),
                ("BEVY_ASSET_ROOT", "/project"),
                ("LD_LIBRARY_PATH", "/p/.jackdaw/target/game/debug/deps"),
                ("RUST_LOG", "warn"),
            ],
        );
        let command = played.without_editor();
        let env: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        let value = |name: &str| {
            env.iter()
                .find(|(key, _)| key == name)
                .and_then(|(_, value)| value.clone())
        };
        assert_eq!(value("BEVY_ASSET_ROOT").as_deref(), Some("/project"));
        assert!(value("LD_LIBRARY_PATH").is_some());
        assert_eq!(value("JACKDAW_PIE"), None);
        assert_eq!(value("JACKDAW_PIE_WINDOWLESS"), None);
        assert_eq!(
            value("RUST_LOG").as_deref(),
            Some("warn,jackdaw_runtime=debug")
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/project")));
        assert_eq!(command.get_args().count(), 2);
    }
}
