//! `cargo xtask perf`: launch the editor on a project, drive it through its
//! remote API the way a person would, and time each path.

mod brp;
mod logs;
mod preserve;
mod report;
mod scene;
mod stats;
mod system;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};

use brp::{Brp, CallError, STALL_TIMEOUT, ms_since};
use preserve::Preserved;
use report::{Report, Row};
use scene::SceneShape;
use stats::{STALL_MS, Settle, Summary};

const USAGE: &str = "usage: cargo xtask perf --project <DIR> [--scene <path under assets>] \
                     [--editor <jackdaw binary>] [--out <DIR>] [--no-play] \
                     [--play-timeout <seconds>] [--load-timeout <seconds>]";

/// Status round trips taken for the idle frame time.
const IDLE_SAMPLES: usize = 150;
/// Steps in the simulated drag.
const DRAG_STEPS: u32 = 30;
/// How long the editor must go without a stall to count as settled.
const QUIET: Duration = Duration::from_secs(2);
/// Longest wait for an operation to settle.
const SETTLE_CAP: Duration = Duration::from_secs(180);
const TRANSFORM: &str = "bevy_transform::components::transform::Transform";

const LAUNCHED: &str = "Game launched";
const CONNECTED: &str = "Game connected";
const ASKED: &str = "Game asked for the scene";
const ASSETS: &str = "Game loaded its asset files";
const SENT: &str = "Scene sent to the game";
const SPAWNED: &str = "Scene spawned in the game";
const STREAMING: &str = "Game started streaming frames";
const FIRST_FRAME: &str = "First frame shown in the Game panel";
const PLAY_STAGES: [&str; 8] = [
    LAUNCHED,
    CONNECTED,
    ASKED,
    ASSETS,
    SENT,
    SPAWNED,
    STREAMING,
    FIRST_FRAME,
];

struct Options {
    project: PathBuf,
    scene: String,
    editor: PathBuf,
    out: PathBuf,
    play: bool,
    play_timeout: Duration,
    load_timeout: Duration,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut project = None;
        let mut scene = "scene.bsn".to_string();
        let mut editor = None;
        let mut out = None;
        let mut play = true;
        let mut play_timeout = Duration::from_secs(300);
        let mut load_timeout = Duration::from_secs(600);
        let mut rest = args.iter();
        while let Some(arg) = rest.next() {
            let mut value = || {
                rest.next()
                    .cloned()
                    .ok_or_else(|| format!("{arg} needs a value"))
            };
            let seconds = |text: String| {
                text.parse::<u64>()
                    .map(Duration::from_secs)
                    .map_err(|_| format!("{arg} takes whole seconds, got {text}"))
            };
            match arg.as_str() {
                "--project" => project = Some(PathBuf::from(value()?)),
                "--scene" => scene = value()?,
                "--editor" => editor = Some(PathBuf::from(value()?)),
                "--out" => out = Some(PathBuf::from(value()?)),
                "--no-play" => play = false,
                "--play-timeout" => play_timeout = seconds(value()?)?,
                "--load-timeout" => load_timeout = seconds(value()?)?,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        let project = project.ok_or("--project is required")?;
        let project = std::fs::canonicalize(&project)
            .map_err(|err| format!("project {}: {err}", project.display()))?;
        let editor = editor.unwrap_or_else(|| workspace().join("target/debug/jackdaw"));
        if !editor.is_file() {
            return Err(format!(
                "no editor at {}; build it with `cargo build -p jackdaw` or pass --editor",
                editor.display()
            ));
        }
        let out = out.unwrap_or_else(|| project.join(".jackdaw/perf"));
        Ok(Self {
            project,
            scene,
            editor,
            out,
            play,
            play_timeout,
            load_timeout,
        })
    }

    fn scene_file(&self) -> PathBuf {
        self.project.join("assets").join(&self.scene)
    }
}

pub fn cmd(args: &[String]) -> bool {
    let options = match Options::parse(args) {
        Ok(options) => options,
        Err(why) => {
            eprintln!("perf: {why}\n{USAGE}");
            return false;
        }
    };
    match run(&options) {
        Ok(finished) => finished,
        Err(why) => {
            eprintln!("perf: {why}");
            false
        }
    }
}

/// Measure, write the report, and say whether every path ran.
fn run(options: &Options) -> Result<bool, String> {
    let scene_file = options.scene_file();
    let scene_bytes = std::fs::metadata(&scene_file)
        .map_err(|err| format!("scene {}: {err}", scene_file.display()))?
        .len();
    std::fs::create_dir_all(&options.out)
        .map_err(|err| format!("create {}: {err}", options.out.display()))?;
    let started_at = SystemTime::now();
    let stamp: String = logs::format_utc(started_at)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let stem = scene_file.file_stem().map_or_else(
        || "scene".into(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    let base = options.out.join(format!("perf-{stem}-{stamp}"));
    let editor_log = base.with_extension("log");
    let machine = system::Machine::read();
    let load_start = system::load_average();

    let state = options.project.join(".jackdaw");
    let out = std::fs::canonicalize(&options.out).unwrap_or_else(|_| options.out.clone());
    let preserved = Preserved::take(&state, &[state.join("target"), out]);
    let port = free_port()?;
    let mut editor = Editor::launch(options, port, &editor_log)?;
    eprintln!("perf: editor pid {} on port {port}", editor.pid);
    let mut probe = Probe {
        brp: Brp::new(port),
        options,
        rows: Vec::new(),
        shape: SceneShape::default(),
        stamp: stamp.clone(),
    };
    let outcome = probe.measure(&mut editor);
    editor.shut_down();
    for failed in preserved.restore() {
        eprintln!("perf: {failed}");
    }
    if let Err(why) = &outcome {
        probe
            .rows
            .push(Row::missing("Run stopped early", &why.to_string()));
    }

    let report = Report {
        fields: vec![
            ("date", json!(logs::format_utc(started_at))),
            ("commit", json!(commit())),
            ("editor", json!(options.editor.display().to_string())),
            ("project", json!(options.project.display().to_string())),
            ("scene", json!(options.scene)),
            ("scene_bytes", json!(scene_bytes)),
            ("nodes", json!(probe.shape.nodes)),
            ("brushes", json!(probe.shape.brushes)),
            ("cpu", json!(machine.cpu)),
            ("ram_bytes", json!(machine.ram_bytes)),
            ("load_start", json!(load_start)),
            ("load_end", json!(system::load_average())),
            ("editor_log", json!(editor_log.display().to_string())),
        ],
        rows: probe.rows,
    };
    let markdown = report.to_markdown();
    let table = base.with_extension("md");
    let data = base.with_extension("json");
    std::fs::write(&table, &markdown).map_err(|err| format!("write {}: {err}", table.display()))?;
    let pretty = serde_json::to_string_pretty(&report.to_json()).map_err(|err| err.to_string())?;
    std::fs::write(&data, pretty).map_err(|err| format!("write {}: {err}", data.display()))?;
    println!("{markdown}");
    println!("wrote {} and {}", table.display(), data.display());
    Ok(outcome.is_ok())
}

/// The workspace this xtask belongs to.
fn workspace() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest.parent().unwrap_or(manifest).to_path_buf()
}

fn commit() -> String {
    let git = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(workspace())
            .args(args)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
    };
    let Some(head) = git(&["rev-parse", "--short", "HEAD"]) else {
        return "unknown".to_string();
    };
    match git(&["status", "--porcelain", "--untracked-files=no"]) {
        Some(changes) if !changes.is_empty() => format!("{head}-dirty"),
        _ => head,
    }
}

fn free_port() -> Result<u16, String> {
    std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .and_then(|listener| listener.local_addr())
        .map(|addr| addr.port())
        .map_err(|err| format!("no free port: {err}"))
}

/// The editor process under test.
struct Editor {
    child: Child,
    pid: u32,
    started: Instant,
    lines: logs::Lines,
    project: PathBuf,
    running: bool,
}

impl Editor {
    fn launch(options: &Options, port: u16, log: &Path) -> Result<Self, String> {
        let copy =
            std::fs::File::create(log).map_err(|err| format!("create {}: {err}", log.display()))?;
        let copy = Arc::new(Mutex::new(copy));
        let started = Instant::now();
        let mut child = Command::new(&options.editor)
            .current_dir(&options.project)
            .env("JACKDAW_OPEN_PROJECT", &options.project)
            .env("JACKDAW_REMOTE_PORT", port.to_string())
            .env("JACKDAW_PIE_PREBUILD", "0")
            .env_remove("RUST_LOG")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| format!("launch {}: {err}", options.editor.display()))?;
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
            project: options.project.clone(),
            running: true,
        })
    }

    /// Why the editor is gone, if it is.
    fn exited(&mut self) -> Option<String> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(status.to_string()),
            Ok(None) => None,
            Err(err) => Some(err.to_string()),
        }
    }

    fn lines_from(&self, from: usize) -> Vec<logs::Line> {
        self.lines
            .lock()
            .map(|lines| lines.get(from..).map(<[_]>::to_vec).unwrap_or_default())
            .unwrap_or_default()
    }

    fn line_count(&self) -> usize {
        self.lines.lock().map_or(0, |lines| lines.len())
    }

    /// The game processes Play started, by their executables under the
    /// project's build folder.
    fn games(&self) -> Vec<u32> {
        system::descendants_running_from(self.pid, &self.project.join(".jackdaw/target"))
            .into_iter()
            .map(|(pid, _)| pid)
            .collect()
    }

    /// Stop the editor and every process it started.
    fn shut_down(&mut self) {
        if !self.running {
            return;
        }
        self.running = false;
        let started = system::descendants_of(self.pid);
        let _ = self.child.kill();
        let _ = self.child.wait();
        if !started.is_empty() {
            let _ = Command::new("kill")
                .args(started.iter().map(u32::to_string))
                .stderr(Stdio::null())
                .status();
        }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.shut_down();
    }
}

struct Probe<'a> {
    brp: Brp,
    options: &'a Options,
    rows: Vec<Row>,
    shape: SceneShape,
    stamp: String,
}

impl Probe<'_> {
    /// Every path, in the order a person would take them. A refused step is
    /// recorded and the run goes on; an editor that stops answering ends it.
    fn measure(&mut self, editor: &mut Editor) -> Result<(), CallError> {
        self.launch(editor)?;
        self.memory("Editor memory after load", editor.pid);
        let tree = self
            .brp
            .call("jackdaw/scene_tree", json!({}), STALL_TIMEOUT)?;
        self.shape = SceneShape::of(&tree);
        self.attempt("Idle frame, Realtime on", |probe| probe.idle())?;
        self.attempt("Select one node", |probe| probe.select())?;
        self.attempt("Drag", |probe| probe.drag())?;
        self.attempt("Undo", |probe| probe.undo())?;
        self.memory("Editor memory after edits", editor.pid);
        self.attempt("Tab switch", |probe| probe.switch_tabs())?;
        if self.options.play {
            self.attempt("Play", |probe| probe.play(editor))?;
            self.attempt("Stop", |probe| probe.stop())?;
        }
        self.attempt("Save", |probe| probe.save())?;
        self.memory("Editor memory at end", editor.pid);
        self.rows.push(Row::missing(
            "Plain run, first frame",
            "not measured: a windowed game shows nothing the probe can see without a display hook",
        ));
        Ok(())
    }

    fn attempt(
        &mut self,
        path: &str,
        step: impl FnOnce(&mut Self) -> Result<(), CallError>,
    ) -> Result<(), CallError> {
        match step(self) {
            Err(CallError::Refused(why)) => {
                self.rows.push(Row::missing(path, &why));
                Ok(())
            }
            other => other,
        }
    }

    fn memory(&mut self, path: &str, pid: u32) {
        self.rows.push(match system::memory_of(pid) {
            Some(bytes) => Row::bytes(path, bytes).with_note("RSS + swap"),
            None => Row::missing(path, "no /proc entry"),
        });
    }

    fn scene_is_open(&self, status: &Value) -> bool {
        let Some(open) = status["scene"].as_str().map(Path::new) else {
            return false;
        };
        let wanted = self.options.scene_file();
        open.ends_with(Path::new("assets").join(&self.options.scene))
            || std::fs::canonicalize(open).ok() == std::fs::canonicalize(wanted).ok()
    }

    /// From launch until the scene is open and the editor has been idle and
    /// quick for a second.
    fn launch(&mut self, editor: &mut Editor) -> Result<(), CallError> {
        let mut answered = None;
        let mut loaded_since: Option<f64> = None;
        let mut elsewhere_since: Option<Instant> = None;
        let mut opened = false;
        let mut dialog = None;
        let loaded = loop {
            if let Some(why) = editor.exited() {
                return Err(CallError::Unreachable(format!("the editor exited ({why})")));
            }
            if editor.started.elapsed() > self.options.load_timeout {
                return Err(CallError::Unreachable(
                    "the scene did not load in time".to_string(),
                ));
            }
            let (status, rtt) = match self.brp.status() {
                Ok(answer) => answer,
                Err(CallError::Unreachable(_)) if answered.is_none() => {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(err) => return Err(err),
            };
            let at = ms_since(editor.started);
            answered.get_or_insert(at);
            if let Some(title) = status["dialog"]["title"].as_str() {
                dialog.get_or_insert_with(|| title.to_string());
            }
            let busy = !status["progress"].is_null();
            let here = self.scene_is_open(&status);
            if here && !busy && rtt < STALL_MS {
                let since = *loaded_since.get_or_insert(at);
                if at - since >= 1000.0 {
                    break since;
                }
            } else {
                loaded_since = None;
            }
            if here || busy {
                elsewhere_since = None;
            } else if !opened
                && elsewhere_since
                    .get_or_insert_with(Instant::now)
                    .elapsed()
                    .as_millis()
                    > 1500
            {
                self.brp
                    .operator("scene.open", json!({ "path": self.options.scene }))?;
                opened = true;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        if let Some(answered) = answered {
            self.rows
                .push(Row::duration("Launch to first remote answer", answered));
        }
        let mut row = Row::duration("Launch to scene loaded", loaded);
        let mut notes = Vec::new();
        if opened {
            notes.push("includes scene.open after the restored tabs".to_string());
        }
        if let Some(dialog) = dialog {
            notes.push(format!("dialog up: {dialog}"));
        }
        row.note = notes.join("; ");
        self.rows.push(row);
        self.brp
            .operator("viewport.realtime.toggle", json!({ "on": true }))?;
        self.settle(Instant::now(), 0.0, |_| true)?;
        Ok(())
    }

    fn idle(&mut self) -> Result<(), CallError> {
        let mut rtts = Vec::with_capacity(IDLE_SAMPLES);
        for _ in 0..IDLE_SAMPLES {
            rtts.push(self.brp.status()?.1);
        }
        if let Some(summary) = Summary::of(&rtts) {
            self.rows.push(
                Row::summary("Idle frame, Realtime on", &summary)
                    .with_note("status round trip; vsync floors it at about 16.6 ms"),
            );
        }
        Ok(())
    }

    fn target(&self) -> Result<u64, CallError> {
        self.shape
            .target
            .ok_or_else(|| CallError::Refused("the scene has no node to act on".to_string()))
    }

    fn select(&mut self) -> Result<(), CallError> {
        let target = self.target()?;
        let start = Instant::now();
        let (_, call) = self
            .brp
            .operator("selection.select", json!({ "entity": target }))?;
        let (settle, _) = self.settle(start, call, |_| true)?;
        let path = if self.shape.brushes > 0 {
            "Select one brush"
        } else {
            "Select one node"
        };
        self.rows.push(Row::settle(path, &settle));
        Ok(())
    }

    fn drag(&mut self) -> Result<(), CallError> {
        let target = self.target()?;
        let base = self
            .brp
            .call(
                "world.get_components",
                json!({ "entity": target, "components": [TRANSFORM] }),
                STALL_TIMEOUT,
            )
            .ok()
            .and_then(|answer| answer["components"][TRANSFORM]["translation"][0].as_f64())
            .unwrap_or(0.0);
        let start = Instant::now();
        let mut calls = Vec::new();
        for step in 1..=DRAG_STEPS {
            let x = base + 0.05 * f64::from(step);
            let (_, took) = self
                .brp
                .operator("entity.set_transform", json!({ "entity": target, "x": x }))?;
            calls.push(took);
        }
        let total = ms_since(start);
        let (after, _) = self.settle(Instant::now(), 0.0, |_| true)?;
        if let Some(summary) = Summary::of(&calls) {
            self.rows.push(
                Row::summary("Drag, 30 entity.set_transform calls", &summary).with_note(format!(
                    "per call; all {} in {}, then settled in {}, worst frame after {}",
                    DRAG_STEPS,
                    report::duration(total),
                    report::duration(after.settled),
                    report::duration(after.worst_frame)
                )),
            );
        }
        Ok(())
    }

    fn undo(&mut self) -> Result<(), CallError> {
        let start = Instant::now();
        let (_, call) = self.brp.operator("history.undo", json!({}))?;
        let (settle, _) = self.settle(start, call, |_| true)?;
        self.rows.push(Row::settle("Undo one drag step", &settle));
        Ok(())
    }

    fn switch_tabs(&mut self) -> Result<(), CallError> {
        let start = Instant::now();
        let (_, call) = self.brp.operator("scene.new", json!({}))?;
        let (away, _) = self.settle(start, call, |_| true)?;
        self.rows
            .push(Row::settle("Switch away (scene.new)", &away));

        let start = Instant::now();
        let (_, call) = self
            .brp
            .operator("scene.open", json!({ "path": self.options.scene }))?;
        let (back, shown) = self.settle(start, call, |status| self.scene_is_open(status))?;
        let mut row = Row::settle("Switch back (scene.open)", &back);
        if shown.is_none() {
            row.note = "the scene's tab did not come back to the front".to_string();
        }
        self.rows.push(row);
        Ok(())
    }

    fn play(&mut self, editor: &mut Editor) -> Result<(), CallError> {
        let from_line = editor.line_count();
        let start = Instant::now();
        let start_wall = SystemTime::now();
        let (_, call) = self.brp.operator("pie.play", json!({}))?;
        self.rows.push(Row::duration("Play, call answered", call));

        let mut stages: Vec<(&'static str, f64)> = Vec::new();
        let mut read_lines = from_line;
        let mut game_since: Option<u64> = None;
        let mut frame_reported = false;
        let mut failed = None;
        let mut rtts = Vec::new();
        let mut editor_peak = 0;
        let mut game_peak = 0;
        let mut sampled: Option<Instant> = None;
        let stop = Arc::new(AtomicBool::new(false));
        let mut spawn: Option<Receiver<f64>> = None;

        let timed_out = loop {
            if start.elapsed() > self.options.play_timeout {
                break true;
            }
            if let Some(why) = editor.exited() {
                stop.store(true, Ordering::Relaxed);
                return Err(CallError::Unreachable(format!(
                    "the editor exited during Play ({why})"
                )));
            }
            let (status, rtt) = self.brp.status()?;
            rtts.push(rtt);
            let now = ms_since(start);
            if status["pie"] == "failed" {
                failed = Some("the game build failed");
                break false;
            }
            for instance in status["instances"].as_array().into_iter().flatten() {
                match instance["stage"].as_str() {
                    Some("connecting") => note_stage(&mut stages, LAUNCHED, now),
                    Some("connected") => {
                        note_stage(&mut stages, LAUNCHED, now);
                        note_stage(&mut stages, CONNECTED, now);
                    }
                    _ => {}
                }
            }
            frame_reported |= status.get("game_frame").is_some();
            if !status["game_frame"].is_null() {
                note_stage(&mut stages, FIRST_FRAME, now);
            }
            for line in editor.lines_from(read_lines) {
                read_lines += 1;
                if let Some(stage) = editor_stage(&line.text) {
                    let at = line.at.saturating_duration_since(start).as_secs_f64() * 1000.0;
                    note_stage(&mut stages, stage, at);
                }
            }
            if let Ok(output) = self.brp.call(
                "jackdaw/game_output",
                json!({ "since": game_since }),
                Duration::from_secs(10),
            ) {
                for line in output["lines"].as_array().into_iter().flatten() {
                    let text = logs::strip_ansi(line["text"].as_str().unwrap_or_default());
                    if let Some(stage) = game_stage(&text) {
                        let at = logs::line_time(&text)
                            .and_then(|at| at.duration_since(start_wall).ok())
                            .map_or(now, |since| since.as_secs_f64() * 1000.0);
                        note_stage(&mut stages, stage, at);
                    }
                }
                game_since = output["next"].as_u64().or(game_since);
            }
            let connected = stages.iter().any(|(stage, _)| *stage == CONNECTED);
            if spawn.is_none()
                && connected
                && let Some(node) = self.shape.last_node
            {
                spawn = Some(watch_spawn(self.brp_port(), node, start, stop.clone()));
            }
            if let Some(at) = spawn.as_ref().and_then(|spawn| spawn.try_recv().ok()) {
                note_stage(&mut stages, SPAWNED, at);
            }
            if sampled.is_none_or(|at| at.elapsed() >= Duration::from_secs(1)) {
                sampled = Some(Instant::now());
                let (editor_now, games_now) = memory_now(editor);
                editor_peak = editor_peak.max(editor_now);
                game_peak = game_peak.max(games_now);
            }
            let seen = |wanted: &str| stages.iter().any(|(stage, _)| *stage == wanted);
            let done = seen(FIRST_FRAME) || (!frame_reported && seen(SPAWNED) && seen(STREAMING));
            if done {
                break false;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        stop.store(true, Ordering::Relaxed);
        let (editor_now, games_now) = memory_now(editor);
        editor_peak = editor_peak.max(editor_now);
        game_peak = game_peak.max(games_now);

        stages.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (stage, at) in &stages {
            self.rows
                .push(Row::duration(&format!("Play: {stage}"), *at).with_note(stage_source(stage)));
        }
        for stage in PLAY_STAGES {
            if stages.iter().any(|(seen, _)| *seen == stage) {
                continue;
            }
            let why = if let Some(failed) = failed {
                failed.to_string()
            } else if timed_out {
                format!(
                    "not seen within {}",
                    report::duration(self.options.play_timeout.as_secs_f64() * 1000.0)
                )
            } else if stage == FIRST_FRAME || stage == SENT || stage == ASKED {
                "this editor does not report it".to_string()
            } else {
                "not seen before the first frame".to_string()
            };
            self.rows
                .push(Row::missing(&format!("Play: {stage}"), &why));
        }
        if let Some(summary) = Summary::of(&rtts) {
            self.rows.push(
                Row::summary("Editor frames during Play", &summary)
                    .with_note("status round trip while the game starts"),
            );
        }
        self.rows.push(if editor_peak > 0 {
            Row::bytes("Editor memory during Play, peak", editor_peak).with_note("RSS + swap")
        } else {
            Row::missing("Editor memory during Play, peak", "not sampled")
        });
        self.rows.push(if game_peak > 0 {
            Row::bytes("Game memory during Play, peak", game_peak).with_note("RSS + swap")
        } else {
            Row::missing("Game memory during Play, peak", "no game process found")
        });
        Ok(())
    }

    fn stop(&mut self) -> Result<(), CallError> {
        let start = Instant::now();
        let (_, call) = self.brp.operator("pie.stop", json!({}))?;
        let (settle, gone) = self.settle(start, call, |status| {
            status["pie"] == "stopped"
                && status["instances"]
                    .as_array()
                    .is_none_or(|instances| instances.is_empty())
        })?;
        let mut row = Row::settle("Stop", &settle);
        row.note = match gone {
            Some(at) => format!("game gone after {}", report::duration(at)),
            None => "the game was still listed when the wait ended".to_string(),
        };
        self.rows.push(row);
        Ok(())
    }

    /// Save to a copy in the out folder, never over the project's scene.
    fn save(&mut self) -> Result<(), CallError> {
        let dir = self.options.out.join(format!("save-{}", self.stamp));
        let name = Path::new(&self.options.scene)
            .file_name()
            .map_or_else(|| "scene.bsn".into(), |name| name.to_os_string());
        let copy = dir.join(name);
        std::fs::create_dir_all(&dir)
            .map_err(|err| CallError::Refused(format!("create {}: {err}", dir.display())))?;
        let start = Instant::now();
        let saved = self
            .brp
            .operator("scene.save", json!({ "path": copy.display().to_string() }));
        let outcome =
            saved.and_then(|(_, call)| self.settle(start, call, |status| status["dirty"] == false));
        let written = std::fs::metadata(&copy).map(|meta| meta.len()).ok();
        let _ = std::fs::remove_dir_all(&dir);
        let (settle, clean) = outcome?;
        let mut row = Row::settle("Save", &settle);
        row.note = match (written, clean) {
            (Some(bytes), Some(_)) => format!("{} written to a copy", report::bytes(bytes)),
            (Some(bytes), None) => format!(
                "{} written to a copy, but the tab still reads as unsaved",
                report::bytes(bytes)
            ),
            (None, _) => "no file was written".to_string(),
        };
        self.rows.push(row);
        Ok(())
    }

    fn brp_port(&self) -> u16 {
        self.brp.port()
    }

    /// Poll status from `start` until `reached` holds and the editor has gone
    /// [`QUIET`] without a stall. Answers how it settled and when `reached`
    /// first held, from `start`.
    fn settle(
        &self,
        start: Instant,
        call: f64,
        mut reached: impl FnMut(&Value) -> bool,
    ) -> Result<(Settle, Option<f64>), CallError> {
        let mut samples = Vec::new();
        let mut quiet_from = Instant::now();
        let mut reached_at = None;
        loop {
            let asked = Instant::now();
            let (status, rtt) = self.brp.status()?;
            samples.push((
                asked.saturating_duration_since(start).as_secs_f64() * 1000.0,
                rtt,
            ));
            if rtt > STALL_MS {
                quiet_from = Instant::now();
            }
            if reached_at.is_none() && reached(&status) {
                reached_at = Some(ms_since(start));
            }
            let quiet = quiet_from.elapsed() >= QUIET;
            if (quiet && reached_at.is_some()) || start.elapsed() > SETTLE_CAP {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok((Settle::of(call, &samples), reached_at))
    }
}

/// Ask the game on its own thread whether the scene node `node` has spawned,
/// and send when it first has, from `start`.
fn watch_spawn(port: u16, node: u64, start: Instant, stop: Arc<AtomicBool>) -> Receiver<f64> {
    let (send, receive) = channel();
    std::thread::spawn(move || {
        let brp = Brp::new(port);
        let mut asked = 0u32;
        while !stop.load(Ordering::Relaxed) {
            asked += 1;
            let answer = brp.call(
                "jackdaw/game_entity",
                json!({ "node": node, "request": format!("perf-spawn-{asked}") }),
                Duration::from_secs(60),
            );
            let found = answer.as_ref().is_ok_and(|answer| {
                answer["entities"]
                    .as_array()
                    .is_some_and(|entities| !entities.is_empty())
            });
            if found {
                let _ = send.send(ms_since(start));
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
    receive
}

/// Memory of the editor and of every game it runs, in bytes.
fn memory_now(editor: &Editor) -> (u64, u64) {
    let games = editor
        .games()
        .into_iter()
        .filter_map(system::memory_of)
        .sum();
    (system::memory_of(editor.pid).unwrap_or(0), games)
}

/// Record `stage` at `at`, keeping the earliest time it was seen.
fn note_stage(stages: &mut Vec<(&'static str, f64)>, stage: &'static str, at: f64) {
    match stages.iter_mut().find(|(seen, _)| *seen == stage) {
        Some((_, earliest)) => *earliest = earliest.min(at),
        None => stages.push((stage, at)),
    }
}

/// The Play stage an editor log line marks.
fn editor_stage(text: &str) -> Option<&'static str> {
    let (_, message) = text.split_once("PIE: ")?;
    let message = message.trim_end();
    if message.ends_with("launched, awaiting connection") {
        Some(LAUNCHED)
    } else if message.ends_with(" connected") {
        Some(CONNECTED)
    } else if message.contains(" asked for the scene ") {
        Some(ASKED)
    } else if message.ends_with(" was sent the scene it asked for") {
        Some(SENT)
    } else {
        None
    }
}

/// The Play stage a game log line marks.
fn game_stage(text: &str) -> Option<&'static str> {
    if text.contains("Loaded ") && text.contains(" asset files from ") {
        Some(ASSETS)
    } else if text.contains("PIE frames: windowless streaming") {
        Some(STREAMING)
    } else {
        None
    }
}

/// Where a Play stage's time comes from.
fn stage_source(stage: &str) -> &'static str {
    match stage {
        LAUNCHED | CONNECTED => "editor log or status",
        ASKED | SENT => "editor log",
        ASSETS | STREAMING => "game log timestamp",
        SPAWNED => "first game query that found the last node",
        FIRST_FRAME => "status game_frame",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_lines_mark_the_play_stages() {
        let line = |message: &str| {
            format!("2026-11-10T18:39:59.414769Z  INFO jackdaw_editor_core::pie: PIE: {message}")
        };
        assert_eq!(
            editor_stage(&line("Play#1 launched, awaiting connection")),
            Some(LAUNCHED)
        );
        assert_eq!(editor_stage(&line("Play#1 connected")), Some(CONNECTED));
        assert_eq!(
            editor_stage(&line("Play#1 asked for the scene large_scene.bsn")),
            Some(ASKED)
        );
        assert_eq!(
            editor_stage(&line("Play#1 was sent the scene it asked for")),
            Some(SENT)
        );
        assert_eq!(editor_stage(&line("Play#1 exited")), None);
        assert_eq!(editor_stage("connected"), None);
    }

    #[test]
    fn a_stage_keeps_the_earliest_time_any_source_saw_it() {
        let mut stages = Vec::new();
        note_stage(&mut stages, CONNECTED, 2500.0);
        note_stage(&mut stages, CONNECTED, 2330.0);
        note_stage(&mut stages, CONNECTED, 2600.0);
        note_stage(&mut stages, LAUNCHED, 1760.0);
        assert_eq!(stages, vec![(CONNECTED, 2330.0), (LAUNCHED, 1760.0)]);
    }

    #[test]
    fn game_lines_mark_assets_and_streaming() {
        assert_eq!(
            game_stage(
                "2026-11-10T18:40:41Z  INFO jackdaw_runtime: Loaded 43 asset files from assets"
            ),
            Some(ASSETS)
        );
        assert_eq!(
            game_stage(
                "INFO jackdaw_runtime::pie_frames: PIE frames: windowless streaming at 954x540"
            ),
            Some(STREAMING)
        );
        assert_eq!(game_stage("PIE frames: stream stopped"), None);
    }

    #[test]
    fn options_need_a_project_and_reject_unknown_flags() {
        let args = |list: &[&str]| list.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
        assert!(Options::parse(&args(&[])).is_err());
        assert!(Options::parse(&args(&["--project"])).is_err());
        assert!(Options::parse(&args(&["--frobnicate"])).is_err());
        assert!(Options::parse(&args(&["--project", ".", "--play-timeout", "soon"])).is_err());
    }
}
