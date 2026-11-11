//! The game run on its own, as `cargo run` starts it, timed to its scene's spawn.

use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use super::logs;

/// How long the game keeps running after its spawn, so the frame showing it is
/// drawn before the game is stopped.
const AFTER_SPAWN: Duration = Duration::from_secs(1);

/// A scene the game spawned: how long after launch, and its name.
#[derive(Debug, PartialEq)]
pub struct Spawned {
    pub at: f64,
    pub scene: String,
}

/// Launch the game `exe` from `project` with no editor, copy what it prints to
/// `log`, and wait up to `timeout` for it to spawn a scene.
pub fn run(exe: &Path, project: &Path, log: &Path, timeout: Duration) -> Result<Spawned, String> {
    let copy =
        std::fs::File::create(log).map_err(|err| format!("create {}: {err}", log.display()))?;
    let copy = Arc::new(Mutex::new(copy));
    let mut command = Command::new(exe);
    command
        .current_dir(project)
        .env("CARGO_MANIFEST_DIR", project)
        .env_remove("RUST_LOG")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let launched_wall = SystemTime::now();
    let launched = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|err| format!("launch {}: {err}", exe.display()))?;
    let lines = logs::Lines::default();
    if let Some(stdout) = child.stdout.take() {
        logs::capture(stdout, lines.clone(), copy.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        logs::capture(stderr, lines.clone(), copy);
    }
    let mut read = 0;
    let outcome = loop {
        if launched.elapsed() > timeout {
            break Err(format!(
                "no scene spawned within {}",
                super::report::duration(timeout.as_secs_f64() * 1000.0)
            ));
        }
        if let Ok(Some(status)) = child.try_wait() {
            break Err(format!("the game exited ({status}) before a scene spawned"));
        }
        let fresh: Vec<logs::Line> = lines
            .lock()
            .map(|lines| lines.get(read..).map(<[_]>::to_vec).unwrap_or_default())
            .unwrap_or_default();
        read += fresh.len();
        if let Some(spawned) = fresh
            .iter()
            .find_map(|line| spawned_in(line, launched, launched_wall))
        {
            std::thread::sleep(AFTER_SPAWN);
            break Ok(spawned);
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let _ = child.kill();
    let _ = child.wait();
    outcome
}

/// The spawn a game log line reports, timed from launch by the line's own
/// timestamp where it has one.
fn spawned_in(line: &logs::Line, launched: Instant, launched_wall: SystemTime) -> Option<Spawned> {
    let scene = scene_spawned(&line.text)?;
    let at = logs::line_time(&line.text)
        .and_then(|at| at.duration_since(launched_wall).ok())
        .unwrap_or_else(|| line.at.saturating_duration_since(launched));
    Some(Spawned {
        at: at.as_secs_f64() * 1000.0,
        scene,
    })
}

/// The scene a game log line says has spawned.
pub fn scene_spawned(text: &str) -> Option<String> {
    let (_, rest) = text.split_once(" Spawned scene '")?;
    let (scene, _) = rest.rsplit_once('\'')?;
    Some(scene.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spawn_line_names_its_scene() {
        assert_eq!(
            scene_spawned(
                "2026-11-10T23:25:35.861541Z  INFO jackdaw_runtime: Spawned scene 'levels/farm.bsn'"
            ),
            Some("levels/farm.bsn".to_string())
        );
        assert_eq!(
            scene_spawned("INFO jackdaw_runtime: Loaded 43 asset files"),
            None
        );
    }

    #[test]
    fn a_spawn_is_timed_by_the_line_s_own_timestamp() {
        let launched = Instant::now();
        let launched_wall = logs::line_time("2026-11-10T23:25:32.500000Z").unwrap();
        let line = logs::Line {
            at: launched + Duration::from_secs(9),
            text: "2026-11-10T23:25:35.750000Z  INFO jackdaw_runtime: Spawned scene 'a.bsn'"
                .to_string(),
        };
        let spawned = spawned_in(&line, launched, launched_wall).unwrap();
        assert_eq!(spawned.scene, "a.bsn");
        assert!((spawned.at - 3250.0).abs() < 1.0, "{}", spawned.at);
    }
}
