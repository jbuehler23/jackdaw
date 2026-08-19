//! Play-In-Editor runtime.
//!
//! Builds the project with plain `cargo build`, launches the game's own
//! executable as a child process, and drives it over an `ipc-channel`
//! connection. Children stream `StateEvent`s back and respond to
//! `ControlEvent`s (Pause / Resume / Stop). Stop reaps the children; the
//! authored scene is never mutated.
//!
//! Instances are keyed by [`InstanceKey`] (config label plus 1-based
//! instance number). All run configurations of a project share one
//! build, keyed by the project root: several instances wait on one build
//! and spawn together when it finishes.

use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use jackdaw_api::pie::PlayState;
use jackdaw_api::prelude::*;
use jackdaw_pie_protocol::event::{from_bytes, to_bytes};
use jackdaw_pie_protocol::manifest::RunConfig;
use jackdaw_pie_protocol::{
    ControlEvent, IpcChannelTransport, PieChannel, PieTransport, StateEvent, serve,
};

use crate::build_status::BuildStatus;
use crate::ext_build::BuildProgress;
use crate::pie_mirror::{PieInstances, PieViewMode};
use crate::project_build::shim::ShimSpec;
use crate::run_config::RunConfigs;

/// How many trailing stderr lines to keep from a game process, so a
/// crash can be reported without buffering unbounded output.
const STDERR_TAIL_LINES: usize = 40;

/// How long to wait for a launched child to connect back before giving
/// up on it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Marker for the toolbar transport buttons. `PiePlugin` installs
/// an `On<Add, PieButton>` observer that wires each button's
/// `Pointer<Click>` to the corresponding handler.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieButton {
    Play,
    Pause,
    Stop,
    Reload,
    /// Rebuild the project so new or changed game code appears in the
    /// editor. Distinct from `Reload` (which rebuilds and relaunches the
    /// running game): `Rebuild` is the edit-mode refresh of the component
    /// list, and dispatches `project.build`.
    Rebuild,
}

/// Identifies one running instance: a config label plus its 1-based
/// instance number.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct InstanceKey {
    pub config: String,
    pub instance: u32,
}

impl std::fmt::Display for InstanceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} #{}", self.config, self.instance)
    }
}

/// Rolling buffer of a child's most recent stderr lines, filled by the
/// per-child reader thread and read back when reporting a crash.
type StderrTail = Arc<Mutex<VecDeque<String>>>;

/// One launched child and its connection progress.
enum ChildStage {
    /// The child is launched and `IpcServerHandle::accept` is blocking
    /// on a task pool, waiting for it to connect. `since` bounds that
    /// wait so a child that never connects does not hang forever.
    Connecting {
        child: Child,
        accept: Task<io::Result<IpcChannelTransport>>,
        stderr_tail: StderrTail,
        since: Instant,
    },
    /// The child is connected and running; its transport is held here.
    Live {
        child: Child,
        transport: IpcChannelTransport,
        stderr_tail: StderrTail,
    },
}

/// The product of a completed game build: the executable Play runs. The
/// extracted type schema is persisted to `.jackdaw/schema.json` by the
/// build itself and picked up by the editor's watcher, so it does not
/// travel back through here.
struct GameBuildResult {
    binary: PathBuf,
}

/// An in-flight or finished build, deduped by project root. Instances
/// waiting on it are spawned when it finishes.
enum BuildState {
    /// `cargo build` is running on a task pool; `pending` lists the
    /// instances to spawn once the binary is ready. `progress` is the
    /// sink cargo writes compile progress into, surfaced in the footer.
    Running {
        task: Task<io::Result<GameBuildResult>>,
        pending: Vec<PendingSpawn>,
        progress: Arc<Mutex<BuildProgress>>,
    },
    /// The binary is built and cached at this path; later instances of
    /// the same spec spawn from it without rebuilding.
    Done(PathBuf),
    /// The build failed; its pending instances were dropped.
    Failed,
}

/// One instance waiting for its build to finish before spawning.
struct PendingSpawn {
    key: InstanceKey,
    run: RunConfig,
}

/// Editor-side play orchestration. `NonSend` because ipc transports
/// are `Send` but not `Sync`.
#[derive(Default)]
pub struct PieSession {
    children: HashMap<InstanceKey, ChildStage>,
    builds: HashMap<PathBuf, BuildState>,
}

impl Drop for PieSession {
    /// A clean editor shutdown (window close) drops the `World` and so this
    /// resource; take the running games down with it. Hard kills that skip
    /// destructors (Ctrl+C calls `process::exit`, SIGKILL skips everything) are
    /// covered by the `PR_SET_PDEATHSIG` hook set on each child at spawn.
    fn drop(&mut self) {
        for (_key, mut stage) in self.children.drain() {
            match &mut stage {
                ChildStage::Connecting { child, .. } | ChildStage::Live { child, .. } => {
                    child.kill().ok();
                    child.wait().ok();
                }
            }
        }
    }
}

impl PieSession {
    /// Whether an instance is currently launched (connecting or live).
    pub fn is_running(&self, key: &InstanceKey) -> bool {
        self.children.contains_key(key)
    }

    /// Keys of all launched instances, for the dropdown checks.
    pub fn running_keys(&self) -> impl Iterator<Item = &InstanceKey> {
        self.children.keys()
    }

    /// Whether an instance is queued behind an in-flight build but not
    /// yet spawned. Guards against double-launching during the build
    /// window, which would strand the first child.
    fn is_pending(&self, key: &InstanceKey) -> bool {
        self.builds.values().any(|build| {
            matches!(build, BuildState::Running { pending, .. }
                if pending.iter().any(|p| p.key == *key))
        })
    }
}

pub struct PiePlugin;

impl Plugin for PiePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<PlayState>()
            .init_non_send::<PieSession>()
            .init_resource::<PieViewMode>()
            .init_resource::<PieInstances>()
            .init_resource::<crate::pie_projection::PieProjection>()
            .init_resource::<crate::live_frame::LiveFrameStream>()
            .init_resource::<crate::live_edits::LiveEditLog>()
            .init_resource::<crate::live_highlight::LastHighlight>()
            .init_resource::<PieWindowMode>()
            .init_resource::<PiePrebuildState>()
            .init_resource::<crate::project_types::ProjectTypes>()
            .init_resource::<ProjectSchemaWatch>()
            .init_resource::<EditorBuildSettings>()
            // PIE is an editor-only subsystem; gate it to the editor state so it
            // never ticks in the launcher (`ProjectSelect`). Without this,
            // `reconcile_build_status` resets the shared `BuildStatus` from
            // `Building` to `Idle` every frame during the launcher's editor
            // build, so the build's completion handler drops the result and the
            // handoff never fires.
            .add_systems(
                Update,
                (
                    advance_pie_session,
                    prebuild_play_target,
                    sync_project_build_settings,
                    watch_project_source_for_edit,
                    watch_project_schema,
                    drain_game_events,
                    crate::live_highlight::sync_selection_highlight,
                )
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_systems(
                OnEnter(PlayState::Stopped),
                (
                    reset_view_mode_on_stop,
                    cleanup_session_on_stop,
                    crate::live_frame::clear_stream,
                    crate::live_edits_ui::open_stop_prompt_if_dirty,
                ),
            )
            .add_systems(
                OnExit(PlayState::Stopped),
                crate::live_edits_ui::review_handoff_on_replay,
            )
            .add_observer(wire_pie_button);
    }
}

/// Reset the outliner/inspector view back to Scene when play stops.
fn reset_view_mode_on_stop(mut mode: ResMut<PieViewMode>) {
    *mode = PieViewMode::Scene;
}

/// Shared cleanup for every path into `Stopped`: the stop button, a crashed
/// or self-exited child being reaped, and the last per-instance stop. Drops
/// the buffered snapshots and focus, then reverts the preview to the authored
/// scene. Without this on the reap path, ghost previews and a dead focus key
/// survive a crash, and the dead key blocks the next session's auto-enter
/// (focus is only established when none exists).
fn cleanup_session_on_stop(world: &mut World) {
    if let Some(mut instances) = world.get_resource_mut::<PieInstances>() {
        instances.buffers.clear();
        instances.focused = None;
    }
    crate::pie_projection::revert_preview(world);
}

/// Switch the outliner/inspector view to Live and project the focused
/// instance's buffered state into the preview world.
///
/// Shared by the Scene/Live toggle and the auto-enter path that fires the
/// moment a running instance first streams data. A no-op for the projection
/// when no instance is focused yet. [`reset_view_mode_on_stop`] owns the
/// revert when play stops.
pub(crate) fn enter_live_view(world: &mut World) {
    *world.resource_mut::<PieViewMode>() = PieViewMode::Live;
    crate::pie_projection::reproject_focused(world);
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<PiePlayOp>()
        .register_operator::<PiePauseOp>()
        .register_operator::<PieStopOp>()
        .register_operator::<PieReloadOp>()
        .register_operator::<ProjectBuildOp>()
        .register_operator::<ProjectToggleAutoBuildOp>()
        .register_operator::<PieWindowModeToggleOp>()
        .register_operator::<crate::live_input::PiePlayInputToggleOp>()
        .register_operator::<crate::live_edits::PieLiveEditSaveOp>()
        .register_operator::<crate::live_edits::PieLiveEditRevertOp>()
        .register_operator::<crate::live_edits::PieLiveEditsApplyAllOp>()
        .register_operator::<crate::live_edits::PieLiveEditsDiscardAllOp>();
}

fn play_is_stopped_or_paused(state: Res<State<PlayState>>) -> bool {
    !matches!(state.get(), PlayState::Playing)
}

fn play_is_playing(state: Res<State<PlayState>>) -> bool {
    *state.get() == PlayState::Playing
}

fn play_is_running(state: Res<State<PlayState>>) -> bool {
    *state.get() != PlayState::Stopped
}

/// Start the game. From Stopped, builds the project's game binary and
/// launches it connected to the editor; from Paused, resumes.
#[operator(
    id = "pie.play",
    label = "Play",
    description = "Start the game running in the editor.",
    is_available = play_is_stopped_or_paused
)]
pub(crate) fn pie_play(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(handle_play);
    OperatorResult::Finished
}

/// Pause the running game.
#[operator(
    id = "pie.pause",
    label = "Pause",
    description = "Pause the running game.",
    is_available = play_is_playing
)]
pub(crate) fn pie_pause(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(handle_pause);
    OperatorResult::Finished
}

/// Stop the running game and return to authoring mode.
#[operator(
    id = "pie.stop",
    label = "Stop",
    description = "Stop the running game.",
    is_available = play_is_running
)]
pub(crate) fn pie_stop(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(handle_stop);
    OperatorResult::Finished
}

/// Rebuild and relaunch the running game.
#[operator(
    id = "pie.reload",
    label = "Reload",
    description = "Rebuild and relaunch the running game.",
    is_available = play_is_running
)]
pub(crate) fn pie_reload(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(handle_reload);
    OperatorResult::Finished
}

/// Rebuild the game now so new or changed game code (added
/// components, edited fields) is picked up by the editor. This is the
/// manual counterpart to auto-build: it runs the same pipeline as an
/// out-of-editor `jackdaw build`, persisting the schema the editor then
/// reloads.
#[operator(
    id = "project.build",
    label = "Rebuild Project",
    description = "Rebuild the project so new or changed components appear in the editor."
)]
pub(crate) fn project_build(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        if let Some(root) = project_root(world) {
            spawn_project_build(world, &root);
        }
    });
    OperatorResult::Finished
}

/// Turn automatic rebuild-on-source-change on or off. Off by default so
/// the editor never builds behind the user's back; enabling it gives
/// live pickup for those who want it.
#[operator(
    id = "project.toggle_auto_build",
    label = "Toggle Auto Build",
    description = "Turn automatic rebuild-on-source-change on or off."
)]
pub(crate) fn project_toggle_auto_build(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let enabled = {
            let mut settings = world.resource_mut::<EditorBuildSettings>();
            settings.auto_build = !settings.auto_build;
            settings.auto_build
        };
        persist_project_build_settings(world);
        info!(
            "PIE: auto-build {}",
            if enabled { "enabled" } else { "disabled" }
        );
    });
    OperatorResult::Finished
}

/// Input capture needs a focused instance and a fresh frame to forward to;
/// it no longer depends on the outliner view mode.
pub(crate) fn focused_with_fresh_stream(
    stream: Res<crate::live_frame::LiveFrameStream>,
    instances: Res<PieInstances>,
) -> bool {
    instances.focused.is_some() && stream.is_fresh()
}

/// Where a launched game's window lives. Applies to the next launch.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PieWindowMode {
    /// No OS window: the game streams its frames into the editor's Game panel.
    #[default]
    Embedded,
    /// The game opens its own OS window (debug fallback; no input capture).
    Windowed,
}

/// Whether a launch under this mode sets `JACKDAW_PIE_WINDOWLESS`.
fn wants_windowless_env(mode: PieWindowMode) -> bool {
    mode == PieWindowMode::Embedded
}

fn toggle_window_mode(mode: &mut PieWindowMode) {
    *mode = match *mode {
        PieWindowMode::Embedded => PieWindowMode::Windowed,
        PieWindowMode::Windowed => PieWindowMode::Embedded,
    };
}

/// Switch the next launch between an embedded game (streamed into the Game
/// panel) and a separate game window.
#[operator(
    id = "pie.window_mode_toggle",
    label = "Toggle Game Window Mode",
    description = "Switch the next launch between embedded (Game panel) and a separate game window."
)]
pub(crate) fn pie_window_mode_toggle(
    _: In<OperatorParameters>,
    mut mode: ResMut<PieWindowMode>,
) -> OperatorResult {
    toggle_window_mode(&mut mode);
    OperatorResult::Finished
}

/// Stop every running instance, drop the cached build path so the next
/// launch re-runs the (incremental) cargo build, then relaunch each
/// instance that was running. The game reloads its scene from disk.
pub fn handle_reload(world: &mut World) {
    let keys: Vec<InstanceKey> = world
        .non_send::<PieSession>()
        .children
        .keys()
        .cloned()
        .collect();

    if keys.is_empty() {
        return;
    }

    let Some(run_configs) = world
        .get_resource::<RunConfigs>()
        .map(|c| c.manifest.clone())
    else {
        warn!("PIE: Reload but run configurations are not loaded");
        return;
    };

    handle_stop(world);

    for key in keys {
        let Some(run) = run_configs.run_by_name(&key.config).cloned() else {
            warn!("PIE: Reload could not find run config '{}'", key.config);
            continue;
        };
        launch_instance(world, key, run);
    }

    info!("PIE: Reload (rebuild + relaunch)");
}

/// Spawn a click observer on each `PieButton` as it's added. The
/// observer dispatches the corresponding `pie.*` operator.
fn wire_pie_button(
    trigger: On<Add, PieButton>,
    buttons: Query<&PieButton>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let Ok(kind) = buttons.get(entity).copied() else {
        return;
    };
    let op_id = match kind {
        PieButton::Play => PiePlayOp::ID,
        PieButton::Pause => PiePauseOp::ID,
        PieButton::Stop => PieStopOp::ID,
        PieButton::Reload => PieReloadOp::ID,
        PieButton::Rebuild => ProjectBuildOp::ID,
    };
    commands
        .entity(entity)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands
                .operator(op_id)
                .settings(CallOperatorSettings {
                    execution_context: ExecutionContext::Invoke,
                    creates_history_entry: false,
                })
                .call();
        });
}

/// Resolve the open project's root directory, or log and bail if no
/// project is open (Play has nothing to build without one).
fn project_root(world: &World) -> Option<PathBuf> {
    match world.get_resource::<crate::project::ProjectRoot>() {
        Some(project) => Some(project.root.clone()),
        None => {
            warn!("PIE: Play requested but no project is open");
            None
        }
    }
}

/// Launch one run-config instance. No-op if it is already running.
/// Resolves the build spec, then either spawns immediately from a
/// cached binary, joins an in-flight build's pending list, or starts a
/// new build keyed by the spec.
pub(crate) fn launch_instance(world: &mut World, key: InstanceKey, run: RunConfig) {
    {
        let session = world.non_send::<PieSession>();
        if session.is_running(&key) || session.is_pending(&key) {
            return;
        }
    }

    // Read root before borrowing the session mutably below.
    let Some(root) = project_root(world) else {
        return;
    };

    // If the game binary is built AND still current, spawn from it
    // immediately. A source edit since the last build invalidates the
    // cache so Play runs the new code: each Play is a fresh process, so
    // rebuilding is always safe.
    if let Some(BuildState::Done(path)) = world.non_send::<PieSession>().builds.get(&root) {
        if source_is_current(&root, path) {
            let path = path.clone();
            if let Some(stage) = spawn_instance(world, &key, &run, &path) {
                world
                    .non_send_mut::<PieSession>()
                    .children
                    .insert(key, stage);
            }
            return;
        }
        info!("PIE: project source changed; rebuilding before Play");
        world.non_send_mut::<PieSession>().builds.remove(&root);
    }

    let Some(spec) = crate::project_build::shim_spec_for_project(&root).ok() else {
        warn!("PIE: no lib crate found in {}", root.display());
        return;
    };

    // Join an in-flight build's pending list, or start a new build.
    let mut session = world.non_send_mut::<PieSession>();
    match session.builds.get_mut(&root) {
        Some(BuildState::Running { pending, .. }) => {
            pending.push(PendingSpawn { key, run });
        }
        Some(BuildState::Done(_)) => unreachable!("handled above"),
        Some(BuildState::Failed) | None => {
            info!("PIE: building the game for {key}");
            let (task, progress) = spawn_game_build(&root, spec);
            session.builds.insert(
                root,
                BuildState::Running {
                    task,
                    pending: vec![PendingSpawn { key, run }],
                    progress,
                },
            );
        }
    }
}

/// Whether the built game binary is newer than the project's source. A
/// stale binary (source edited since the build) means Play must rebuild.
/// `Cargo.toml` and everything under `src/` are checked; a missing
/// binary counts as stale.
fn source_is_current(root: &Path, binary: &Path) -> bool {
    let Ok(binary_mtime) = std::fs::metadata(binary).and_then(|m| m.modified()) else {
        return false;
    };
    let newest_source = newest_mtime(&root.join("src"))
        .into_iter()
        .chain(
            std::fs::metadata(root.join("Cargo.toml"))
                .and_then(|m| m.modified())
                .ok(),
        )
        .max();
    match newest_source {
        Some(src_mtime) => src_mtime <= binary_mtime,
        None => true,
    }
}

/// The newest modification time under `dir`, recursively; `None` when
/// the directory is missing or empty.
fn newest_mtime(dir: &Path) -> Option<std::time::SystemTime> {
    let mut newest = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(path) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let entry_path = entry.path();
            if entry_path.is_dir() {
                stack.push(entry_path);
            } else if let Ok(mtime) = entry.metadata().and_then(|m| m.modified()) {
                newest = Some(newest.map_or(mtime, |n: std::time::SystemTime| n.max(mtime)));
            }
        }
    }
    newest
}

/// Start the project's game build on the compute pool, returning the
/// task and the progress sink the footer polls.
fn spawn_game_build(
    root: &Path,
    spec: ShimSpec,
) -> (Task<io::Result<GameBuildResult>>, Arc<Mutex<BuildProgress>>) {
    let progress = Arc::new(Mutex::new(BuildProgress::default()));
    let sink = Arc::clone(&progress);
    let jackdaw_dir = root.join(".jackdaw");
    let task = AsyncComputeTaskPool::get().spawn(async move {
        if let Ok(mut p) = sink.lock() {
            p.push_log("building the game".to_string());
        }
        // Stream live progress into the sink the footer bar and build log
        // read: per-crate counts drive the bar, diagnostics fill the log.
        let report_sink = Arc::clone(&sink);
        let mut report = move |event: crate::project_build::BuildEvent| {
            if let Ok(mut p) = report_sink.lock() {
                match event {
                    crate::project_build::BuildEvent::Compiled {
                        crate_name,
                        done,
                        fresh,
                    } => {
                        p.artifacts_done = done;
                        // Cached units still count toward progress but should
                        // not read as compiling in the header or the log.
                        if !fresh {
                            p.current_crate = Some(crate_name.clone());
                            p.push_log(format!("Compiling {crate_name}"));
                        }
                    }
                    crate::project_build::BuildEvent::Log(line) => {
                        // Warnings about the SDK explain the compiler
                        // errors that follow, and a user watching the
                        // terminal rather than the build panel would
                        // otherwise see only the errors.
                        if line.starts_with("warning:") || line.contains("is not usable") {
                            warn!("{line}");
                        }
                        p.push_log(line);
                    }
                }
            }
        };
        let result = crate::project_build::build_project_binary(&spec, &jackdaw_dir, &mut report);
        match result {
            Ok(build) => {
                if let Ok(mut p) = sink.lock() {
                    let types = build
                        .schema
                        .as_ref()
                        .map(|s| s.components.len())
                        .unwrap_or(0);
                    p.push_log(format!("game ready ({types} types)"));
                }
                Ok(GameBuildResult {
                    binary: build.binary,
                })
            }
            Err(err) => {
                // Diagnostics already streamed into the log via the build
                // reporter, so just note the failure and surface the error.
                if let Ok(mut p) = sink.lock() {
                    p.push_log("build failed".to_string());
                }
                Err(io::Error::other(err.to_string()))
            }
        }
    });
    (task, progress)
}

/// Stop one instance: ask a live child to exit, reap it, and drop it.
/// Returns to authoring mode once no children remain.
pub(crate) fn stop_instance(world: &mut World, key: &InstanceKey) {
    let Some(mut stage) = world.non_send_mut::<PieSession>().children.remove(key) else {
        return;
    };
    match &mut stage {
        ChildStage::Live {
            child, transport, ..
        } => {
            send_control_to(transport, ControlEvent::Stop);
            child.kill().ok();
            child.wait().ok();
        }
        ChildStage::Connecting { child, .. } => {
            child.kill().ok();
            child.wait().ok();
        }
    }
    drop(stage);

    if world.non_send::<PieSession>().children.is_empty()
        && *world.resource::<State<PlayState>>().get() != PlayState::Stopped
    {
        world
            .resource_mut::<NextState<PlayState>>()
            .set(PlayState::Stopped);
    }
}

/// Begin play. From Stopped, launches the first run config's instance
/// (building it if needed). From Paused, resumes every live child. No-op
/// if already Playing or if the project has no run configurations.
pub fn handle_play(world: &mut World) {
    let current = world.resource::<State<PlayState>>().get().clone();
    match current {
        PlayState::Stopped => {
            let Some(run_configs) = world.get_resource::<RunConfigs>() else {
                warn!("PIE: run configurations not loaded");
                return;
            };
            let runs = run_configs.manifest.runs.clone();
            let Some(first) = runs.into_iter().next() else {
                warn!("PIE: no run configurations");
                return;
            };
            let key = InstanceKey {
                config: first.label().to_string(),
                instance: 1,
            };
            launch_instance(world, key, first);
        }
        PlayState::Paused => {
            broadcast_control(world, ControlEvent::Resume);
            world
                .resource_mut::<NextState<PlayState>>()
                .set(PlayState::Playing);
            info!("PIE: Play (resumed)");
        }
        PlayState::Playing => {}
    }
}

/// Transition `Playing` -> `Paused`, telling every live child to freeze.
/// No-op otherwise.
pub fn handle_pause(world: &mut World) {
    if *world.resource::<State<PlayState>>().get() == PlayState::Playing {
        broadcast_control(world, ControlEvent::Pause);
        world
            .resource_mut::<NextState<PlayState>>()
            .set(PlayState::Paused);
        info!("PIE: Pause");
    }
}

/// Stop every instance: ask the live children to exit, then reap and
/// drop them all and discard pending builds. Returns to authoring mode.
pub fn handle_stop(world: &mut World) {
    let current = world.resource::<State<PlayState>>().get().clone();

    broadcast_control(world, ControlEvent::Stop);

    let mut session = world.non_send_mut::<PieSession>();
    for (_key, mut stage) in session.children.drain() {
        match &mut stage {
            ChildStage::Connecting { child, .. } | ChildStage::Live { child, .. } => {
                child.kill().ok();
                child.wait().ok();
            }
        }
    }
    // Dropping the in-flight build tasks aborts them.
    session.builds.clear();

    if current != PlayState::Stopped {
        world
            .resource_mut::<NextState<PlayState>>()
            .set(PlayState::Stopped);
        info!("PIE: Stop");
    }
}

/// Encode and send a single control message on the reliable channel to
/// one child's transport. An encode failure logs and skips.
fn send_control_to(transport: &mut IpcChannelTransport, event: ControlEvent) {
    let bytes = match to_bytes(&event) {
        Ok(bytes) => bytes,
        Err(err) => {
            error!("PIE: failed to encode {event:?}: {err}");
            return;
        }
    };
    transport.send(PieChannel::Reliable, &bytes);
}

/// Send a control message to every live child. Connecting children
/// (not yet holding a transport) are skipped.
fn broadcast_control(world: &mut World, event: ControlEvent) {
    let mut session = world.non_send_mut::<PieSession>();
    for stage in session.children.values_mut() {
        if let ChildStage::Live { transport, .. } = stage {
            send_control_to(transport, event.clone());
        }
    }
}

/// Send a control message (live edits, frame stream start/stop) only to the
/// focused instance's child transport. If no instance is focused, or the
/// focused child is not yet live, the message is dropped with a debug log.
/// Safe no-op when `PieInstances` or `PieSession` are absent (headless worlds).
pub(crate) fn send_control_to_focused(world: &mut World, event: ControlEvent) {
    let focused = world
        .get_resource::<PieInstances>()
        .and_then(|instances| instances.focused.clone());
    let Some(key) = focused else {
        debug!("PIE: control dropped. No focused instance.");
        return;
    };
    let Some(mut session) = world.get_non_send_mut::<PieSession>() else {
        debug!("PIE: control dropped. PieSession not present.");
        return;
    };
    if let Some(ChildStage::Live { transport, .. }) = session.children.get_mut(&key) {
        send_control_to(transport, event);
    } else {
        debug!("PIE: control dropped. Focused instance {key} is not live.");
    }
}

/// The focused instance's key, but only while its child process is connected
/// and live. `None` when nothing is focused, the child is still connecting,
/// or it has already been reaped.
pub(crate) fn focused_live_instance(world: &World) -> Option<InstanceKey> {
    let focused = world.get_resource::<PieInstances>()?.focused.clone()?;
    let session = world.get_non_send::<PieSession>()?;
    match session.children.get(&focused) {
        Some(ChildStage::Live { .. }) => Some(focused),
        _ => None,
    }
}

/// Whether the Live "Save to Scene" action can run right now: the inspector
/// is in Live mode and a preview entity is selected (either an authored
/// entity that maps back to an AST node, or an ephemeral runtime entity
/// that would be promoted to a new node).
pub(crate) fn can_save_live_to_scene(world: &World) -> bool {
    if *world.resource::<PieViewMode>() != PieViewMode::Live {
        return false;
    }
    let Some(preview) = world.resource::<crate::selection::Selection>().primary() else {
        return false;
    };
    // Only act on entities that are part of the current live projection.
    let in_projection = world
        .resource::<crate::pie_projection::PieProjection>()
        .by_bits
        .values()
        .any(|&e| e == preview);
    if !in_projection {
        return false;
    }
    // Ephemeral entities can be promoted to new authored nodes (Path B).
    if world
        .get::<crate::pie_projection::PieEphemeral>(preview)
        .is_some()
    {
        return true;
    }
    // Authored entities are saveable when the AST still holds their node (Path A).
    world
        .resource::<jackdaw_bsn::SceneBsnAst>()
        .ast_for(preview)
        .is_some()
}

/// Collect all non-skipped reflected components from `entity` as
/// `(type_path, BsnValue)` pairs, using the same filter that
/// `register_entity_in_ast` uses when it first captures a live entity.
///
/// Components with no `ReflectComponent` data, types not registered in the
/// `AppTypeRegistry`, and paths matched by [`should_skip_component`](crate::scene_io::should_skip_component) are
/// silently omitted. The result is sorted by type path for deterministic undo
/// batching.
fn preview_entity_components_as_bsn(
    world: &World,
    entity: Entity,
) -> Vec<(String, jackdaw_bsn::BsnValue)> {
    use bevy::ecs::reflect::AppTypeRegistry;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();

    let Ok(entity_ref) = world.get_entity(entity) else {
        return Vec::new();
    };

    // The same structural/derived components that register_entity_in_ast skips.
    let skip_ids = crate::scene_io::structural_skip_type_ids();

    let mut out: Vec<(String, jackdaw_bsn::BsnValue)> = registry
        .iter()
        .filter(|reg| !skip_ids.contains(&reg.type_id()))
        .filter_map(|reg| {
            let type_path = reg.type_info().type_path_table().path();
            if crate::scene_io::should_skip_component(type_path) {
                return None;
            }
            let reflect_component = reg.data::<ReflectComponent>()?;
            let component = reflect_component.reflect(entity_ref)?;
            let value =
                jackdaw_bsn::BsnValue::from_reflect(component.as_partial_reflect(), &registry);
            Some((type_path.to_string(), value))
        })
        .collect();

    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Promote the selected preview entity's current component values into the
/// authored scene (Path A) or create a new authored node from it (Path B).
///
/// Path A: the preview entity is already bound to an AST node (it is an
/// authored entity with a live overlay). Each non-skipped reflected component
/// is read from the preview entity, serialized, and written into the node
/// through a [`SetBsnField`](crate::commands::SetBsnField) command. The commands are grouped into one
/// undoable [`CommandGroup`](jackdaw_commands::CommandGroup) so a single Ctrl+Z reverts the whole promote.
///
/// Path B: the preview entity carries [`PieEphemeral`](crate::pie_projection::PieEphemeral) (the game spawned it
/// at runtime with no authored counterpart). A new node is registered in the
/// live BSN document, its patches filled from the preview entity's
/// reflected components and bound to this entity. The entity
/// receives a [`SceneNodeId`](jackdaw_scene_types::SceneNodeId) component and loses [`PieEphemeral`](crate::pie_projection::PieEphemeral) so it is
/// now treated as an authored entity. Path B is not undoable in v1 (no
/// remove-node command exists that mirrors the insert).
///
/// No-op with a `warn!` when nothing is selected, the preview entity is gone,
/// or the entity carries neither a node binding nor `PieEphemeral`.
pub(crate) fn save_live_entity_to_scene(world: &mut World) {
    use crate::pie_projection::PieEphemeral;

    if *world.resource::<PieViewMode>() != PieViewMode::Live {
        return;
    }
    let Some(preview) = world.resource::<crate::selection::Selection>().primary() else {
        warn!("save to scene: no entity selected");
        return;
    };

    // Resolve the game-side bits for this preview entity from the projection map.
    let bits = world
        .resource::<crate::pie_projection::PieProjection>()
        .by_bits
        .iter()
        .find_map(|(&b, &e)| if e == preview { Some(b) } else { None });
    let Some(bits) = bits else {
        warn!("save to scene: selected entity {preview:?} has no live projection entry");
        return;
    };

    if world.get_entity(preview).is_err() {
        warn!("save to scene: preview entity for live {bits:x} no longer exists");
        return;
    }

    let is_ephemeral = world.get::<PieEphemeral>(preview).is_some();

    if is_ephemeral {
        promote_ephemeral_to_authored(world, preview, bits);
    } else {
        promote_authored_overlay(world, preview, bits);
    }
}

/// Path A: preview entity is bound to an existing AST node. Capture its
/// current component values and write them through `SetBsnField` commands.
fn promote_authored_overlay(world: &mut World, preview: Entity, bits: u64) {
    use crate::commands::{CommandGroup, CommandHistory, EditorCommand, SetBsnField};

    let node_id = {
        let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
        let Some(node) = ast.ast_for(preview) else {
            warn!("save to scene: preview entity {preview:?} (bits {bits:x}) has no AST node");
            return;
        };
        ast.stable_id_of(node).unwrap_or(0)
    };

    let entries = preview_entity_components_as_bsn(world, preview);

    let mut sub_commands: Vec<Box<dyn EditorCommand>> = Vec::new();
    for (type_path, new_value) in entries {
        let old_value = {
            let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
            ast.ast_for(preview)
                .and_then(|node| jackdaw_bsn::get_bsn_field(ast, node, &type_path, ""))
        };
        sub_commands.push(Box::new(SetBsnField {
            entity: preview,
            type_path,
            field_path: String::new(),
            old_value,
            new_value,
            was_derived: false,
        }));
    }

    let count = sub_commands.len();
    let mut cmd: Box<dyn EditorCommand> = if count == 0 {
        warn!("save to scene: preview entity {preview:?} had no saveable components");
        return;
    } else if count == 1 {
        match sub_commands.into_iter().next() {
            Some(only) => only,
            None => return,
        }
    } else {
        Box::new(CommandGroup {
            label: "Save runtime values to scene".to_string(),
            commands: sub_commands,
        })
    };
    cmd.execute(world);
    world.resource_mut::<CommandHistory>().push_executed(cmd);
    info!("save to scene: promoted runtime values into node {node_id}");
}

/// Path B: preview entity has no AST node (game-spawned runtime entity).
/// Mint a stable node id for it, drop the ephemeral marker, and register it
/// in the live BSN document, which captures its reflected components as a
/// new authored node. Not undoable in v1.
pub(crate) fn promote_ephemeral_to_authored(world: &mut World, preview: Entity, bits: u64) {
    use crate::pie_projection::PieEphemeral;

    let node_id = jackdaw_scene_types::SceneNodeId::next();
    world.entity_mut(preview).remove::<PieEphemeral>();
    world.entity_mut(preview).insert(node_id);
    crate::scene_io::register_entity_in_ast(world, preview);

    info!(
        "save to scene: promoted ephemeral {preview:?} (bits {bits:x}) to new authored node {}",
        node_id.0
    );
}

/// Drive every instance forward each frame: advance finished builds
/// into spawned children, move connecting children to live once they
/// accept, and reap exited children. Reconciles `PlayState` to match
/// whether any child is live.
fn advance_pie_session(world: &mut World) {
    poll_builds(world);
    poll_children(world);
    reconcile_play_state(world);
    reconcile_build_status(world);
}

/// One-shot flag so the background pre-build is attempted only once per editor
/// session, without re-running `cargo metadata` every frame.
#[derive(Resource, Default)]
pub struct PiePrebuildState {
    attempted: bool,
}

/// Pre-build the game in the background once the editor opens, so the
/// first Play reuses a warm artifact instead of compiling on demand.
/// Mirrors `launch_instance`'s build but with no pending spawn: it only
/// drives `PieSession.builds` to `Done`, which a later Play reuses
/// immediately.
fn prebuild_play_target(world: &mut World) {
    if world.resource::<PiePrebuildState>().attempted {
        return;
    }
    // Wait until the project's run configs have loaded.
    if world.get_resource::<RunConfigs>().is_none() {
        return;
    }
    // Run configs are loaded; this is the one-shot attempt regardless of outcome.
    world.resource_mut::<PiePrebuildState>().attempted = true;
    let Some(root) = project_root(world) else {
        return;
    };
    let spec = match crate::project_build::shim_spec_for_project(&root) {
        Ok(spec) => spec,
        Err(error) => {
            // Without a resolvable package there is nothing to compile,
            // so the editor would otherwise open with an empty Add
            // Component list and no explanation anywhere.
            let reason = error.to_string();
            warn!("PIE: cannot build {}: {reason}", root.display());
            if let Some(mut status) = world.get_resource_mut::<BuildStatus>() {
                status.state = crate::build_status::BuildState::Blocked { reason };
            }
            return;
        }
    };
    let mut session = world.non_send_mut::<PieSession>();
    if session.builds.contains_key(&root) {
        return; // already built or building (the user may have hit Play already)
    }
    info!("PIE: pre-building the game in the background for a fast first Play");
    let (task, progress) = spawn_game_build(&root, spec);
    session.builds.insert(
        root,
        BuildState::Running {
            task,
            pending: Vec::new(),
            progress,
        },
    );
}

/// Throttle so the edit-mode source scan runs about once a second
/// rather than every frame.
#[derive(Resource)]
struct EditRebuildThrottle(Timer);

impl Default for EditRebuildThrottle {
    fn default() -> Self {
        Self(Timer::from_seconds(1.0, TimerMode::Repeating))
    }
}

/// Editor-wide project build preferences.
#[derive(Resource, Default, serde::Serialize, serde::Deserialize)]
struct EditorBuildSettings {
    /// When true, the editor rebuilds the project automatically on source
    /// change. Opt-in and off by default: otherwise a rebuild fires
    /// behind the user's back and competes with a `cargo build` they may
    /// have started themselves. When off, rebuilds come from the Rebuild
    /// action or an out-of-editor `jackdaw build`.
    auto_build: bool,
}

fn sync_project_build_settings(
    project: Option<Res<crate::project::ProjectRoot>>,
    mut settings: ResMut<EditorBuildSettings>,
    mut loaded_root: Local<Option<PathBuf>>,
) {
    let Some(project) = project else {
        return;
    };
    if loaded_root.as_ref() == Some(&project.root) {
        return;
    }
    *loaded_root = Some(project.root.clone());
    let path = project.root.join(".jackdaw/settings.json");
    *settings = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
}

fn persist_project_build_settings(world: &World) {
    let (Some(project), Some(settings)) = (
        world.get_resource::<crate::project::ProjectRoot>(),
        world.get_resource::<EditorBuildSettings>(),
    ) else {
        return;
    };
    let path = project.root.join(".jackdaw/settings.json");
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        warn!("PIE: could not persist build settings: {error}");
        return;
    }
    match serde_json::to_vec_pretty(settings) {
        Ok(bytes) => {
            if let Err(error) = std::fs::write(path, bytes) {
                warn!("PIE: could not persist build settings: {error}");
            }
        }
        Err(error) => warn!("PIE: could not encode build settings: {error}"),
    }
}

/// Start a game build for `root` unless one is already in flight.
/// Shared by the source watcher (auto-build) and the manual Rebuild
/// operator. The finished build persists `.jackdaw/schema.json`, which
/// [`watch_project_schema`] picks up to refresh the editor's types.
fn spawn_project_build(world: &mut World, root: &Path) {
    if matches!(
        world.non_send::<PieSession>().builds.get(root),
        Some(BuildState::Running { .. })
    ) {
        return;
    }
    let Some(spec) = crate::project_build::shim_spec_for_project(root).ok() else {
        return;
    };
    info!("PIE: building the game");
    let (task, progress) = spawn_game_build(root, spec);
    world.non_send_mut::<PieSession>().builds.insert(
        root.to_path_buf(),
        BuildState::Running {
            task,
            pending: Vec::new(),
            progress,
        },
    );
}

/// While editing (not playing) and only when auto-build is enabled,
/// rebuild the game when its source changes so a newly added or
/// edited component appears in the editor without a restart. The
/// completed build persists the schema, which `watch_project_schema`
/// loads. Off by default; the manual Rebuild action is the normal path.
fn watch_project_source_for_edit(world: &mut World) {
    if *world.resource::<State<PlayState>>().get() != PlayState::Stopped {
        return;
    }
    if !world.resource::<EditorBuildSettings>().auto_build {
        return;
    }
    {
        let delta = world.resource::<Time>().delta();
        let mut throttle = world.get_resource_or_insert_with(EditRebuildThrottle::default);
        if !throttle.0.tick(delta).just_finished() {
            return;
        }
    }
    let Some(root) = project_root(world) else {
        return;
    };
    // Only rebuild once the project has been built and loaded at least
    // once; the initial build is driven by `prebuild_play_target`.
    let built_path = match world.non_send::<PieSession>().builds.get(&root) {
        Some(BuildState::Done(path)) => path.clone(),
        _ => return,
    };
    if source_is_current(&root, &built_path) {
        return;
    }
    spawn_project_build(world, &root);
}

/// Throttle for the schema-file poll so it stats the file about twice a
/// second rather than every frame.
#[derive(Resource)]
struct ProjectSchemaWatch {
    last_mtime: Option<std::time::SystemTime>,
    throttle: Timer,
}

impl Default for ProjectSchemaWatch {
    fn default() -> Self {
        Self {
            last_mtime: None,
            throttle: Timer::from_seconds(0.5, TimerMode::Repeating),
        }
    }
}

/// Refresh the editor's known project component types from
/// `<project>/.jackdaw/schema.json` whenever that file changes. This is
/// the single pickup path for project types: the editor's own build and
/// an out-of-editor `jackdaw build` both write the file, and only this
/// system reads it into `ProjectTypes`. On editor start it also loads a
/// schema left by a previous session, so components are known before any
/// rebuild.
fn watch_project_schema(world: &mut World) {
    {
        let delta = world.resource::<Time>().delta();
        let mut watch = world.resource_mut::<ProjectSchemaWatch>();
        if !watch.throttle.tick(delta).just_finished() {
            return;
        }
    }
    let Some(root) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.root.clone())
    else {
        return;
    };
    let jackdaw_dir = root.join(".jackdaw");
    let path = jackdaw_schema::schema_path(&jackdaw_dir);
    let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) else {
        return;
    };
    if world.resource::<ProjectSchemaWatch>().last_mtime == Some(mtime) {
        return;
    }
    let Some(schema) = jackdaw_schema::read_schema(&jackdaw_dir) else {
        return;
    };
    let native =
        crate::project_types::native_type_paths(&world.resource::<AppTypeRegistry>().read());
    let component_count = {
        let mut project_types = world.resource_mut::<crate::project_types::ProjectTypes>();
        project_types.update(&schema, &native);
        project_types.components().count()
    };
    world.resource_mut::<ProjectSchemaWatch>().last_mtime = Some(mtime);

    // Surface the outcome in the footer so the user knows the build
    // finished and how many of their components loaded.
    world
        .resource_mut::<crate::build_status::BuildStatus>()
        .state = crate::build_status::BuildState::Ready {
        at: Instant::now(),
        components: component_count,
    };

    // Repaint the open inspector: a shape change (a field added, removed,
    // or retyped in game code) must show without a reselect.
    let selected: Vec<Entity> = world
        .resource::<crate::selection::Selection>()
        .entities
        .clone();
    for entity in selected {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.insert(crate::inspector::InspectorDirty);
        }
    }
    info!("PIE: refreshed project types from schema ({component_count} project components)");
}

/// Mirror the active game build into the editor's `BuildStatus` so the
/// footer shows what is compiling, and clear it once no build remains.
fn reconcile_build_status(world: &mut World) {
    let building = world
        .non_send::<PieSession>()
        .builds
        .values()
        .find_map(|build| match build {
            BuildState::Running { progress, .. } => Some(Arc::clone(progress)),
            _ => None,
        });
    let project = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.root.clone())
        .unwrap_or_default();
    let Some(mut status) = world.get_resource_mut::<BuildStatus>() else {
        return;
    };
    match building {
        Some(progress) => {
            status.state = crate::build_status::BuildState::Building {
                project,
                started: Instant::now(),
                progress,
            };
        }
        None => match &status.state {
            // A build ended with no outcome latched yet: go Idle.
            // `poll_builds` (Failed) and `watch_project_schema` (Ready)
            // set the real outcome the same or next frame.
            crate::build_status::BuildState::Building { .. } => {
                status.state = crate::build_status::BuildState::Idle;
            }
            // Keep a recent outcome on screen, then let it fade to Idle.
            crate::build_status::BuildState::Ready { at, .. }
            | crate::build_status::BuildState::Failed { at } => {
                if at.elapsed() > Duration::from_secs(6) {
                    status.state = crate::build_status::BuildState::Idle;
                }
            }
            // Sticky: only the user fixing their project clears it.
            crate::build_status::BuildState::Blocked { .. }
            | crate::build_status::BuildState::Idle => {}
        },
    }
}

/// Poll each in-flight build. On success, spawn its pending instances
/// and mark the build `Done`; on failure mark it `Failed` and drop
/// pending instances. The builds map is taken with `mem::take` before
/// spawning so `spawn_instance` can read the world without aliasing.
fn poll_builds(world: &mut World) {
    let mut builds = std::mem::take(&mut world.non_send_mut::<PieSession>().builds);
    let mut spawned: Vec<(InstanceKey, ChildStage)> = Vec::new();

    for state in builds.values_mut() {
        let BuildState::Running { task, pending, .. } = state else {
            continue;
        };
        match future::block_on(future::poll_once(task)) {
            None => {}
            Some(Ok(result)) => {
                // The build persisted the project's type schema to
                // `.jackdaw/schema.json`; `watch_project_schema` picks it
                // up and refreshes `ProjectTypes`. Pickup is deliberately
                // decoupled from building so an out-of-editor `jackdaw
                // build` flows through the same path. The editor never
                // links or loads project code; it reads the schema the
                // game reported about itself, and Play runs that same
                // binary in a fresh process below.
                for spawn in pending.drain(..) {
                    if let Some(stage) =
                        spawn_instance(world, &spawn.key, &spawn.run, &result.binary)
                    {
                        spawned.push((spawn.key, stage));
                    }
                }
                *state = BuildState::Done(result.binary);
            }
            Some(Err(err)) => {
                let keys: Vec<String> = pending.iter().map(|p| p.key.to_string()).collect();
                error!("PIE: game build failed for {}: {err}", keys.join(", "));
                *state = BuildState::Failed;
                world
                    .resource_mut::<crate::build_status::BuildStatus>()
                    .state = crate::build_status::BuildState::Failed { at: Instant::now() };
            }
        }
    }

    let mut session = world.non_send_mut::<PieSession>();
    session.builds = builds;
    for (key, stage) in spawned {
        session.children.insert(key, stage);
    }
}

/// Poll each launched child. Connecting children that accept become
/// live; ones that fail or time out are reaped. Live children that
/// exit are reaped. The children map is taken with `mem::take` and
/// rebuilt from survivors, moving each `Child` by value.
fn poll_children(world: &mut World) {
    let children = std::mem::take(&mut world.non_send_mut::<PieSession>().children);

    let mut survivors: HashMap<InstanceKey, ChildStage> = HashMap::with_capacity(children.len());
    for (key, stage) in children {
        if let Some(next) = advance_child(&key, stage) {
            survivors.insert(key, next);
        }
    }

    world.non_send_mut::<PieSession>().children = survivors;
}

/// Step one child's stage by value, returning the stage it should
/// carry into next frame, or `None` if it should be dropped. A
/// connected `Connecting` becomes `Live`; a child that failed to
/// connect or has exited is reaped and dropped.
fn advance_child(key: &InstanceKey, stage: ChildStage) -> Option<ChildStage> {
    match stage {
        ChildStage::Connecting {
            mut child,
            mut accept,
            stderr_tail,
            since,
        } => {
            // A child that died before connecting will never accept;
            // reap it immediately and surface the crash.
            match child.try_wait() {
                Ok(Some(status)) => {
                    error!("PIE: {key} exited before connecting with {status}");
                    report_stderr_tail(&stderr_tail);
                    return None;
                }
                Ok(None) => {}
                Err(err) => {
                    error!("PIE: {key} failed to poll while connecting: {err}");
                    child.kill().ok();
                    child.wait().ok();
                    return None;
                }
            }
            match future::block_on(future::poll_once(&mut accept)) {
                None => {
                    if since.elapsed() >= CONNECT_TIMEOUT {
                        error!(
                            "PIE: {key} did not connect within {}s; is the jackdaw_runtime/pie feature enabled for this bin?",
                            CONNECT_TIMEOUT.as_secs()
                        );
                        child.kill().ok();
                        child.wait().ok();
                        report_stderr_tail(&stderr_tail);
                        None
                    } else {
                        Some(ChildStage::Connecting {
                            child,
                            accept,
                            stderr_tail,
                            since,
                        })
                    }
                }
                Some(Ok(transport)) => {
                    info!("PIE: {key} connected");
                    Some(ChildStage::Live {
                        child,
                        transport,
                        stderr_tail,
                    })
                }
                Some(Err(err)) => {
                    error!("PIE: {key} failed to connect: {err}");
                    child.kill().ok();
                    child.wait().ok();
                    report_stderr_tail(&stderr_tail);
                    None
                }
            }
        }
        ChildStage::Live {
            mut child,
            transport,
            stderr_tail,
        } => match child.try_wait() {
            Ok(None) => Some(ChildStage::Live {
                child,
                transport,
                stderr_tail,
            }),
            Ok(Some(status)) => {
                if status.success() {
                    info!("PIE: {key} exited");
                } else {
                    error!("PIE: {key} exited with {status}");
                    report_stderr_tail(&stderr_tail);
                }
                None
            }
            Err(err) => {
                error!("PIE: {key} failed to poll: {err}");
                None
            }
        },
    }
}

/// Reconcile `PlayState` with live children: any live child implies
/// `Playing` (unless already `Paused`); zero children implies `Stopped`.
fn reconcile_play_state(world: &mut World) {
    let session = world.non_send::<PieSession>();
    let any_live = session
        .children
        .values()
        .any(|stage| matches!(stage, ChildStage::Live { .. }));
    let child_count = session.children.len();
    let current = world.resource::<State<PlayState>>().get().clone();

    if any_live && !matches!(current, PlayState::Playing | PlayState::Paused) {
        world
            .resource_mut::<NextState<PlayState>>()
            .set(PlayState::Playing);
    } else if child_count == 0 && current != PlayState::Stopped {
        world
            .resource_mut::<NextState<PlayState>>()
            .set(PlayState::Stopped);
    }
}

#[cfg(target_os = "windows")]
mod pie_windows {
    use std::{
        os::{
            raw::c_void,
            windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        },
        process::Child,
        sync::LazyLock,
    };
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::JobObjects::*;
    use windows::core::PCWSTR;

    pub static PIE_JOB: LazyLock<Job> = LazyLock::new(|| unsafe {
        let handle = OwnedHandle::from_raw_handle(
            CreateJobObjectW(None, PCWSTR::null())
                .expect("Failed to create job object for PIE")
                .0,
        );

        let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                    | JOB_OBJECT_LIMIT_BREAKAWAY_OK
                    | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
                ..Default::default()
            },
            ..Default::default()
        };

        SetInformationJobObject(
            HANDLE(handle.as_raw_handle()),
            JobObjectExtendedLimitInformation,
            (&raw const info).cast::<c_void>(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
        .expect("Failed to set job object info for PIE");

        Job(handle)
    });

    pub struct Job(pub OwnedHandle);

    impl Job {
        pub fn assign_to_process(&self, process: &Child) {
            let handle = process.as_raw_handle();
            unsafe {
                AssignProcessToJobObject(HANDLE(self.0.as_raw_handle()), HANDLE(handle))
                    .expect("Failed to assign process to job for PIE");
            }
        }
    }
}

/// Launch the project's game binary for one instance, point it at a
/// fresh rendezvous, start draining its stderr, and begin awaiting its
/// connection on a task pool. Returns the `Connecting` stage; on
/// rendezvous or spawn failure logs and returns `None` so the caller
/// skips it.
fn spawn_instance(
    world: &World,
    key: &InstanceKey,
    run: &RunConfig,
    binary: &Path,
) -> Option<ChildStage> {
    let root = project_root(world)?;

    let (handle, server_name) = match serve() {
        Ok(pair) => pair,
        Err(err) => {
            error!("PIE: {key} failed to open ipc rendezvous: {err}");
            return None;
        }
    };

    // A relative `cwd` is joined against the project root; an absolute
    // path replaces it (standard `Path::join` semantics). Assets
    // resolve from here, exactly as for a directly-run game binary.
    let cwd = match run.cwd.as_ref() {
        Some(dir) => root.join(dir),
        None => root.clone(),
    };

    let mut command = Command::new(binary);
    crate::project_build::prepare_game_command(&mut command, binary);
    command
        .current_dir(&cwd)
        .envs(&run.env)
        .env("BEVY_ASSET_ROOT", &cwd)
        .env("JACKDAW_PIE", &server_name)
        .args(&run.args)
        .stderr(Stdio::piped());

    if world
        .get_resource::<PieWindowMode>()
        .copied()
        .map(wants_windowless_env)
        .unwrap_or(true)
    {
        command.env("JACKDAW_PIE_WINDOWLESS", "1");
    }

    // Ask the kernel to SIGKILL this child when the editor (its parent) dies by
    // any means -- including a SIGKILL the editor can never trap, or the
    // `process::exit` the Ctrl+C handler takes (which skips `Drop`). Without
    // this, killing the editor leaves the games running.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;

        // `libc::prctl` is C-variadic; the workspace linter cannot analyze
        // variadic calls, and `PR_SET_PDEATHSIG` only ever takes one
        // argument, so declare that fixed-arity signature for the same
        // symbol.
        unsafe extern "C" {
            fn prctl(option: libc::c_int, arg2: libc::c_ulong) -> libc::c_int;
        }

        // SAFETY: the closure runs in the forked child before `exec`; `prctl`,
        // `getppid`, and `raise` are all async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                if prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                // The editor may have already died in the window between fork
                // and the prctl above; the death signal would never arrive, so
                // exit now rather than be orphaned.
                if libc::getppid() == 1 {
                    libc::raise(libc::SIGKILL);
                }
                Ok(())
            });
        }
    }

    let spawn = command.spawn();

    let mut child = match spawn {
        Ok(child) => child,
        Err(err) => {
            error!("PIE: {key} failed to launch ({}): {err}", binary.display());
            return None;
        }
    };

    // On Windows, we use a job object with JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE set.
    // Because we own the only handle to the object, if Jackdaw crashes, the job object will be closed and the game killed.
    #[cfg(target_os = "windows")]
    pie_windows::PIE_JOB.assign_to_process(&child);

    let stderr_tail: StderrTail = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_TAIL_LINES)));
    if let Some(stderr) = child.stderr.take() {
        let tail = Arc::clone(&stderr_tail);
        std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(mut buf) = tail.lock() {
                    if buf.len() == STDERR_TAIL_LINES {
                        buf.pop_front();
                    }
                    buf.push_back(line);
                }
            }
        });
    }

    let accept = AsyncComputeTaskPool::get().spawn(async move { handle.accept() });

    info!("PIE: {key} launched, awaiting connection");
    Some(ChildStage::Connecting {
        child,
        accept,
        stderr_tail,
        since: Instant::now(),
    })
}

/// Log the buffered stderr tail as an error, for diagnosing a crashed
/// or unconnectable game.
fn report_stderr_tail(stderr_tail: &StderrTail) {
    if let Ok(buf) = stderr_tail.lock()
        && !buf.is_empty()
    {
        let tail: Vec<&str> = buf.iter().map(String::as_str).collect();
        error!("PIE: game stderr tail:\n{}", tail.join("\n"));
    }
}

/// Apply a game pick answer: resolve the bits through the projection and
/// select the preview entity. Stale or unstreamed hits resolve to nothing
/// and are dropped.
pub(crate) fn handle_pick_result(world: &mut World, bits: Option<u64>) {
    let Some(bits) = bits else {
        return;
    };
    let Some(&preview) = world
        .resource::<crate::pie_projection::PieProjection>()
        .by_bits
        .get(&bits)
    else {
        return;
    };
    if world.get_entity(preview).is_err() {
        return;
    }
    let old_entities: Vec<Entity> = world
        .resource::<crate::selection::Selection>()
        .entities
        .clone();
    {
        let mut selection = world.resource_mut::<crate::selection::Selection>();
        selection.entities.clear();
        selection.entities.push(preview);
    }
    for e in old_entities {
        if e != preview
            && let Ok(mut entity_mut) = world.get_entity_mut(e)
        {
            entity_mut.remove::<crate::selection::Selected>();
        }
    }
    if let Ok(mut entity_mut) = world.get_entity_mut(preview) {
        entity_mut.insert(crate::selection::Selected);
    }
}

/// Drain `StateEvent`s from every live child. Events for every instance are
/// always accumulated into that instance's [`InstanceBuffer`](crate::pie_mirror::InstanceBuffer) regardless of
/// view mode, so the buffers always hold current game state and a Scene->Live
/// toggle re-projects fresh data. Additionally, events from the focused instance
/// are projected into the preview ECS via `project_event`, but only in Live mode.
fn drain_game_events(world: &mut World) {
    // Collect all pending (key, events) pairs without holding the session borrow.
    let mut per_instance: Vec<(InstanceKey, Vec<StateEvent>)> = Vec::new();
    let mut pixel_frames: Vec<(InstanceKey, Vec<u8>)> = Vec::new();

    {
        let mut session = world.non_send_mut::<PieSession>();
        for (key, stage) in session.children.iter_mut() {
            let ChildStage::Live { transport, .. } = stage else {
                continue;
            };
            let frames = transport.drain_received();
            if frames.is_empty() {
                continue;
            }
            let mut events: Vec<StateEvent> = Vec::with_capacity(frames.len());
            for (channel, bytes) in frames {
                if channel == PieChannel::Frames {
                    // Binary pixel payload routed to the live frame intake below,
                    // after the focused instance is known.
                    pixel_frames.push((key.clone(), bytes));
                    continue;
                }
                match from_bytes::<StateEvent>(&bytes) {
                    Ok(event) => events.push(event),
                    Err(err) => warn!("PIE: {key} dropping malformed state event: {err}"),
                }
            }
            if !events.is_empty() {
                per_instance.push((key.clone(), events));
            }
        }
    }

    for (key, events) in per_instance {
        let count = events.len();

        // Read focused before any mutable borrow so borrowck is satisfied.
        let focused = world.resource::<PieInstances>().focused.clone();

        // Establish focus on the first instance seen, regardless of view mode.
        let focused = if focused.is_none() {
            world.resource_mut::<PieInstances>().focused = Some(key.clone());
            // First streamed data: content exists to show, so auto-enter Live.
            // `reset_view_mode_on_stop` owns the revert when play stops.
            if *world.resource::<crate::pie_mirror::PieViewMode>()
                == crate::pie_mirror::PieViewMode::Scene
            {
                enter_live_view(world);
            }
            Some(key.clone())
        } else {
            focused
        };

        let is_focused = focused.as_ref() == Some(&key);

        // Read after the auto-enter above so the first batch projects in the
        // same drain it arrives.
        let live_mode = *world.resource::<crate::pie_mirror::PieViewMode>()
            == crate::pie_mirror::PieViewMode::Live;

        for event in events {
            // Mirror the focused game's cursor grab onto the editor before any
            // buffering or projection; the cursor state is not scene data.
            if let StateEvent::CursorState { grabbed, visible } = &event {
                if is_focused {
                    crate::live_input::note_game_cursor_state(world, *grabbed, *visible);
                }
                continue;
            }

            // A pick answer resolves to a preview entity selection; it is not
            // scene data, so it never reaches the buffer or the projector.
            if let StateEvent::PickResult { entity } = &event {
                if is_focused {
                    handle_pick_result(world, *entity);
                }
                continue;
            }

            // Always accumulate into the per-instance buffer.
            world
                .resource_mut::<PieInstances>()
                .buffers
                .entry(key.clone())
                .or_default()
                .apply(&event);

            // Project into the preview world only for the focused instance in Live mode.
            if live_mode && is_focused {
                crate::pie_projection::project_event(world, event);
            }
        }

        debug!("PIE: {key} received {count} state event(s)");
    }

    // Upload the newest pixel frame from the focused instance; earlier frames
    // in the same drain are superseded, and frames from unfocused instances
    // (or arriving before any focus exists) are dropped.
    let focused = world.resource::<PieInstances>().focused.clone();
    if let Some(focused) = focused
        && let Some((_, bytes)) = pixel_frames.iter().rev().find(|(key, _)| *key == focused)
    {
        match jackdaw_pie_protocol::decode_frame(bytes) {
            Some(frame) => world.resource_scope(
                |world, mut stream: Mut<crate::live_frame::LiveFrameStream>| {
                    let mut images = world.resource_mut::<Assets<Image>>();
                    crate::live_frame::apply_frame(&mut stream, &mut images, frame);
                },
            ),
            None => warn!("PIE: dropping malformed pixel frame"),
        }
    }
}

#[cfg(test)]
mod save_to_scene_tests {
    use bevy::ecs::reflect::AppTypeRegistry;
    use jackdaw_bsn::SceneBsnAst;
    use jackdaw_commands::CommandHistory;
    use jackdaw_scene_types::SceneNodeId;

    use super::*;
    use crate::pie_projection::{PieEphemeral, PieProjection};
    use crate::selection::Selection;

    const TRANSFORM_PATH: &str = "bevy_transform::components::transform::Transform";

    /// The whole authored Transform value stored on an entity's document node.
    fn stored_transform(world: &World, entity: Entity) -> Option<jackdaw_bsn::BsnValue> {
        let ast = world.resource::<SceneBsnAst>();
        let node = ast.ast_for(entity)?;
        jackdaw_bsn::get_bsn_field(ast, node, TRANSFORM_PATH, "")
    }

    /// Build a minimal world for Path A tests: an authored preview entity
    /// carrying the LIVE transform value, bound to a document node that
    /// stores the authored (identity) value, and a `PieProjection` entry
    /// mapping `bits -> preview_entity`.
    fn setup_path_a() -> (World, Entity, u64, jackdaw_bsn::BsnValue) {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        registry.write().register::<Transform>();
        world.insert_resource(registry);
        world.init_resource::<CommandHistory>();
        world.init_resource::<PieProjection>();
        world.insert_resource(PieViewMode::Live);

        // Preview entity carries the LIVE value (as `project_event` would apply).
        let live_transform = Transform::from_xyz(1.0, 2.0, 3.0);
        let editor_entity = world.spawn(live_transform).id();

        // The document node stores the authored (identity) value.
        let registry = world.resource::<AppTypeRegistry>().clone();
        let (authored_patch, live_value) = {
            let reg = registry.read();
            (
                jackdaw_bsn::component_to_bsn_patch(&Transform::IDENTITY, &reg),
                jackdaw_bsn::BsnValue::from_reflect(&live_transform, &reg),
            )
        };
        let mut ast = SceneBsnAst::default();
        let node = ast.create_entity_node(vec![authored_patch]);
        ast.add_to_roots(node);
        ast.link(editor_entity, node);
        world.insert_resource(ast);

        // PieProjection: bits -> preview entity. Selection: preview entity is selected.
        let bits = 0xABCDu64;
        world
            .resource_mut::<PieProjection>()
            .by_bits
            .insert(bits, editor_entity);
        world.insert_resource(Selection {
            entities: vec![editor_entity],
        });

        (world, editor_entity, bits, live_value)
    }

    // ---- Path A tests ---------------------------------------------------

    #[test]
    fn path_a_promote_writes_live_values_to_ast_and_preview_ecs() {
        let (mut world, editor_entity, _bits, live_value) = setup_path_a();

        assert!(
            can_save_live_to_scene(&world),
            "authored entity with an AST node is saveable"
        );

        save_live_entity_to_scene(&mut world);

        // The document node now holds the live Transform.
        let stored =
            stored_transform(&world, editor_entity).expect("transform present after promote");
        assert!(
            jackdaw_bsn::bsn_value_eq(&stored, &live_value),
            "the document holds the promoted live Transform"
        );

        // The preview ECS entity was refreshed through SetBsnField.
        let tf = world.get::<Transform>(editor_entity).copied().unwrap();
        assert_eq!(tf.translation, Vec3::new(1.0, 2.0, 3.0));

        // One undoable command was pushed.
        assert_eq!(world.resource::<CommandHistory>().undo_stack.len(), 1);
    }

    #[test]
    fn path_a_promote_is_undoable_back_to_authored_value() {
        let (mut world, editor_entity, _bits, _live_value) = setup_path_a();
        save_live_entity_to_scene(&mut world);

        let mut cmd = world
            .resource_mut::<CommandHistory>()
            .undo_stack
            .pop()
            .expect("a command was pushed");
        cmd.undo(&mut world);

        let tf = world.get::<Transform>(editor_entity).copied().unwrap();
        assert_eq!(
            tf.translation,
            Vec3::ZERO,
            "undo restores the authored Transform"
        );
    }

    // ---- Path B tests ---------------------------------------------------

    /// Build a minimal world for Path B: a `PieEphemeral` preview entity
    /// carrying a Transform, with a `PieProjection` entry and no AST node.
    fn setup_path_b() -> (World, Entity, u64) {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        registry.write().register::<Transform>();
        world.insert_resource(registry);
        world.init_resource::<CommandHistory>();
        world.init_resource::<PieProjection>();
        world.insert_resource(PieViewMode::Live);
        world.insert_resource(SceneBsnAst::default());
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            registry.write().register::<SceneNodeId>();
        }

        // Ephemeral entity: game-spawned at runtime, never in the AST.
        let ephemeral = world
            .spawn((Transform::from_xyz(5.0, 6.0, 7.0), PieEphemeral))
            .id();

        let bits = 0xEF01u64;
        world
            .resource_mut::<PieProjection>()
            .by_bits
            .insert(bits, ephemeral);
        world.insert_resource(Selection {
            entities: vec![ephemeral],
        });

        (world, ephemeral, bits)
    }

    #[test]
    fn path_b_ephemeral_entity_is_saveable() {
        let (world, _ephemeral, _bits) = setup_path_b();
        assert!(
            can_save_live_to_scene(&world),
            "ephemeral entity can be promoted to a new authored node"
        );
    }

    #[test]
    fn path_b_promote_creates_new_ast_node_with_components() {
        let (mut world, ephemeral, _bits) = setup_path_b();

        save_live_entity_to_scene(&mut world);

        // A new document node should exist and be bound to the former
        // ephemeral entity.
        let ast = world.resource::<SceneBsnAst>();
        let node = ast
            .ast_for(ephemeral)
            .expect("node is bound to the promoted entity");
        assert!(
            ast.component_type_paths(node)
                .iter()
                .any(|tp| tp == TRANSFORM_PATH),
            "node carries the captured Transform"
        );

        // The entity now has a SceneNodeId and no longer PieEphemeral.
        assert!(
            world.get::<SceneNodeId>(ephemeral).is_some(),
            "promoted entity carries a SceneNodeId"
        );
        assert!(
            world.get::<PieEphemeral>(ephemeral).is_none(),
            "PieEphemeral is removed after promotion"
        );

        // The document node's id matches the entity's SceneNodeId.
        let node_id = world.get::<SceneNodeId>(ephemeral).copied().unwrap();
        assert_eq!(ast.stable_id_of(node), Some(node_id.0));
    }

    // ---- guard / no-op tests -------------------------------------------

    #[test]
    fn no_selection_is_noop() {
        let mut world = World::new();
        world.init_resource::<PieProjection>();
        world.insert_resource(PieViewMode::Live);
        world.insert_resource(SceneBsnAst::default());
        world.init_resource::<CommandHistory>();
        world.insert_resource(Selection::default());

        assert!(!can_save_live_to_scene(&world));
        save_live_entity_to_scene(&mut world);
        assert_eq!(world.resource::<CommandHistory>().undo_stack.len(), 0);
    }

    #[test]
    fn no_projection_entry_is_noop() {
        // The selected entity has no entry in PieProjection (e.g. a normal
        // authored entity, not a live projection). The gate returns false.
        let mut world = World::new();
        world.init_resource::<PieProjection>();
        world.insert_resource(PieViewMode::Live);
        world.insert_resource(SceneBsnAst::default());
        world.init_resource::<CommandHistory>();

        // Spawn a non-projected entity and select it.
        let stale = world.spawn_empty().id();
        world.insert_resource(Selection {
            entities: vec![stale],
        });

        assert!(!can_save_live_to_scene(&world));
        save_live_entity_to_scene(&mut world);
        assert_eq!(world.resource::<CommandHistory>().undo_stack.len(), 0);
    }
}

#[cfg(test)]
mod stop_cleanup_tests {
    use bevy::ecs::reflect::AppTypeRegistry;
    use bevy::reflect::TypePath;
    use jackdaw_bsn::SceneBsnAst;
    use jackdaw_commands::CommandHistory;

    use super::*;
    use crate::pie_mirror::{InstanceBuffer, PieMirrorEntry};
    use crate::pie_projection::PieProjection;
    use crate::scenes::Scenes;

    #[derive(Component, Reflect, Default, PartialEq, Debug)]
    #[reflect(Component)]
    #[type_path = "stop_cleanup_tests"]
    struct Mutable(i32);

    fn instance_key(name: &str) -> InstanceKey {
        InstanceKey {
            config: name.to_string(),
            instance: 1,
        }
    }

    /// Minimal world that satisfies `cleanup_session_on_stop`: a registry with a
    /// reflected component, a `PieProjection`, an authored AST node bound to a
    /// preview entity, and the resources `revert_preview` touches. `Scenes` has
    /// no tabs, so `respawn_scene_from_ast` no-ops while the despawn and map
    /// clearing still run.
    fn build_focus_world() -> World {
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            registry.write().register::<Mutable>();
        }
        world.init_resource::<PieProjection>();

        let preview_entity = world.spawn(Mutable(0)).id();
        let mut ast = SceneBsnAst::default();
        let node = ast.create_entity_node(Vec::new());
        ast.add_to_roots(node);
        ast.link(preview_entity, node);
        world.insert_resource(ast);

        world.init_resource::<CommandHistory>();
        world.init_resource::<Scenes>();
        world.init_resource::<PieInstances>();
        world
    }

    fn make_buffer_with_entity(bits: u64) -> InstanceBuffer {
        let mut buf = InstanceBuffer::default();
        let mut components = std::collections::HashMap::new();
        components.insert(
            <Mutable as TypePath>::type_path().to_string(),
            serde_json::json!(bits as i32),
        );
        buf.entities.insert(
            bits,
            PieMirrorEntry {
                components,
                scene_node_id: None,
            },
        );
        buf
    }

    #[test]
    fn stop_cleanup_clears_session_state() {
        let mut world = build_focus_world();
        let key = instance_key("game");
        world
            .resource_mut::<PieInstances>()
            .buffers
            .insert(key.clone(), make_buffer_with_entity(0xA0));
        world.resource_mut::<PieInstances>().focused = Some(key);
        let ephemeral = world.spawn(crate::pie_projection::PieEphemeral).id();
        world
            .resource_mut::<crate::pie_projection::PieProjection>()
            .by_bits
            .insert(0xA0, ephemeral);

        cleanup_session_on_stop(&mut world);

        let instances = world.resource::<PieInstances>();
        assert!(instances.buffers.is_empty());
        assert!(instances.focused.is_none());
        assert!(world.get_entity(ephemeral).is_err());
        assert!(
            world
                .resource::<crate::pie_projection::PieProjection>()
                .by_bits
                .is_empty()
        );
    }

    #[test]
    fn pick_result_selects_the_projected_entity() {
        let mut world = build_focus_world();
        let preview = world.spawn_empty().id();
        world
            .resource_mut::<crate::pie_projection::PieProjection>()
            .by_bits
            .insert(0xC3, preview);
        world.init_resource::<crate::selection::Selection>();

        crate::pie::handle_pick_result(&mut world, Some(0xC3));
        assert!(
            world
                .resource::<crate::selection::Selection>()
                .is_selected(preview)
        );

        // Unresolvable bits and None are no-ops, not panics.
        crate::pie::handle_pick_result(&mut world, Some(0xDEAD));
        crate::pie::handle_pick_result(&mut world, None);
    }
}

#[cfg(test)]
mod enter_live_tests {
    use super::*;
    use crate::pie_projection::PieProjection;

    #[test]
    fn enter_live_view_flips_mode_to_live() {
        let mut world = World::new();
        world.insert_resource(PieViewMode::Scene);
        world.init_resource::<PieInstances>();
        world.init_resource::<PieProjection>();

        enter_live_view(&mut world);

        assert_eq!(*world.resource::<PieViewMode>(), PieViewMode::Live);
    }

    #[test]
    fn window_mode_toggle_flips_and_embedded_sets_the_env() {
        assert!(wants_windowless_env(PieWindowMode::Embedded));
        assert!(!wants_windowless_env(PieWindowMode::Windowed));
        let mut mode = PieWindowMode::Embedded;
        toggle_window_mode(&mut mode);
        assert_eq!(mode, PieWindowMode::Windowed);
        toggle_window_mode(&mut mode);
        assert_eq!(mode, PieWindowMode::Embedded);
    }
}
