//! Play-In-Editor runtime.
//!
//! Builds the project with plain `cargo build`, launches the game's own
//! executable as a child process, and drives it over an `ipc-channel`
//! connection. Children send `StateEvent`s back and respond to
//! `ControlEvent`s (Pause / Resume / Step / Stop). Stop reaps the children and leaves
//! the scene document as it stands, unless the project's Play settings ask for
//! the edits made during Play to be undone (see [`crate::play_revert`]). Messages
//! to a child leave on its own writer thread (`GameOutbox`), so the main thread
//! never waits on a game.
//!
//! Instances are keyed by [`InstanceKey`] (config label plus 1-based
//! instance number). All run configurations of a project share one
//! build, keyed by the project root: several instances wait on one build
//! and spawn together when it finishes.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use jackdaw_api::pie::PlayState;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_pie_protocol::event::{from_bytes, to_bytes};
use jackdaw_pie_protocol::manifest::RunConfig;
use jackdaw_pie_protocol::{
    ControlEvent, IpcChannelTransport, PieChannel, PieTransport, StateEvent, serve,
};

use crate::core_extension::CoreExtensionInputContext;
use crate::ext_build::BuildProgress;
use crate::game_output::{GameOutput, OutputCapture, plain_line};
use crate::pie_outbox::{GameOutbox, NodeEdit, OutboxSlot};
use crate::play_scene::LaunchScene;
use crate::project_build::BuildLoad;
use crate::project_build::shim::ShimSpec;
use crate::run_config::RunConfigs;
use crate::worker_task::{WorkerOutcome, WorkerTask};

/// How long to wait for a launched child to connect back before giving
/// up on it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// The editor's remote control port variable, kept out of a launched game.
const EDITOR_REMOTE_PORT_ENV: &str = "JACKDAW_REMOTE_PORT";

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

/// How many stderr lines a [`GameExit`] shows.
const EXIT_STDERR_LINES: usize = 8;

/// How many characters of one stderr line a [`GameExit`] keeps.
const EXIT_STDERR_LINE_CHARS: usize = 200;

/// Why a game went away without being stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GameExitReason {
    /// Exited with a failure status after connecting, as "code N" or "signal N".
    Crashed(String),
    /// Exited before connecting, with its status.
    ExitedBeforeConnecting(String),
    /// Still unconnected at the connect timeout, so it was stopped.
    NeverConnected,
    /// The connection to it could not be set up.
    ConnectFailed(String),
}

/// A game that went away on its own, with the end of its stderr.
#[derive(Clone, Debug)]
pub struct GameExit {
    pub key: InstanceKey,
    pub reason: GameExitReason,
    /// The last few lines the game wrote to stderr, without escape sequences.
    pub stderr: Vec<String>,
}

impl GameExit {
    fn new(key: InstanceKey, reason: GameExitReason, output: OutputCapture) -> Self {
        let lines = output.finish(EXIT_STDERR_LINES);
        Self {
            key,
            reason,
            stderr: lines
                .iter()
                .map(|line| plain_line(line, EXIT_STDERR_LINE_CHARS))
                .collect(),
        }
    }
}

/// Why the last game went away, until Play or Stop is pressed or a game exits
/// cleanly.
#[derive(Resource, Default, Debug)]
pub struct LastGameExit(pub Option<GameExit>);

/// Record `failed` as the last exit, else forget a recorded exit of a game that
/// has since exited cleanly.
fn record_exits(last: &mut LastGameExit, failed: Option<GameExit>, exited: &[InstanceKey]) {
    if failed.is_some() {
        last.0 = failed;
    } else if last
        .0
        .as_ref()
        .is_some_and(|exit| exited.contains(&exit.key))
    {
        last.0 = None;
    }
}

/// A child's exit status as "code N", or "signal N" for one killed by a signal.
fn exit_status_text(status: std::process::ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("code {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    status.to_string()
}

/// What one poll of a child found.
enum ChildPoll {
    Running(ChildStage),
    /// Exited with a success status.
    Exited,
    /// Dropped after its process could not be polled.
    Lost,
    /// Went away uncleanly.
    Failed(GameExit),
}

/// One launched child and its connection progress.
enum ChildStage {
    /// The child is launched and `IpcServerHandle::accept` is blocking
    /// on a task pool, waiting for it to connect. `since` bounds that
    /// wait so a child that never connects does not hang forever.
    Connecting {
        child: Child,
        accept: Task<io::Result<IpcChannelTransport>>,
        output: OutputCapture,
        since: Instant,
    },
    /// The child is connected and running. Messages to it go through `outbox`;
    /// `transport` receives from it.
    Live {
        child: Child,
        transport: IpcChannelTransport,
        outbox: GameOutbox,
        output: OutputCapture,
    },
}

impl ChildStage {
    /// Kill the child and wait for its process to end.
    fn kill_and_reap(&mut self) {
        match self {
            ChildStage::Connecting { child, .. } => {
                child.kill().ok();
                child.wait().ok();
            }
            ChildStage::Live { child, outbox, .. } => {
                outbox.mark_closing();
                child.kill().ok();
                child.wait().ok();
            }
        }
    }
}

/// The product of a completed game build: the executable Play runs. The
/// extracted type schema is persisted to `.jackdaw/schema.json` by the
/// build itself and picked up by the editor's watcher, so it does not
/// travel back through here.
pub(crate) struct GameBuildResult {
    binary: PathBuf,
}

/// An in-flight or finished build, deduped by project root. Instances
/// waiting on it are spawned when it finishes.
pub(crate) enum BuildState {
    /// `cargo build` is running on a task pool; `pending` lists the
    /// instances to spawn once the binary is ready. `progress` is the
    /// sink cargo writes compile progress into, surfaced in the footer.
    Running {
        task: GameBuildTask,
        pending: Vec<PendingSpawn>,
        progress: Arc<Mutex<BuildProgress>>,
    },
    /// The binary is built and cached at this path; later instances of
    /// the same spec spawn from it without rebuilding.
    Done(PathBuf),
    /// The build failed; its pending instances were dropped. `launch` says
    /// whether any were waiting, which a background rebuild's are not.
    Failed { launch: bool },
}

/// A game build on the compute pool. Dropping it cancels the build, which kills
/// cargo, so a launch stopped while building stops compiling too.
pub(crate) struct GameBuildTask {
    task: Task<io::Result<GameBuildResult>>,
    cancel: Arc<AtomicBool>,
}

impl From<Task<io::Result<GameBuildResult>>> for GameBuildTask {
    fn from(task: Task<io::Result<GameBuildResult>>) -> Self {
        Self {
            task,
            cancel: Arc::default(),
        }
    }
}

impl Drop for GameBuildTask {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// One instance waiting for its build to finish before spawning.
pub(crate) struct PendingSpawn {
    key: InstanceKey,
    run: RunConfig,
    scene: Option<LaunchScene>,
}

/// The running instance the Game panel shows and the selection highlights in.
#[derive(Resource, Default, Debug)]
pub struct PieInstances {
    pub focused: Option<InstanceKey>,
}

/// Editor-side play orchestration. `NonSend` because ipc transports
/// are `Send` but not `Sync`.
#[derive(Default)]
pub struct PieSession {
    children: HashMap<InstanceKey, ChildStage>,
    pub(crate) launch_modes: HashMap<InstanceKey, PieWindowMode>,
    /// The scene an instance plays in place of its usual one, recorded at press.
    pub(crate) launch_scenes: HashMap<InstanceKey, LaunchScene>,
    pub(crate) builds: HashMap<PathBuf, BuildState>,
    /// Scene answers being written on a worker.
    serving: Vec<ServingScene>,
    /// Replaces [`CONNECT_TIMEOUT`] when set.
    connect_timeout: Option<Duration>,
    /// A launch's build failed and the Build panel has not yet seen that
    /// failure; the Game panel reports it instead.
    pub(crate) launch_build_failure_unseen: bool,
}

/// A scene answer on a worker, with its places in the instance's outbox.
struct ServingScene {
    key: InstanceKey,
    slots: AnswerSlots,
    waiting: crate::play_scene::WaitingAnswer,
    task: WorkerTask<crate::play_scene::WrittenScene>,
}

/// The places an answer holds in an outbox, in send order.
struct AnswerSlots {
    files: OutboxSlot,
    prefabs: OutboxSlot,
    answer: OutboxSlot,
}

impl AnswerSlots {
    fn fill(self, finished: crate::play_scene::FinishedScene) {
        let encoded = |event: Option<ControlEvent>| {
            let event = event?;
            to_bytes(&event)
                .inspect_err(|err| error!("PIE: failed to encode {event:?}: {err}"))
                .ok()
        };
        if let Some(bytes) = encoded(finished.files) {
            self.files.fill(bytes);
        }
        if let Some(bytes) = encoded(finished.prefabs) {
            self.prefabs.fill(bytes);
        }
        if let Some(bytes) = finished.answer {
            self.answer.fill(bytes);
        }
    }
}

impl Drop for PieSession {
    /// A clean editor shutdown (window close or Ctrl+C) drops the `World` and
    /// so this resource; take the running games down with it. Hard kills that
    /// skip destructors (SIGKILL skips everything) are covered by the game
    /// itself, which exits once its link to the editor closes, and on Linux
    /// and Windows by the kernel as well.
    fn drop(&mut self) {
        for (_key, mut stage) in self.children.drain() {
            stage.kill_and_reap();
        }
    }
}

impl PieSession {
    /// How many scene answers are being written on a worker.
    pub(crate) fn answers_being_written(&self) -> usize {
        self.serving.len()
    }

    /// Whether an instance is currently launched (connecting or live).
    pub fn is_running(&self, key: &InstanceKey) -> bool {
        self.children.contains_key(key)
    }

    /// The window mode an instance was launched with.
    pub fn launch_mode(&self, key: &InstanceKey) -> Option<PieWindowMode> {
        self.launch_modes.get(key).copied()
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

    /// Whether an instance is waiting on a build before it spawns.
    pub(crate) fn is_launching(&self) -> bool {
        self.builds.values().any(
            |build| matches!(build, BuildState::Running { pending, .. } if !pending.is_empty()),
        )
    }

    /// How long the longest-waiting launched instance has gone without connecting.
    pub(crate) fn connecting_for(&self) -> Option<Duration> {
        self.children
            .values()
            .filter_map(|stage| match stage {
                ChildStage::Connecting { since, .. } => Some(since.elapsed()),
                ChildStage::Live { .. } => None,
            })
            .max()
    }

    /// Whether any instance can take document edits now: `Live` while at
    /// least one is connected (others may still be launching), `Launching`
    /// while instances build or connect and none is connected yet, `None`
    /// with no instance launched.
    /// The instances connected and able to take control messages.
    pub(crate) fn live_keys(&self) -> Vec<InstanceKey> {
        self.children
            .iter()
            .filter(|(_, stage)| matches!(stage, ChildStage::Live { .. }))
            .map(|(key, _)| key.clone())
            .collect()
    }

    pub(crate) fn sync_target(&self) -> SyncTarget {
        let mut connecting = false;
        for stage in self.children.values() {
            match stage {
                ChildStage::Live { .. } => return SyncTarget::Live,
                ChildStage::Connecting { .. } => connecting = true,
            }
        }
        if connecting || self.is_launching() {
            SyncTarget::Launching
        } else {
            SyncTarget::None
        }
    }

    fn note_launch_scene(&mut self, key: &InstanceKey, scene: Option<LaunchScene>) {
        match scene {
            Some(scene) => {
                self.launch_scenes.insert(key.clone(), scene);
            }
            None => {
                self.launch_scenes.remove(key);
            }
        }
    }

    /// Whether the last thing a launch did was fail its build.
    pub(crate) fn launch_failed(&self) -> bool {
        self.builds
            .values()
            .any(|build| matches!(build, BuildState::Failed { launch: true }))
    }
}

/// Whether the running instances can take document edits; see
/// [`PieSession::sync_target`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SyncTarget {
    None,
    Launching,
    Live,
}

/// What play-in-editor is doing, from its [`PlayPhase`]: `building` while a
/// launch builds the game or waits for it to connect, `running`, `paused`,
/// `failed` when a launch's build did not compile, `stopped` otherwise.
pub fn play_status(world: &World) -> &'static str {
    let session = world.get_non_send::<PieSession>();
    let state = world
        .get_resource::<State<PlayState>>()
        .map_or(PlayState::Stopped, |state| state.get().clone());
    match play_phase(&state, session) {
        PlayPhase::Building => "building",
        PlayPhase::Playing => "running",
        PlayPhase::Paused => "paused",
        PlayPhase::Stopped if session.is_some_and(PieSession::launch_failed) => "failed",
        PlayPhase::Stopped => "stopped",
    }
}

pub struct PiePlugin;

impl Plugin for PiePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<PlayState>()
            .init_non_send::<PieSession>()
            .init_resource::<PieInstances>()
            .init_resource::<PlayStatus>()
            .init_resource::<LastGameExit>()
            .init_resource::<GameOutput>()
            .init_resource::<crate::live_frame::LiveFrameStream>()
            .init_resource::<crate::live_highlight::LastHighlight>()
            .init_resource::<PieWindowMode>()
            .init_resource::<crate::play_scene::PlayAssetNames>()
            .init_resource::<crate::game_query::GameQueries>()
            // PIE is an editor-only subsystem; gate it to the editor state so it
            // never ticks in the launcher (`ProjectSelect`). Without this,
            // `reconcile_build_status` resets the shared `BuildStatus` from
            // `Building` to `Idle` every frame during the launcher's editor
            // build, so the build's completion handler drops the result and the
            // handoff never fires.
            .add_systems(
                Update,
                (
                    (advance_pie_session, refresh_buttons_on_play_status).chain(),
                    drain_game_events,
                    crate::live_highlight::sync_selection_highlight,
                    (
                        crate::game_query::sync_watch,
                        crate::game_query::expire_game_queries,
                    ),
                )
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_systems(
                OnEnter(PlayState::Stopped),
                (
                    cleanup_session_on_stop,
                    crate::live_frame::clear_stream,
                    crate::play_scene::forget_asset_names,
                    crate::play_overlay::clear_play_sidecars,
                    crate::game_query::forget_game_queries,
                ),
            )
            .add_systems(
                OnExit(PlayState::Stopped),
                crate::play_overlay::clear_play_sidecars,
            );
    }
}

/// Shared cleanup for every path into `Stopped`: the stop button, a crashed or
/// self-exited child being reaped, and the last per-instance stop.
fn cleanup_session_on_stop(world: &mut World) {
    let _span = info_span!("play.cleanup").entered();
    if let Some(mut instances) = world.get_resource_mut::<PieInstances>() {
        instances.focused = None;
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<PiePlayOp>()
        .register_operator::<PiePauseOp>()
        .register_operator::<PieStepOp>()
        .register_operator::<PieStopOp>()
        .register_operator::<PieRestartOp>()
        .register_operator::<PieToggleInstanceOp>()
        .register_operator::<ProjectBuildOp>()
        .register_operator::<ProjectToggleAutoBuildOp>()
        .register_operator::<ProjectTogglePrebuildOp>()
        .register_operator::<PieWindowModeToggleOp>()
        .register_operator::<PiePopOutOp>()
        .register_operator::<PieNextInstanceOp>()
        .register_operator::<PieFocusInstanceOp>()
        .register_operator::<crate::live_input::PiePlayInputToggleOp>();

    ctx.bind_operator::<CoreExtensionInputContext, PiePlayOp>([PresetInput::key("F5")]);
    ctx.bind_operator::<CoreExtensionInputContext, PieRestartOp>([
        PresetInput::key("F5").ctrl_or_super()
    ]);
    ctx.bind_operator::<CoreExtensionInputContext, PiePauseOp>([PresetInput::key("F7")]);
    ctx.bind_operator::<CoreExtensionInputContext, PieStopOp>([PresetInput::key("F8")]);
    ctx.bind_operator::<CoreExtensionInputContext, PieStepOp>([PresetInput::key("F10")]);
}

/// What Play is doing, as the toolbar and the status bar show it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayPhase {
    #[default]
    Stopped,
    /// A launch is building the game or waiting for it to connect.
    Building,
    Playing,
    Paused,
}

/// Which transport controls can be used in a [`PlayPhase`]. Play stays usable
/// while playing so it shows the accent fill; pressing it then does nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportControls {
    pub play: bool,
    pub pause: bool,
    pub step: bool,
    pub stop: bool,
    pub restart: bool,
}

impl PlayPhase {
    pub fn controls(self) -> TransportControls {
        let running = matches!(self, PlayPhase::Playing | PlayPhase::Paused);
        TransportControls {
            play: self != PlayPhase::Building,
            pause: running,
            step: self == PlayPhase::Paused,
            stop: self != PlayPhase::Stopped,
            restart: running,
        }
    }

    /// What the Play button reads.
    pub fn play_caption(self) -> &'static str {
        match self {
            PlayPhase::Stopped | PlayPhase::Playing => "Play",
            PlayPhase::Building => "Building...",
            PlayPhase::Paused => "Resume",
        }
    }
}

/// The current [`PlayPhase`] and the name of the run configuration it is for,
/// `None` for an unnamed one. Written only when either changes, so readers can
/// rely on change detection.
#[derive(Resource, Default, Clone, Debug, PartialEq, Eq)]
pub struct PlayStatus {
    pub phase: PlayPhase,
    pub config: Option<String>,
}

/// Play moved from one [`PlayPhase`] to another.
#[derive(Event, Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayPhaseChanged {
    pub from: PlayPhase,
    pub to: PlayPhase,
}

/// The phase `state` and `session` add up to. Instances launched or waiting
/// on a build while the state is still `Stopped` are starting up.
pub(crate) fn play_phase(state: &PlayState, session: Option<&PieSession>) -> PlayPhase {
    match state {
        PlayState::Playing => PlayPhase::Playing,
        PlayState::Paused => PlayPhase::Paused,
        PlayState::Stopped => {
            if session.is_some_and(|session| !session.children.is_empty() || session.is_launching())
            {
                PlayPhase::Building
            } else {
                PlayPhase::Stopped
            }
        }
    }
}

/// The run configuration Play is for: the focused instance's, else the first
/// launched or waiting one's.
fn play_config(session: &PieSession, focused: Option<&InstanceKey>) -> Option<String> {
    let mut waiting = session.builds.values().filter_map(|build| match build {
        BuildState::Running { pending, .. } => pending.first().map(|spawn| &spawn.key),
        _ => None,
    });
    focused
        .or_else(|| {
            session
                .children
                .keys()
                .min_by_key(|key| (&key.config, key.instance))
        })
        .or_else(|| waiting.next())
        .map(|key| key.config.clone())
}

fn reconcile_play_status(world: &mut World) {
    let state = world.resource::<State<PlayState>>().get().clone();
    let session = world.non_send::<PieSession>();
    let phase = play_phase(&state, Some(session));
    let focused = world
        .get_resource::<PieInstances>()
        .and_then(|instances| instances.focused.as_ref());
    let named = |label: &String| {
        world
            .get_resource::<RunConfigs>()
            .and_then(|configs| configs.manifest.run_by_name(label))
            .is_some_and(|run| run.name.is_some())
    };
    let config = match phase {
        PlayPhase::Stopped => None,
        _ => play_config(session, focused).filter(named),
    };
    let status = PlayStatus { phase, config };
    let Some(mut held) = world.get_resource_mut::<PlayStatus>() else {
        return;
    };
    let was = held.phase;
    held.set_if_neq(status);
    if was != phase {
        world.trigger(PlayPhaseChanged {
            from: was,
            to: phase,
        });
    }
}

/// Re-check every operator button when Play changes phase: a build finishing
/// or a game exiting changes what the transport controls can do without any
/// operator running.
fn refresh_buttons_on_play_status(status: Res<PlayStatus>, mut commands: Commands) {
    if status.is_changed() {
        commands.trigger(RefreshOperatorButtons);
    }
}

fn phase_now(state: &State<PlayState>, session: Option<&NonSend<PieSession>>) -> PlayPhase {
    play_phase(state.get(), session.map(|session| &**session))
}

fn can_play(state: Res<State<PlayState>>, session: Option<NonSend<PieSession>>) -> bool {
    phase_now(&state, session.as_ref()).controls().play
}

fn can_pause(state: Res<State<PlayState>>, session: Option<NonSend<PieSession>>) -> bool {
    phase_now(&state, session.as_ref()).controls().pause
}

fn can_step(state: Res<State<PlayState>>, session: Option<NonSend<PieSession>>) -> bool {
    phase_now(&state, session.as_ref()).controls().step
}

fn can_stop(state: Res<State<PlayState>>, session: Option<NonSend<PieSession>>) -> bool {
    phase_now(&state, session.as_ref()).controls().stop
}

fn can_restart(state: Res<State<PlayState>>, session: Option<NonSend<PieSession>>) -> bool {
    phase_now(&state, session.as_ref()).controls().restart
}

/// Start the game. From Stopped, builds the project's game binary and
/// launches it connected to the editor; from Paused, resumes.
#[operator(
    id = "pie.play", writes_disk = true,
    label = "Play",
    description = "Start the game running in the editor, or resume a paused game.",
    is_available = can_play
)]
pub(crate) fn pie_play(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(handle_play);
    OperatorResult::Finished
}

/// Pause the running game, or resume a paused one.
#[operator(
    id = "pie.pause",
    label = "Pause",
    description = "Pause the running game, or resume it when paused.",
    is_available = can_pause
)]
pub(crate) fn pie_pause(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(handle_pause);
    OperatorResult::Finished
}

/// Advance the paused game by one fixed tick.
#[operator(
    id = "pie.step",
    label = "Step",
    description = "Advance the paused game by one tick.",
    is_available = can_step
)]
pub(crate) fn pie_step(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(handle_step);
    OperatorResult::Finished
}

/// Stop the running game, or cancel a launch still building.
#[operator(
    id = "pie.stop",
    label = "Stop",
    description = "Stop the running game, or cancel a launch that is still building.",
    is_available = can_stop
)]
pub(crate) fn pie_stop(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(handle_stop);
    OperatorResult::Finished
}

/// Rebuild and relaunch the running game.
#[operator(
    id = "pie.restart",
    label = "Restart",
    description = "Rebuild and relaunch the running game.",
    is_available = can_restart
)]
pub(crate) fn pie_restart(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(handle_restart);
    OperatorResult::Finished
}

/// Launch one instance of a run configuration, or stop it if it is running.
#[operator(
    id = "pie.toggle_instance",
    writes_disk = true,
    label = "Toggle Game Instance",
    description = "Launch one instance of a run configuration, or stop it when it is running.",
    params(
        config(String, doc = "The run configuration's name."),
        instance(i64, default = 1, doc = "Which instance of it, counting from 1."),
    )
)]
pub(crate) fn pie_toggle_instance(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(config) = params.as_str("config").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    let Ok(instance) = u32::try_from(params.as_int("instance").unwrap_or(1)) else {
        return OperatorResult::Cancelled;
    };
    let key = InstanceKey { config, instance };
    commands.queue(move |world: &mut World| {
        if world.non_send::<PieSession>().is_running(&key) {
            stop_instance(world, &key);
        } else if let Some(run) = world
            .get_resource::<RunConfigs>()
            .and_then(|configs| configs.manifest.run_by_name(&key.config).cloned())
        {
            launch_instance(world, key, run);
        }
    });
    OperatorResult::Finished
}

/// Rebuild the project now so new or changed code is picked up by the
/// editor. This is the manual counterpart to auto-build and runs the same
/// pipeline as an out-of-editor `jackdaw build`: a game persists the schema
/// the editor then reloads, and an extension is loaded into the editor.
#[operator(
    id = "project.build",
    writes_disk = true,
    label = "Rebuild Project",
    description = "Rebuild the project so new or changed components and extensions appear in the editor."
)]
pub(crate) fn project_build(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let Some(root) = project_root(world) else {
            return;
        };
        if crate::extension_build::is_extension_project(world, &root, true) {
            crate::extension_build::start_extension_build(world, &root);
        } else {
            crate::project_build::spawn_project_build(world, &root, BuildLoad::Foreground);
        }
    });
    OperatorResult::Finished
}

/// Turn automatic rebuild-on-source-change on or off. Off by default so
/// the editor never builds behind the user's back; enabling it gives
/// live pickup for those who want it.
#[operator(
    id = "project.toggle_auto_build",
    writes_disk = true,
    label = "Toggle Auto Build",
    description = "Turn automatic rebuild-on-source-change on or off."
)]
pub(crate) fn project_toggle_auto_build(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let enabled = {
            let mut settings = world.resource_mut::<crate::project_build::EditorBuildSettings>();
            settings.auto_build = !settings.auto_build;
            settings.auto_build
        };
        crate::project_build::persist_project_build_settings(world);
        info!(
            "PIE: auto-build {}",
            if enabled { "enabled" } else { "disabled" }
        );
    });
    OperatorResult::Finished
}

/// Turn the open-time background pre-build on or off. On by default; the
/// `JACKDAW_PIE_PREBUILD` environment variable overrides this for a
/// single session.
#[operator(
    id = "project.toggle_prebuild",
    writes_disk = true,
    label = "Toggle Prebuild On Open",
    description = "Turn the background pre-build that runs when a project opens on or off."
)]
pub(crate) fn project_toggle_prebuild(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let enabled = {
            let mut settings = world.resource_mut::<crate::project_build::EditorBuildSettings>();
            settings.prebuild = !settings.prebuild;
            settings.prebuild
        };
        crate::project_build::persist_project_build_settings(world);
        info!(
            "PIE: pre-build on open {}",
            if enabled { "enabled" } else { "disabled" }
        );
    });
    OperatorResult::Finished
}

/// Input capture needs an embedded game with a focused instance and a fresh
/// frame to forward to.
pub(crate) fn focused_with_fresh_stream(
    stream: Res<crate::live_frame::LiveFrameStream>,
    instances: Res<PieInstances>,
    launched: LaunchedWindowMode,
) -> bool {
    launched.get() == PieWindowMode::Embedded && instances.focused.is_some() && stream.is_fresh()
}

/// The window mode of the game being shown: the focused instance's launch
/// mode, else any launched instance's, else the [`PieWindowMode`] setting a
/// launch would use now.
pub(crate) fn effective_window_mode(
    instances: Option<&PieInstances>,
    session: Option<&PieSession>,
    setting: Option<PieWindowMode>,
) -> PieWindowMode {
    let focused = instances
        .and_then(|instances| instances.focused.as_ref())
        .and_then(|key| session?.launch_mode(key));
    focused
        .or_else(|| session?.launch_modes.values().next().copied())
        .or(setting)
        .unwrap_or_default()
}

/// System-param access to [`effective_window_mode`].
#[derive(SystemParam)]
pub(crate) struct LaunchedWindowMode<'w> {
    instances: Option<Res<'w, PieInstances>>,
    session: Option<NonSend<'w, PieSession>>,
    setting: Option<Res<'w, PieWindowMode>>,
}

impl LaunchedWindowMode<'_> {
    pub(crate) fn get(&self) -> PieWindowMode {
        effective_window_mode(
            self.instances.as_deref(),
            self.session.as_deref(),
            self.setting.as_deref().copied(),
        )
    }
}

/// [`effective_window_mode`] for exclusive access.
pub(crate) fn effective_window_mode_in(world: &World) -> PieWindowMode {
    effective_window_mode(
        world.get_resource::<PieInstances>(),
        world.get_non_send::<PieSession>(),
        world.get_resource::<PieWindowMode>().copied(),
    )
}

/// Record `key` as launched with `mode` and focused, without a child process.
#[cfg(test)]
pub(crate) fn record_test_launch(world: &mut World, key: &InstanceKey, mode: PieWindowMode) {
    world.init_resource::<PieInstances>();
    if world.get_non_send::<PieSession>().is_none() {
        world.insert_non_send(PieSession::default());
    }
    world
        .non_send_mut::<PieSession>()
        .launch_modes
        .insert(key.clone(), mode);
    world.resource_mut::<PieInstances>().focused = Some(key.clone());
}

#[cfg(test)]
impl PieSession {
    /// Add a connected instance over `transport`, with `child` standing in
    /// for its process.
    pub(crate) fn insert_test_live(
        &mut self,
        key: InstanceKey,
        child: Child,
        transport: IpcChannelTransport,
    ) {
        let outbox = GameOutbox::new(&transport).expect("a writer thread");
        let output = OutputCapture::detached(&key);
        self.children.insert(
            key,
            ChildStage::Live {
                child,
                transport,
                outbox,
                output,
            },
        );
    }

    /// Add an instance that is still connecting and never connects.
    pub(crate) fn insert_test_connecting(&mut self, key: InstanceKey, child: Child) {
        let accept = AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new)
            .spawn(std::future::pending::<io::Result<IpcChannelTransport>>());
        let output = OutputCapture::detached(&key);
        self.children.insert(
            key,
            ChildStage::Connecting {
                child,
                accept,
                output,
                since: Instant::now(),
            },
        );
    }

    /// Fail every build, dropping the instances waiting on it.
    pub(crate) fn fail_test_builds(&mut self) {
        for build in self.builds.values_mut() {
            *build = BuildState::Failed { launch: true };
        }
    }

    /// Add a build that never finishes with `key` waiting on it.
    pub(crate) fn insert_test_pending_build(&mut self, key: InstanceKey) {
        let task = AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new)
            .spawn(std::future::pending::<io::Result<GameBuildResult>>());
        self.builds.insert(
            PathBuf::from(format!("test-build-{key}")),
            BuildState::Running {
                task: task.into(),
                pending: vec![PendingSpawn {
                    key,
                    run: RunConfig::default(),
                    scene: None,
                }],
                progress: Arc::default(),
            },
        );
    }
}

/// Connect `transport` as the live, focused game `key`, with `child` standing in
/// for its process, for a test. Nothing reaps `child` until the session advances.
#[doc(hidden)]
pub fn connect_test_game(
    world: &mut World,
    key: InstanceKey,
    mut child: Child,
    transport: IpcChannelTransport,
) -> io::Result<()> {
    let outbox = GameOutbox::new(&transport)?;
    let output = world
        .get_resource_or_init::<GameOutput>()
        .capture(&key, &mut child);
    if !world.contains_non_send::<PieSession>() {
        world.insert_non_send(PieSession::default());
    }
    world.non_send_mut::<PieSession>().children.insert(
        key.clone(),
        ChildStage::Live {
            child,
            transport,
            outbox,
            output,
        },
    );
    let mode = world
        .get_resource::<PieWindowMode>()
        .copied()
        .unwrap_or_default();
    world
        .non_send_mut::<PieSession>()
        .launch_modes
        .insert(key.clone(), mode);
    greet_connected(world, &key);
    world.get_resource_or_init::<PieInstances>().focused = Some(key);
    Ok(())
}

/// Add `child` as game `key` that is still connecting and never connects, for a
/// test.
#[doc(hidden)]
pub fn connecting_test_game(world: &mut World, key: InstanceKey, mut child: Child) {
    if !world.contains_non_send::<PieSession>() {
        world.insert_non_send(PieSession::default());
    }
    let accept = AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new)
        .spawn(std::future::pending::<io::Result<IpcChannelTransport>>());
    let output = world
        .get_resource_or_init::<GameOutput>()
        .capture(&key, &mut child);
    world.non_send_mut::<PieSession>().children.insert(
        key,
        ChildStage::Connecting {
            child,
            accept,
            output,
            since: Instant::now(),
        },
    );
}

/// Hold `binary` as the open project's built game, for a test: Play then
/// launches it as it would a game it just built.
#[doc(hidden)]
pub fn insert_built_game(world: &mut World, binary: PathBuf) {
    let Some(root) = project_root(world) else {
        return;
    };
    world
        .non_send_mut::<PieSession>()
        .builds
        .insert(root, BuildState::Done(binary));
}

/// Give up on games that have not connected after `timeout`, for a test.
#[doc(hidden)]
pub fn set_connect_timeout(world: &mut World, timeout: Duration) {
    if !world.contains_non_send::<PieSession>() {
        world.insert_non_send(PieSession::default());
    }
    world.non_send_mut::<PieSession>().connect_timeout = Some(timeout);
}

/// Hold the open project's game build as in progress, for a test: a launch then
/// joins the build's waiting list instead of running anything.
#[doc(hidden)]
pub fn insert_build_in_progress(world: &mut World) {
    let Some(root) = project_root(world) else {
        return;
    };
    let task = AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new)
        .spawn(std::future::pending::<io::Result<GameBuildResult>>());
    world.non_send_mut::<PieSession>().builds.insert(
        root,
        BuildState::Running {
            task: task.into(),
            pending: Vec::new(),
            progress: Arc::default(),
        },
    );
}

/// Fail the game builds in progress as a compile error would, dropping the
/// launches waiting on them, for a test.
#[doc(hidden)]
pub fn fail_builds_in_progress(world: &mut World) {
    let mut session = world.non_send_mut::<PieSession>();
    for build in session.builds.values_mut() {
        *build = BuildState::Failed { launch: true };
    }
    session.launch_build_failure_unseen = true;
    if let Some(mut status) = world.get_resource_mut::<crate::build_status::BuildStatus>() {
        status.state = crate::build_status::BuildState::Failed { at: Instant::now() };
    }
}

/// The launches waiting on a game build, for a test.
#[doc(hidden)]
pub fn launches_waiting_on_build(
    world: &World,
) -> Vec<(RunConfig, Option<crate::play_scene::LaunchScene>)> {
    world
        .non_send::<PieSession>()
        .builds
        .values()
        .filter_map(|build| match build {
            BuildState::Running { pending, .. } => Some(pending),
            _ => None,
        })
        .flatten()
        .map(|spawn| (spawn.run.clone(), spawn.scene.clone()))
        .collect()
}

/// Where a launched game's window lives. A launch records the mode it used, and
/// running instances keep it; changing the setting only affects later launches.
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

impl PieWindowMode {
    pub(crate) fn from_separate(separate: bool) -> Self {
        if separate {
            Self::Windowed
        } else {
            Self::Embedded
        }
    }
}

fn toggle_window_mode(mode: &mut PieWindowMode) {
    *mode = match *mode {
        PieWindowMode::Embedded => PieWindowMode::Windowed,
        PieWindowMode::Windowed => PieWindowMode::Embedded,
    };
}

/// Flip the window setting and keep it with the developer's Play settings.
fn toggle_and_store_window_mode(world: &mut World) -> PieWindowMode {
    let mode = {
        let mut mode = world.get_resource_or_init::<PieWindowMode>();
        toggle_window_mode(&mut mode);
        *mode
    };
    if let Some(mut settings) = world.get_resource_mut::<crate::play_settings::PlaySettings>() {
        settings.separate_window = mode == PieWindowMode::Windowed;
    }
    crate::play_settings::store(world);
    mode
}

/// Switch the next launch between an embedded game (streamed into the Game
/// panel) and a separate game window.
#[operator(
    id = "pie.window_mode_toggle",
    writes_disk = true,
    label = "Play in Separate Window",
    description = "Launch the game in its own window instead of the Game panel. Applies to the next launch."
)]
pub(crate) fn pie_window_mode_toggle(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        toggle_and_store_window_mode(world);
    });
    OperatorResult::Finished
}

/// Move the game between the Game panel and a separate window. A running game
/// is relaunched in the new place.
#[operator(
    id = "pie.pop_out",
    writes_disk = true,
    label = "Pop Out Game",
    description = "Play in a separate window (for real performance numbers), or back in the Game panel. A running game relaunches there."
)]
pub(crate) fn pie_pop_out(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let mut target = world
            .get_resource::<PieWindowMode>()
            .copied()
            .unwrap_or_default();
        toggle_window_mode(&mut target);
        let plan = match plan_relaunch(world, target) {
            Ok(plan) => plan,
            Err(reason) => {
                crate::status_bar::notify_warn(world, reason);
                return;
            }
        };
        toggle_and_store_window_mode(world);
        if let Some(plan) = plan {
            relaunch(world, target, plan);
        }
    });
    OperatorResult::Finished
}

/// The running instances to relaunch in another window mode, and the built game
/// to relaunch them from.
pub(crate) struct RelaunchPlan {
    moving: Vec<(InstanceKey, RunConfig)>,
    binary: PathBuf,
}

/// What moving the running instances to `mode` needs: `None` when none run in
/// the other mode, or why they cannot move now.
fn plan_relaunch(world: &World, mode: PieWindowMode) -> Result<Option<RelaunchPlan>, String> {
    let Some(session) = world.get_non_send::<PieSession>() else {
        return Ok(None);
    };
    let mut keys: Vec<InstanceKey> = session
        .children
        .keys()
        .filter(|key| {
            session
                .launch_mode(key)
                .is_some_and(|launched| launched != mode)
        })
        .cloned()
        .collect();
    if keys.is_empty() {
        return Ok(None);
    }
    keys.sort_by(|a, b| a.config.cmp(&b.config).then(a.instance.cmp(&b.instance)));
    let binary = project_root(world).and_then(|root| match session.builds.get(&root) {
        Some(BuildState::Done(binary)) => Some(binary.clone()),
        _ => None,
    });
    let Some(binary) = binary else {
        return Err(
            "The game cannot move while it is being built. Try again once the build is done."
                .to_string(),
        );
    };
    let Some(manifest) = world
        .get_resource::<RunConfigs>()
        .map(|configs| &configs.manifest)
    else {
        return Err("The game cannot move: the run configurations are not loaded.".to_string());
    };
    let mut moving = Vec::with_capacity(keys.len());
    for key in keys {
        let Some(run) = manifest.run_by_name(&key.config).cloned() else {
            return Err(format!(
                "The game cannot move: no run configuration is named '{}'.",
                key.config
            ));
        };
        moving.push((key, run));
    }
    Ok(Some(RelaunchPlan { moving, binary }))
}

/// Relaunch the planned instances in `mode` from the game already built, each
/// with the scene it launched with. The new process starts before the old one
/// ends, so a failed start leaves the old one running. Play carries on through
/// the move, so it counts as no stop and its edits stay. The focused game
/// coming into the Game panel takes input on its first frame, as a launch does.
fn relaunch(world: &mut World, mode: PieWindowMode, plan: RelaunchPlan) {
    let focused = world
        .get_resource::<PieInstances>()
        .and_then(|instances| instances.focused.clone());
    let mut failed = Vec::new();
    let mut focus_moved = false;
    for (key, run) in plan.moving {
        let Some((stage, launched)) = spawn_instance(world, &key, &run, &plan.binary) else {
            failed.push(key.to_string());
            continue;
        };
        let scene = world
            .non_send::<PieSession>()
            .launch_scenes
            .get(&key)
            .cloned();
        retire_instance(world, &key);
        focus_moved |= focused.as_ref() == Some(&key);
        {
            let mut session = world.non_send_mut::<PieSession>();
            session.launch_modes.insert(key.clone(), launched);
            session.note_launch_scene(&key, scene);
            session.children.insert(key, stage);
        }
        crate::viewport_redraw::note_game_started(world);
    }
    if focus_moved {
        crate::live_frame::clear_stream(world);
        if mode == PieWindowMode::Embedded {
            crate::live_input::arm_first_frame_capture(world);
        }
    }
    if !failed.is_empty() {
        crate::status_bar::notify_error(
            world,
            format!(
                "Could not relaunch {} in the new place; it keeps running where it was.",
                failed.join(", ")
            ),
        );
    }
}

/// Stop every running instance, drop the cached build path so the next
/// launch re-runs the (incremental) cargo build, then relaunch each
/// instance that was running. The game asks the editor for its scene again.
pub fn handle_restart(world: &mut World) {
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
        warn!("PIE: Restart but run configurations are not loaded");
        return;
    };

    handle_stop(world);
    crate::live_input::arm_first_frame_capture(world);

    for key in keys {
        let Some(run) = run_configs.run_by_name(&key.config).cloned() else {
            warn!("PIE: Restart could not find run config '{}'", key.config);
            continue;
        };
        launch_instance(world, key, run);
    }

    info!("PIE: Restart (rebuild + relaunch)");
}

/// Resolve the open project's root directory, or log and bail if no
/// project is open (Play has nothing to build without one).
pub(crate) fn project_root(world: &World) -> Option<PathBuf> {
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
    if let Some(mut last) = world.get_resource_mut::<LastGameExit>() {
        last.0 = None;
    }
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
    crate::play_overlay::notice_unsaved_model_settings(world);
    let scene = crate::play_scene::launch_scene(world, &run);

    // If the game binary is built AND still current, spawn from it
    // immediately. A source edit since the last build invalidates the
    // cache so Play runs the new code: each Play is a fresh process, so
    // rebuilding is always safe.
    if let Some(BuildState::Done(path)) = world.non_send::<PieSession>().builds.get(&root) {
        if source_is_current(&root, path) {
            let path = path.clone();
            spawn_from_binary(world, key, &run, scene, &path);
            return;
        }
        info!("PIE: project source changed; rebuilding before Play");
        world.non_send_mut::<PieSession>().builds.remove(&root);
    }

    // Join an in-flight build's pending list, or start a new build.
    if let Some(BuildState::Running { pending, .. }) =
        world.non_send_mut::<PieSession>().builds.get_mut(&root)
    {
        pending.push(PendingSpawn { key, run, scene });
        return;
    }
    let spec = match crate::project_build::shim_spec_for_project(&root) {
        Ok(spec) => spec,
        Err(error) => {
            crate::project_build::report_unbuildable(world, &root, &error);
            return;
        }
    };
    info!("PIE: building the game for {key}");
    let (task, progress) = spawn_game_build(&root, spec, BuildLoad::Foreground);
    world.non_send_mut::<PieSession>().builds.insert(
        root,
        BuildState::Running {
            task,
            pending: vec![PendingSpawn { key, run, scene }],
            progress,
        },
    );
}

/// Spawn instance `key` from the built game at `binary`, noting how and with
/// which scene it launched.
fn spawn_from_binary(
    world: &mut World,
    key: InstanceKey,
    run: &RunConfig,
    scene: Option<LaunchScene>,
    binary: &Path,
) {
    if let Some((stage, mode)) = spawn_instance(world, &key, run, binary) {
        let mut session = world.non_send_mut::<PieSession>();
        session.launch_modes.insert(key.clone(), mode);
        session.note_launch_scene(&key, scene);
        session.children.insert(key, stage);
        crate::viewport_redraw::note_game_started(world);
    }
}

/// The scene a game loads as its main one, inside the asset folder: what the
/// game template loads and what the editor opens for a project with no
/// remembered tabs. No project setting names another.
pub(crate) const MAIN_SCENE: &str = "scene.bsn";

/// The asset path of the scene in tab `tab`, if it is a scene with a file in the
/// project.
pub(crate) fn scene_asset_path_of_tab(world: &World, tab: usize) -> Option<String> {
    let scenes = world.get_resource::<crate::scenes::Scenes>()?;
    let tab = scenes.tabs.get(tab)?;
    if tab.kind != crate::scenes::TabKind::Scene {
        return None;
    }
    world
        .get_resource::<crate::project::ProjectRoot>()?
        .to_asset_path(tab.path.as_deref()?)
}

/// Whether the built game binary is newer than the project's source. A
/// stale binary (source edited since the build) means Play must rebuild.
/// `Cargo.toml` and everything under `src/` are checked; a missing
/// binary counts as stale.
pub(crate) fn source_is_current(root: &Path, binary: &Path) -> bool {
    std::fs::metadata(binary)
        .and_then(|m| m.modified())
        .is_ok_and(|binary_mtime| source_unchanged_since(root, binary_mtime))
}

/// Whether nothing under `src/` and no `Cargo.toml` change is newer than
/// `built`.
pub(crate) fn source_unchanged_since(root: &Path, built: std::time::SystemTime) -> bool {
    let newest_source = newest_mtime(&root.join("src"))
        .into_iter()
        .chain(
            std::fs::metadata(root.join("Cargo.toml"))
                .and_then(|m| m.modified())
                .ok(),
        )
        .max();
    match newest_source {
        Some(src_mtime) => src_mtime <= built,
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

/// One line naming what actually went wrong, for the status bar: the first
/// real diagnostic in the log and where it points, rather than `Display`'s
/// summary line.
pub(crate) fn failure_summary(
    error: &crate::project_build::ProjectBuildError,
    detail: &str,
) -> String {
    /// Long enough for a rustc error line, short enough for the footer.
    const MAX: usize = 140;
    let mut lines = detail.lines().map(str::trim);
    let Some(line) = lines.find(|line| line.starts_with("error")) else {
        return error.to_string();
    };
    let summary = if line.chars().count() > MAX {
        let cut: String = line.chars().take(MAX).collect();
        format!("{cut}...")
    } else {
        line.to_string()
    };
    let location = lines
        .take_while(|line| !line.is_empty())
        .find_map(|line| line.strip_prefix("--> "));
    match location {
        Some(location) => format!("{summary} ({location})"),
        None => summary,
    }
}

/// Start the project's game build on the compute pool, returning the
/// task and the progress sink the footer polls.
pub(crate) fn spawn_game_build(
    root: &Path,
    spec: ShimSpec,
    load: jackdaw_project_build::BuildLoad,
) -> (GameBuildTask, Arc<Mutex<BuildProgress>>) {
    let progress = Arc::new(Mutex::new(BuildProgress::default()));
    let sink = Arc::clone(&progress);
    let jackdaw_dir = root.join(".jackdaw");
    let cancel = Arc::new(AtomicBool::new(false));
    let cancelled = Arc::clone(&cancel);
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
        let result = crate::project_build::build_project_binary_cancellable(
            &spec,
            &jackdaw_dir,
            load,
            &cancelled,
            &mut report,
        );
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
                // reporter, so just note the failure and surface the error,
                // adding the detail only when it was not streamed.
                let detail = match &err {
                    crate::project_build::ProjectBuildError::Compile { log } => log.clone(),
                    other => other.to_string(),
                };
                let summary = failure_summary(&err, &detail);
                if let Ok(mut p) = sink.lock() {
                    p.push_log(format!("build failed: {err}"));
                    if detail != err.to_string() && !p.full_log.contains(detail.trim()) {
                        p.push_log(detail.clone());
                    }
                }
                Err(io::Error::other(summary))
            }
        }
    });
    (GameBuildTask { task, cancel }, progress)
}

/// Stop one instance: ask a live child to exit, reap it, and drop it.
/// Returns to authoring mode once no children remain.
pub(crate) fn stop_instance(world: &mut World, key: &InstanceKey) {
    if !retire_instance(world, key) {
        return;
    }
    let last_stopped = world.non_send::<PieSession>().children.is_empty();
    if last_stopped && let Some(mut last) = world.get_resource_mut::<LastGameExit>() {
        last.0 = None;
    }
    if last_stopped && *world.resource::<State<PlayState>>().get() != PlayState::Stopped {
        world
            .resource_mut::<NextState<PlayState>>()
            .set(PlayState::Stopped);
    }
}

/// End instance `key`'s process and forget it, leaving Play's state alone.
/// Returns whether it was running.
fn retire_instance(world: &mut World, key: &InstanceKey) -> bool {
    world
        .non_send_mut::<PieSession>()
        .serving
        .retain(|serving| serving.key != *key);
    if let Some(mut names) = world.get_resource_mut::<crate::play_scene::PlayAssetNames>() {
        names.retain_games(|held| held != key);
    }
    let mut session = world.non_send_mut::<PieSession>();
    session.launch_modes.remove(key);
    session.launch_scenes.remove(key);
    let Some(mut stage) = session.children.remove(key) else {
        return false;
    };
    if let ChildStage::Live { outbox, .. } = &stage {
        send_control_to(outbox, ControlEvent::Stop);
    }
    stage.kill_and_reap();
    true
}

/// Begin play. From Stopped, launches the first run config's instance
/// (building it if needed). From Paused, resumes every live child. No-op
/// if already Playing or if the project has no run configurations.
pub fn handle_play(world: &mut World) {
    let _span = info_span!("play.start").entered();
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
            crate::live_input::arm_first_frame_capture(world);
            launch_instance(world, key, first);
        }
        PlayState::Paused => resume(world),
        PlayState::Playing => {}
    }
}

fn resume(world: &mut World) {
    broadcast_control(world, ControlEvent::Resume);
    world
        .resource_mut::<NextState<PlayState>>()
        .set(PlayState::Playing);
    info!("PIE: Resume");
}

/// Toggle between `Playing` and `Paused`, telling every live child to freeze
/// or carry on. No-op while stopped.
pub fn handle_pause(world: &mut World) {
    match world.resource::<State<PlayState>>().get() {
        PlayState::Playing => {
            broadcast_control(world, ControlEvent::Pause);
            world
                .resource_mut::<NextState<PlayState>>()
                .set(PlayState::Paused);
            info!("PIE: Pause");
        }
        PlayState::Paused => resume(world),
        PlayState::Stopped => {}
    }
}

/// Advance every live child by one fixed tick. No-op unless paused.
pub fn handle_step(world: &mut World) {
    if *world.resource::<State<PlayState>>().get() == PlayState::Paused {
        broadcast_control(world, ControlEvent::Step);
    }
}

/// Stop every instance: ask the live children to exit, then reap and
/// drop them all and discard pending builds. Returns to authoring mode.
pub fn handle_stop(world: &mut World) {
    let _span = info_span!("play.stop").entered();
    let current = world.resource::<State<PlayState>>().get().clone();

    broadcast_control(world, ControlEvent::Stop);

    let mut session = world.non_send_mut::<PieSession>();
    session.launch_modes.clear();
    session.launch_scenes.clear();
    session.serving.clear();
    for (_key, mut stage) in session.children.drain() {
        stage.kill_and_reap();
    }
    // Dropping the in-flight builds cancels them and kills their cargo.
    session.builds.clear();
    if let Some(mut last) = world.get_resource_mut::<LastGameExit>() {
        last.0 = None;
    }

    if current != PlayState::Stopped {
        world
            .resource_mut::<NextState<PlayState>>()
            .set(PlayState::Stopped);
        info!("PIE: Stop");
    }
}

/// Answer a scene root's request for its document. Its place in the instance's
/// outbox is held from here, ahead of any edit sent after this, and the text is
/// written and read back on a worker, so the editor keeps running meanwhile.
fn answer_scene_request(world: &mut World, key: &InstanceKey, request: u64, path: &str) {
    let _span = info_span!("play.serve_scene").entered();
    let launch = world
        .non_send::<PieSession>()
        .launch_scenes
        .get(key)
        .cloned();
    let prepared = crate::play_scene::prepare_scene(world, key, launch.as_ref(), path, request);
    let slots = match world.non_send::<PieSession>().children.get(key) {
        Some(ChildStage::Live { outbox, .. }) => AnswerSlots {
            files: outbox.reserve(),
            prefabs: outbox.reserve(),
            answer: outbox.reserve(),
        },
        _ => return,
    };
    let crate::play_scene::PreparedScene { job, waiting } = prepared;
    world
        .non_send_mut::<PieSession>()
        .serving
        .push(ServingScene {
            key: key.clone(),
            slots,
            waiting,
            task: write_on_worker(job),
        });
}

/// `job` on a worker, unless it holds nothing worth one or there is no pool.
fn write_on_worker(
    job: crate::play_scene::WriteJob,
) -> WorkerTask<crate::play_scene::WrittenScene> {
    match AsyncComputeTaskPool::try_get().filter(|_| !job.is_immediate()) {
        Some(pool) => WorkerTask::spawn(pool, move || job.write()),
        None => WorkerTask::done(job.write()),
    }
}

/// Finish the scene answers whose workers are done. An instance's answers finish in
/// the order they were asked for, as the game reads them, whichever is written first.
fn deliver_served_scenes(world: &mut World) {
    use crate::play_scene::{Finishing, PreparedScene, WrittenScene, finish_scene};

    let mut serving = std::mem::take(&mut world.non_send_mut::<PieSession>().serving);
    let mut waiting_on: Vec<InstanceKey> = Vec::new();
    let mut index = 0;
    while index < serving.len() {
        if waiting_on.contains(&serving[index].key) {
            index += 1;
            continue;
        }
        let written = match serving[index].task.outcome() {
            WorkerOutcome::Running => {
                waiting_on.push(serving[index].key.clone());
                index += 1;
                continue;
            }
            WorkerOutcome::Done(written) => written,
            WorkerOutcome::Panicked => {
                error!("PIE: the worker writing a scene for a game panicked");
                WrittenScene::lost(&serving[index].waiting)
            }
        };
        let ServingScene {
            key,
            slots,
            waiting,
            ..
        } = serving.remove(index);
        let mut later: Vec<&mut crate::play_scene::WaitingAnswer> = serving[index..]
            .iter_mut()
            .filter(|answer| answer.key == key)
            .map(|answer| &mut answer.waiting)
            .collect();
        match finish_scene(world, Some(&key), waiting, written, &mut later) {
            Finishing::Done(finished) => {
                slots.fill(finished);
                crate::viewport_redraw::note_game_request(world);
            }
            Finishing::Again(PreparedScene { job, waiting }) => {
                waiting_on.push(key.clone());
                serving.insert(
                    index,
                    ServingScene {
                        key,
                        slots,
                        waiting,
                        task: write_on_worker(job),
                    },
                );
                index += 1;
            }
        }
    }
    world.non_send_mut::<PieSession>().serving = serving;
}

/// Queue a control message for one instance, if it is live.
pub(crate) fn send_control_to_instance(world: &World, key: &InstanceKey, event: ControlEvent) {
    let session = world.non_send::<PieSession>();
    if let Some(ChildStage::Live { outbox, .. }) = session.children.get(key) {
        send_control_to(outbox, event);
    }
}

/// Encode a single control message and queue it for one child. An encode
/// failure logs and skips.
fn send_control_to(outbox: &GameOutbox, event: ControlEvent) {
    if let ControlEvent::NodeComponents { node, set, removed } = event {
        outbox.queue_edit(NodeEdit { node, set, removed });
        return;
    }
    let bytes = match to_bytes(&event) {
        Ok(bytes) => bytes,
        Err(err) => {
            error!("PIE: failed to encode {event:?}: {err}");
            return;
        }
    };
    outbox.queue(bytes);
}

/// Send a control message to every live child. Connecting children
/// (not yet holding a transport) are skipped.
pub(crate) fn broadcast_control(world: &mut World, event: ControlEvent) {
    let session = world.non_send::<PieSession>();
    for stage in session.children.values() {
        if let ChildStage::Live { outbox, .. } = stage {
            send_control_to(outbox, event.clone());
        }
    }
}

/// Send a control message (input, highlight, frame stream start/stop) only to the
/// focused instance's outbox. If no instance is focused, or the
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
    let Some(session) = world.get_non_send::<PieSession>() else {
        debug!("PIE: control dropped. PieSession not present.");
        return;
    };
    if let Some(ChildStage::Live { outbox, .. }) = session.children.get(&key) {
        send_control_to(outbox, event);
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

/// Whether a focused game is connected, as an operator's availability check.
pub(crate) fn focused_game_is_live(
    instances: Option<Res<PieInstances>>,
    session: Option<NonSend<PieSession>>,
) -> bool {
    let (Some(instances), Some(session)) = (instances, session) else {
        return false;
    };
    instances.focused.as_ref().is_some_and(|focused| {
        matches!(session.children.get(focused), Some(ChildStage::Live { .. }))
    })
}

/// Move focus to `key`: the instance losing it stops streaming frames nobody
/// would see. A no-op if `key` is already focused.
pub(crate) fn set_focused_instance(world: &mut World, key: InstanceKey) {
    if world.resource::<PieInstances>().focused.as_ref() == Some(&key) {
        return;
    }
    send_control_to_focused(world, ControlEvent::StopFrameStream);
    crate::live_frame::clear_stream(world);
    world.resource_mut::<PieInstances>().focused = Some(key);
}

/// The instance after `focused` in `keys`, wrapping around, or `None` with fewer
/// than two.
fn next_instance(mut keys: Vec<InstanceKey>, focused: Option<&InstanceKey>) -> Option<InstanceKey> {
    if keys.len() < 2 {
        return None;
    }
    keys.sort_by(|a, b| a.config.cmp(&b.config).then(a.instance.cmp(&b.instance)));
    let after = focused
        .and_then(|key| keys.iter().position(|held| held == key))
        .map_or(0, |at| (at + 1) % keys.len());
    keys.into_iter().nth(after)
}

fn several_instances_running(session: NonSend<PieSession>) -> bool {
    session.live_keys().len() > 1
}

/// Show one running instance in the Game panel.
#[operator(
    id = "pie.focus_instance",
    label = "Show Game Instance",
    description = "Show this running game instance in the Game panel.",
    params(
        config(String, doc = "The run configuration's name."),
        instance(i64, default = 1, doc = "Which instance of it, counting from 1."),
    )
)]
pub(crate) fn pie_focus_instance(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(config) = params.as_str("config").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    let Ok(instance) = u32::try_from(params.as_int("instance").unwrap_or(1)) else {
        return OperatorResult::Cancelled;
    };
    let key = InstanceKey { config, instance };
    commands.queue(move |world: &mut World| {
        if world
            .get_non_send::<PieSession>()
            .is_some_and(|session| session.live_keys().contains(&key))
        {
            set_focused_instance(world, key);
        }
    });
    OperatorResult::Finished
}

/// Show the next running instance in the Game panel.
#[operator(
    id = "pie.next_instance",
    label = "Next Game Instance",
    description = "Show the next running game instance in the Game panel.",
    is_available = several_instances_running
)]
pub(crate) fn pie_next_instance(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let keys = world.non_send::<PieSession>().live_keys();
        let focused = world.resource::<PieInstances>().focused.clone();
        if let Some(key) = next_instance(keys, focused.as_ref()) {
            set_focused_instance(world, key);
        }
    });
    OperatorResult::Finished
}

/// Keep focus on a connected instance: the first to connect takes it, and a
/// focused instance that has gone hands it on.
fn reconcile_focus(world: &mut World) {
    let session = world.non_send::<PieSession>();
    let mut live = session.live_keys();
    let focused = world.resource::<PieInstances>().focused.as_ref();
    // A focused instance still connecting, as one relaunched in another
    // window mode is, keeps the focus until it connects.
    if focused.is_some_and(|key| session.children.contains_key(key)) {
        return;
    }
    live.sort_by(|a, b| a.config.cmp(&b.config).then(a.instance.cmp(&b.instance)));
    if let Some(key) = live.into_iter().next() {
        set_focused_instance(world, key);
    }
}

/// Drive every instance forward each frame: advance finished builds
/// into spawned children, move connecting children to live once they
/// accept, and reap exited children. Reconciles `PlayState` to match
/// whether any child is live.
fn advance_pie_session(world: &mut World) {
    poll_builds(world);
    poll_children(world);
    reconcile_focus(world);
    reconcile_play_state(world);
    reconcile_play_status(world);
    crate::project_build::reconcile_build_status(world);
}

/// Poll each in-flight build. On success, spawn its pending instances
/// and mark the build `Done`; on failure mark it `Failed` and drop
/// pending instances. The builds map is taken with `mem::take` before
/// spawning so `spawn_instance` can read the world without aliasing.
fn poll_builds(world: &mut World) {
    let mut builds = std::mem::take(&mut world.non_send_mut::<PieSession>().builds);
    let mut launch_build_failed = false;
    let mut spawned: Vec<(InstanceKey, ChildStage, PieWindowMode, Option<LaunchScene>)> =
        Vec::new();

    for (root, state) in builds.iter_mut() {
        let BuildState::Running {
            task,
            pending,
            progress,
        } = state
        else {
            continue;
        };
        match future::block_on(future::poll_once(&mut task.task)) {
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
                    if let Some((stage, mode)) =
                        spawn_instance(world, &spawn.key, &spawn.run, &result.binary)
                    {
                        spawned.push((spawn.key, stage, mode, spawn.scene));
                    }
                }
                *state = BuildState::Done(result.binary);
            }
            Some(Err(err)) => {
                let output = progress.lock().ok().map(|p| p.full_log.clone());
                let project = root.file_name().map_or_else(
                    || root.display().to_string(),
                    |name| name.display().to_string(),
                );
                let keys: Vec<String> = pending.iter().map(|p| p.key.to_string()).collect();
                if keys.is_empty() {
                    error!("PIE: game build of {project} failed: {err}");
                } else {
                    error!(
                        "PIE: game build of {project} failed, not launching {}: {err}",
                        keys.join(", ")
                    );
                }
                let launch = !pending.is_empty();
                launch_build_failed |= launch;
                *state = BuildState::Failed { launch };
                {
                    let mut status = world.resource_mut::<crate::build_status::BuildStatus>();
                    status.state = crate::build_status::BuildState::Failed { at: Instant::now() };
                    status.outcome = Some(err.to_string());
                    status.failed_log = output;
                }
                crate::status_bar::notify_error(world, err.to_string());
            }
        }
    }

    let started = !spawned.is_empty();
    let mut session = world.non_send_mut::<PieSession>();
    session.builds = builds;
    if launch_build_failed {
        session.launch_build_failure_unseen = true;
    }
    for (key, stage, mode, scene) in spawned {
        session.launch_modes.insert(key.clone(), mode);
        session.note_launch_scene(&key, scene);
        session.children.insert(key, stage);
    }
    if started {
        crate::viewport_redraw::note_game_started(world);
    }
}

/// Poll each launched child. Connecting children that accept become
/// live; ones that fail or time out are reaped. Live children that
/// exit are reaped. The children map is taken with `mem::take` and
/// rebuilt from survivors, moving each `Child` by value.
fn poll_children(world: &mut World) {
    let mut session = world.non_send_mut::<PieSession>();
    let children = std::mem::take(&mut session.children);
    let timeout = session.connect_timeout.unwrap_or(CONNECT_TIMEOUT);

    let mut survivors: HashMap<InstanceKey, ChildStage> = HashMap::with_capacity(children.len());
    let mut failed = None;
    let mut exited = Vec::new();
    let mut connected = Vec::new();
    for (key, stage) in children {
        let was_connecting = matches!(stage, ChildStage::Connecting { .. });
        match advance_child(&key, stage, timeout) {
            ChildPoll::Running(next) => {
                if was_connecting && matches!(next, ChildStage::Live { .. }) {
                    connected.push(key.clone());
                }
                survivors.insert(key, next);
            }
            ChildPoll::Exited => exited.push(key),
            ChildPoll::Lost => {}
            ChildPoll::Failed(exit) => failed = Some(exit),
        }
    }
    if (failed.is_some() || !exited.is_empty())
        && let Some(mut last) = world.get_resource_mut::<LastGameExit>()
    {
        record_exits(&mut last, failed, &exited);
    }

    let mut session = world.non_send_mut::<PieSession>();
    session
        .launch_modes
        .retain(|key, _| survivors.contains_key(key));
    session
        .launch_scenes
        .retain(|key, _| survivors.contains_key(key));
    session
        .serving
        .retain(|serving| survivors.contains_key(&serving.key));
    if let Some(mut names) = world.get_resource_mut::<crate::play_scene::PlayAssetNames>() {
        names.retain_games(|key| survivors.contains_key(key));
    }
    world.non_send_mut::<PieSession>().children = survivors;
    for key in connected {
        greet_connected(world, &key);
    }
}

/// Tell a game that has just connected what the developer's Play settings ask
/// of it.
fn greet_connected(world: &World, key: &InstanceKey) {
    let muted = world
        .get_resource::<crate::play_settings::PlaySettings>()
        .is_some_and(|settings| settings.muted);
    if muted {
        send_control_to_instance(world, key, ControlEvent::SetMuted(true));
    }
    let paused = world
        .get_resource::<State<PlayState>>()
        .is_some_and(|state| *state.get() == PlayState::Paused);
    if paused {
        send_control_to_instance(world, key, ControlEvent::Pause);
    }
}

/// Step one child's stage by value. A connected `Connecting` becomes `Live`; a
/// child that failed to connect, took longer than `timeout` to, or has exited
/// is reaped, with why when it went away uncleanly.
fn advance_child(key: &InstanceKey, stage: ChildStage, timeout: Duration) -> ChildPoll {
    let gone = |reason: GameExitReason, output: OutputCapture| {
        ChildPoll::Failed(GameExit::new(key.clone(), reason, output))
    };
    match stage {
        ChildStage::Connecting {
            mut child,
            mut accept,
            output,
            since,
        } => {
            // A child that died before connecting will never accept;
            // reap it immediately and surface the crash.
            match child.try_wait() {
                Ok(Some(status)) => {
                    error!("PIE: {key} exited before connecting with {status}");
                    return gone(
                        GameExitReason::ExitedBeforeConnecting(exit_status_text(status)),
                        output,
                    );
                }
                Ok(None) => {}
                Err(err) => {
                    error!("PIE: {key} failed to poll while connecting: {err}");
                    child.kill().ok();
                    child.wait().ok();
                    return gone(GameExitReason::ConnectFailed(err.to_string()), output);
                }
            }
            match future::block_on(future::poll_once(&mut accept)) {
                None if since.elapsed() >= timeout => {
                    error!(
                        "PIE: {key} did not connect within {}s; does its app add JackdawPlugin, with the pie feature of jackdaw_runtime enabled?",
                        timeout.as_secs()
                    );
                    child.kill().ok();
                    child.wait().ok();
                    gone(GameExitReason::NeverConnected, output)
                }
                None => ChildPoll::Running(ChildStage::Connecting {
                    child,
                    accept,
                    output,
                    since,
                }),
                Some(Ok(transport)) => match GameOutbox::new(&transport) {
                    Ok(outbox) => {
                        info!("PIE: {key} connected");
                        ChildPoll::Running(ChildStage::Live {
                            child,
                            transport,
                            outbox,
                            output,
                        })
                    }
                    Err(err) => {
                        error!("PIE: {key} could not start its writer thread: {err}");
                        child.kill().ok();
                        child.wait().ok();
                        gone(GameExitReason::ConnectFailed(err.to_string()), output)
                    }
                },
                Some(Err(err)) => {
                    error!("PIE: {key} failed to connect: {err}");
                    child.kill().ok();
                    child.wait().ok();
                    gone(GameExitReason::ConnectFailed(err.to_string()), output)
                }
            }
        }
        ChildStage::Live {
            mut child,
            transport,
            outbox,
            output,
        } => match child.try_wait() {
            Ok(None) => ChildPoll::Running(ChildStage::Live {
                child,
                transport,
                outbox,
                output,
            }),
            Ok(Some(status)) if status.success() => {
                info!("PIE: {key} exited");
                ChildPoll::Exited
            }
            Ok(Some(status)) => {
                error!("PIE: {key} exited with {status}");
                gone(GameExitReason::Crashed(exit_status_text(status)), output)
            }
            Err(err) => {
                error!("PIE: {key} failed to poll: {err}");
                ChildPoll::Lost
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
        os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::Child,
        sync::LazyLock,
    };
    use windows_sys::Win32::System::JobObjects::*;

    pub static PIE_JOB: LazyLock<Job> = LazyLock::new(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        assert!(!job.is_null(), "Failed to create job object for PIE");
        let handle = OwnedHandle::from_raw_handle(job);

        let info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION {
            BasicLimitInformation: JOBOBJECT_BASIC_LIMIT_INFORMATION {
                LimitFlags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                    | JOB_OBJECT_LIMIT_BREAKAWAY_OK
                    | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
                ..Default::default()
            },
            ..Default::default()
        };

        let set = SetInformationJobObject(
            handle.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const info).cast(),
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        assert!(set != 0, "Failed to set job object info for PIE");

        Job(handle)
    });

    pub struct Job(pub OwnedHandle);

    impl Job {
        pub fn assign_to_process(&self, process: &Child) {
            let assigned = unsafe {
                AssignProcessToJobObject(self.0.as_raw_handle(), process.as_raw_handle())
            };
            assert!(assigned != 0, "Failed to assign process to job for PIE");
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
) -> Option<(ChildStage, PieWindowMode)> {
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
        .env_remove(EDITOR_REMOTE_PORT_ENV)
        .envs(&run.env)
        .env("BEVY_ASSET_ROOT", &cwd)
        .env("JACKDAW_PIE", &server_name)
        .args(&run.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if let Some(settings) = world.get_resource::<crate::play_settings::PlaySettings>() {
        command.envs(&settings.env).args(&settings.args);
    }

    let mode = world
        .get_resource::<PieWindowMode>()
        .copied()
        .unwrap_or_default();
    if wants_windowless_env(mode) {
        command.env("JACKDAW_PIE_WINDOWLESS", "1");
    }

    // Ask the kernel to SIGKILL this child when the editor (its parent) dies by
    // any means -- including a SIGKILL the editor can never trap, which skips
    // `Drop`. Without this, killing the editor leaves the games running.
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

    let output = world
        .get_resource::<GameOutput>()
        .cloned()
        .unwrap_or_default()
        .capture(key, &mut child);
    let accept = AsyncComputeTaskPool::get().spawn(async move { handle.accept() });

    info!("PIE: {key} launched, awaiting connection");
    Some((
        ChildStage::Connecting {
            child,
            accept,
            output,
            since: Instant::now(),
        },
        mode,
    ))
}

/// Drain `StateEvent`s from every live child and show the newest frame of the
/// focused one.
#[doc(hidden)]
pub fn drain_game_events(world: &mut World) {
    deliver_served_scenes(world);

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
        let is_focused = world.resource::<PieInstances>().focused.as_ref() == Some(&key);
        for event in events {
            match event {
                StateEvent::CursorState { grabbed, visible } => {
                    if is_focused {
                        crate::live_input::note_game_cursor_state(world, grabbed, visible);
                    }
                }
                StateEvent::RequestScene { request, path } => {
                    answer_scene_request(world, &key, request, &path);
                }
                StateEvent::SceneRequestAbandoned { path } => {
                    let message = format!(
                        "{key} stopped waiting for {path} and loaded the file, so it has no \
                         unsaved edits and may not follow further ones"
                    );
                    warn!("PIE: {message}");
                    crate::status_bar::notify_warn(world, message);
                }
                StateEvent::NodeSpawnFailed { node, reason } => {
                    warn!("PIE: {key} could not spawn the node {node}: {reason}");
                    crate::status_bar::notify_error(
                        world,
                        format!("{key} no longer matches the scene: a new entity failed: {reason}"),
                    );
                }
                StateEvent::QueryAnswer { request, answer } => {
                    crate::game_query::receive_answer(world, &key, request, answer);
                }
                StateEvent::Watched { watch, entities } => {
                    crate::game_query::receive_watched(world, &key, watch, entities);
                }
                StateEvent::OwnWindow => {
                    if is_focused {
                        world
                            .resource_mut::<crate::live_frame::LiveFrameStream>()
                            .game_has_own_window = true;
                    }
                }
            }
        }
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
mod play_status_tests {
    use super::*;

    #[test]
    fn a_launch_whose_build_failed_reads_as_failed() {
        let mut world = World::new();
        world.init_resource::<State<PlayState>>();
        let mut session = PieSession::default();
        session.builds.insert(
            PathBuf::from("project"),
            BuildState::Failed { launch: true },
        );
        world.insert_non_send(session);
        assert_eq!(play_status(&world), "failed");
    }

    #[test]
    fn a_background_build_that_failed_leaves_the_editor_stopped() {
        let mut world = World::new();
        world.init_resource::<State<PlayState>>();
        let mut session = PieSession::default();
        session.builds.insert(
            PathBuf::from("project"),
            BuildState::Failed { launch: false },
        );
        world.insert_non_send(session);
        assert_eq!(play_status(&world), "stopped");
    }
}

#[cfg(test)]
mod focus_tests {
    use super::*;

    fn key(instance: u32) -> InstanceKey {
        InstanceKey {
            config: "game".to_string(),
            instance,
        }
    }

    fn child() -> Child {
        Command::new(std::env::current_exe().expect("the test binary"))
            .arg("--list")
            .stdout(Stdio::null())
            .spawn()
            .expect("a child")
    }

    fn transport() -> IpcChannelTransport {
        let (handle, name) = serve().expect("a rendezvous");
        let game =
            std::thread::spawn(move || jackdaw_pie_protocol::connect(&name).expect("connect"));
        let editor = handle.accept().expect("accept");
        drop(game.join().expect("the game side"));
        editor
    }

    fn world_with_games(games: &[u32]) -> World {
        let mut world = World::new();
        world.init_resource::<PieInstances>();
        world.init_resource::<crate::live_frame::LiveFrameStream>();
        let mut session = PieSession::default();
        for &game in games {
            session.insert_test_live(key(game), child(), transport());
        }
        world.insert_non_send(session);
        world
    }

    #[test]
    fn a_connected_game_takes_focus_without_sending_anything() {
        let mut world = world_with_games(&[2, 1]);
        reconcile_focus(&mut world);
        assert_eq!(world.resource::<PieInstances>().focused, Some(key(1)));
    }

    #[test]
    fn focus_stays_on_a_connected_instance_and_leaves_one_that_went() {
        let mut world = world_with_games(&[1, 2]);
        world.resource_mut::<PieInstances>().focused = Some(key(2));
        reconcile_focus(&mut world);
        assert_eq!(world.resource::<PieInstances>().focused, Some(key(2)));

        let mut gone = world
            .non_send_mut::<PieSession>()
            .children
            .remove(&key(2))
            .expect("the instance");
        gone.kill_and_reap();
        reconcile_focus(&mut world);
        assert_eq!(world.resource::<PieInstances>().focused, Some(key(1)));
    }

    #[test]
    fn the_next_instance_wraps_and_needs_two() {
        assert_eq!(next_instance(vec![key(1)], Some(&key(1))), None);
        let keys = vec![key(2), key(1), key(3)];
        assert_eq!(next_instance(keys.clone(), Some(&key(1))), Some(key(2)));
        assert_eq!(next_instance(keys.clone(), Some(&key(3))), Some(key(1)));
        assert_eq!(next_instance(keys, None), Some(key(1)));
    }

    #[test]
    fn stopping_clears_focus() {
        let mut world = world_with_games(&[]);
        world.resource_mut::<PieInstances>().focused = Some(key(1));
        cleanup_session_on_stop(&mut world);
        assert_eq!(world.resource::<PieInstances>().focused, None);
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

#[cfg(test)]
mod source_change_tests {
    use super::*;

    /// A rebuild that compiles to nothing new leaves the library's mtime
    /// alone, so staleness is measured from when the build finished.
    #[test]
    fn source_older_than_the_finished_build_is_current() {
        let root =
            std::env::temp_dir().join(format!("jackdaw-source-since-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        let finished = std::time::SystemTime::now() + Duration::from_secs(1);

        assert!(source_unchanged_since(&root, finished));
        assert!(!source_unchanged_since(
            &root,
            finished - Duration::from_secs(60)
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod build_failure_tests {
    use super::*;
    use crate::build_status::BuildStatus;

    #[test]
    fn a_compile_failure_reports_its_first_diagnostic() {
        let log = "Compiling aldermoor-client\nerror[E0432]: unresolved import `foo`\n".to_string();
        let error = crate::project_build::ProjectBuildError::Compile { log: log.clone() };

        let summary = failure_summary(&error, &log);

        assert!(summary.contains("E0432"), "{summary}");
    }

    #[test]
    fn a_compile_failure_says_where_its_first_diagnostic_points() {
        let log = "error[E0560]: struct `PointLight` has no field named `shadows_enabled`\n  \
                   --> src/lib.rs:40:13\n   |\n\nerror: could not compile `game`\n"
            .to_string();
        let error = crate::project_build::ProjectBuildError::Compile { log: log.clone() };

        assert_eq!(
            failure_summary(&error, &log),
            "error[E0560]: struct `PointLight` has no field named `shadows_enabled` \
             (src/lib.rs:40:13)"
        );
    }

    #[test]
    fn a_failed_build_stays_on_screen_naming_its_error_until_the_next_build() {
        let mut world = World::new();
        world.init_resource::<BuildStatus>();
        let task = AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::default).spawn(async {
            Err(io::Error::other(
                "error[E0560]: no field (src/lib.rs:40:13)",
            ))
        });
        while !task.is_finished() {
            std::thread::yield_now();
        }
        let mut output = BuildProgress::default();
        output.push_log("  --> src/lib.rs:40:13".to_string());
        let mut session = PieSession::default();
        session.builds.insert(
            PathBuf::from("my-game"),
            BuildState::Running {
                task: task.into(),
                pending: Vec::new(),
                progress: Arc::new(Mutex::new(output)),
            },
        );
        world.insert_non_send(session);

        poll_builds(&mut world);
        world.resource_mut::<BuildStatus>().state = crate::build_status::BuildState::Failed {
            at: Instant::now() - Duration::from_secs(60),
        };
        crate::project_build::reconcile_build_status(&mut world);

        let status = world.resource::<BuildStatus>();
        assert!(
            matches!(status.state, crate::build_status::BuildState::Failed { .. }),
            "the failure has not faded"
        );
        assert!(
            status
                .outcome
                .as_deref()
                .is_some_and(|o| o.contains("E0560")),
            "and it names the error: {:?}",
            status.outcome
        );
        assert!(
            status
                .failed_log
                .as_deref()
                .is_some_and(|log| log.contains("src/lib.rs:40:13")),
            "with the compiler output kept for the Build panel"
        );

        world.non_send_mut::<PieSession>().builds.insert(
            PathBuf::from("my-game"),
            BuildState::Running {
                task: AsyncComputeTaskPool::get()
                    .spawn(std::future::pending())
                    .into(),
                pending: Vec::new(),
                progress: Arc::default(),
            },
        );
        crate::project_build::reconcile_build_status(&mut world);
        let status = world.resource::<BuildStatus>();
        assert!(
            matches!(
                status.state,
                crate::build_status::BuildState::Building { .. }
            ) && status.outcome.is_none()
                && status.failed_log.is_none(),
            "the next build replaces it"
        );
    }

    #[test]
    fn a_failure_with_no_diagnostic_falls_back_to_the_error() {
        let error = crate::project_build::ProjectBuildError::Compile { log: String::new() };

        assert_eq!(failure_summary(&error, ""), error.to_string());
    }
}

#[cfg(test)]
mod control_outbox_tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::thread;

    use jackdaw_pie_protocol::{connect, event::from_bytes};

    use super::*;

    const LARGE: usize = 8 * 1024 * 1024;
    const GUARD: Duration = Duration::from_secs(30);

    fn key() -> InstanceKey {
        InstanceKey {
            config: "Game".to_string(),
            instance: 1,
        }
    }

    /// A process standing in for the game; it exits on its own.
    fn child() -> Child {
        Command::new(std::env::current_exe().expect("the test binary"))
            .arg("--list")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("a child process")
    }

    fn link() -> (IpcChannelTransport, IpcChannelTransport) {
        let (server, name) = serve().expect("a rendezvous");
        let accept = thread::spawn(move || server.accept());
        let game = connect(&name).expect("the game connects");
        let editor = accept
            .join()
            .expect("accept ran")
            .expect("the editor accepts");
        (editor, game)
    }

    /// A multi-megabyte control message, told apart by `node`.
    fn large_event(node: u64) -> ControlEvent {
        ControlEvent::NodeComponents {
            node,
            set: Vec::new(),
            removed: vec!["x".repeat(LARGE)],
        }
    }

    /// A small control message, told apart by `node`.
    fn small_event(node: u64) -> ControlEvent {
        ControlEvent::NodeComponents {
            node,
            set: Vec::new(),
            removed: Vec::new(),
        }
    }

    fn world_with_game(editor: IpcChannelTransport) -> World {
        let mut session = PieSession::default();
        session.insert_test_live(key(), child(), editor);
        let mut world = World::new();
        world.init_resource::<State<PlayState>>();
        world.init_resource::<NextState<PlayState>>();
        world.insert_non_send(session);
        world
    }

    #[test]
    fn a_spawn_the_game_refused_shows_in_the_status_bar() {
        let (editor, mut game) = link();
        let mut world = world_with_game(editor);
        world.insert_resource(PieInstances {
            focused: Some(key()),
        });
        world.init_resource::<crate::status_bar::StatusNotice>();
        let failed = StateEvent::NodeSpawnFailed {
            node: 7,
            reason: "it does not parse".to_string(),
        };
        game.send(PieChannel::Reliable, &to_bytes(&failed).expect("encode"));

        let deadline = Instant::now() + GUARD;
        while !world
            .resource::<crate::status_bar::StatusNotice>()
            .is_active()
        {
            assert!(Instant::now() < deadline, "the user was never told");
            drain_game_events(&mut world);
            thread::sleep(Duration::from_millis(1));
        }
        let notice = world.resource::<crate::status_bar::StatusNotice>();
        assert!(
            notice.text().contains("it does not parse"),
            "{}",
            notice.text()
        );
    }

    /// Run `work` on its own thread, failing rather than hanging if it stalls.
    fn guarded<T: Send + 'static>(what: &str, work: impl FnOnce() -> T + Send + 'static) -> T {
        let (done, result) = mpsc::channel();
        thread::spawn(move || done.send(work()));
        result
            .recv_timeout(GUARD)
            .unwrap_or_else(|_| panic!("{what} did not finish in {GUARD:?}"))
    }

    /// The `node` and removed-path length of each of the next `count` messages.
    fn read_nodes(transport: &mut IpcChannelTransport, count: usize) -> Vec<(u64, usize)> {
        let deadline = Instant::now() + GUARD;
        let mut nodes = Vec::new();
        while nodes.len() < count {
            assert!(Instant::now() < deadline, "only {} arrived", nodes.len());
            for (_, bytes) in transport.drain_received() {
                let Ok(ControlEvent::NodeComponents { node, removed, .. }) = from_bytes(&bytes)
                else {
                    panic!("an unexpected message");
                };
                nodes.push((node, removed.iter().map(String::len).sum()));
            }
            thread::sleep(Duration::from_millis(1));
        }
        nodes
    }

    fn sees_close(transport: &mut IpcChannelTransport) -> bool {
        let deadline = Instant::now() + GUARD;
        while Instant::now() < deadline {
            transport.drain_received();
            if transport.peer_gone() {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn a_large_send_returns_at_once_and_arrives_in_order_with_later_ones() {
        let (editor, mut game) = link();
        let (sent, all_sent) = mpsc::channel();
        let (read, all_read) = mpsc::channel::<()>();
        let editor_side = thread::spawn(move || {
            let mut world = world_with_game(editor);
            broadcast_control(&mut world, large_event(1));
            broadcast_control(&mut world, small_event(2));
            broadcast_control(&mut world, small_event(3));
            sent.send(()).ok();
            all_read.recv().ok();
        });

        all_sent
            .recv_timeout(GUARD)
            .expect("sending returns while the game reads nothing");
        assert_eq!(
            read_nodes(&mut game, 3),
            vec![(1, LARGE), (2, 0), (3, 0)],
            "the game reads them whole, in the order they were sent"
        );
        read.send(()).ok();
        editor_side.join().expect("the editor side ran");
    }

    #[test]
    fn both_ends_sending_large_payloads_do_not_deadlock() {
        let (editor, mut game) = link();
        let game_side = thread::spawn(move || {
            let bytes = to_bytes(&large_event(7)).expect("encode");
            game.send(PieChannel::Reliable, &bytes);
            read_nodes(&mut game, 1)
        });
        let at_editor = guarded("the editor side", move || {
            let mut world = world_with_game(editor);
            broadcast_control(&mut world, large_event(8));
            let mut session = world.non_send_mut::<PieSession>();
            let Some(ChildStage::Live { transport, .. }) = session.children.get_mut(&key()) else {
                panic!("the instance is live");
            };
            read_nodes(transport, 1)
        });

        assert_eq!(at_editor, vec![(7, LARGE)]);
        let at_game = guarded("the game side", move || game_side.join().expect("it ran"));
        assert_eq!(at_game, vec![(8, LARGE)]);
    }

    #[test]
    #[cfg_attr(
        windows,
        ignore = "ipc-channel on Windows keeps a copy of a transferred sender in the receiving \
                  process, so the editor end closes only when that process exits"
    )]
    fn dropping_the_stage_returns_at_once_and_the_game_sees_the_close_after_draining() {
        let (editor, mut game) = link();
        guarded("dropping the stage", move || {
            let mut world = world_with_game(editor);
            broadcast_control(&mut world, large_event(1));
            broadcast_control(&mut world, small_event(2));
            let stage = world.non_send_mut::<PieSession>().children.remove(&key());
            drop(stage);
        });

        assert_eq!(read_nodes(&mut game, 2), vec![(1, LARGE), (2, 0)]);
        assert!(sees_close(&mut game), "no sender outlives the writer");
    }

    /// A multi-megabyte document answered while the game sends a multi-megabyte
    /// frame: neither main thread stalls on the other.
    #[test]
    fn a_large_document_is_answered_while_the_game_sends_a_large_frame() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let assets = dir.path().to_path_buf();
        let (editor, mut game) = link();
        let answered = Arc::new(AtomicBool::new(false));
        let answered_in_game = answered.clone();
        let game_side = thread::spawn(move || {
            game.send(PieChannel::Frames, &vec![0; LARGE]);
            let request = StateEvent::RequestScene {
                request: 3,
                path: "zone.bsn".to_string(),
            };
            game.send(PieChannel::Reliable, &to_bytes(&request).expect("encode"));
            let deadline = Instant::now() + GUARD;
            while Instant::now() < deadline {
                for (_, bytes) in game.drain_received() {
                    if let Ok(ControlEvent::SceneDocument { request, answer }) = from_bytes(&bytes)
                    {
                        answered.store(true, Ordering::Release);
                        return (request, answer);
                    }
                }
                thread::sleep(Duration::from_millis(1));
            }
            panic!("no answer arrived");
        });

        guarded("the editor side", move || {
            let mut world = world_with_game(editor);
            world.insert_resource(AppTypeRegistry::default());
            world.insert_resource(crate::project::ProjectRoot::new(
                assets.clone(),
                crate::project::ProjectConfig::default(),
            ));
            let mut tab = crate::scenes::SceneTab::new_untitled(1);
            tab.path = Some(assets.join("assets/zone.bsn"));
            world.insert_resource(crate::scenes::Scenes {
                tabs: vec![tab],
                active: 0,
            });
            world.insert_resource(PieInstances {
                focused: Some(key()),
            });
            let patch = jackdaw_bsn::BsnPatch::Name("x".repeat(LARGE));
            let mut ast = jackdaw_bsn::SceneBsnAst::default();
            let node = ast.create_entity_node(vec![patch]);
            ast.add_to_roots(node);
            world.insert_resource(ast);

            let deadline = Instant::now() + GUARD;
            while Instant::now() < deadline && !answered_in_game.load(Ordering::Acquire) {
                drain_game_events(&mut world);
                thread::sleep(Duration::from_millis(1));
            }
            world
        });

        let (request, answer) = guarded("the game side", move || {
            game_side.join().expect("the game side ran")
        });
        assert_eq!(request, 3);
        let jackdaw_pie_protocol::SceneAnswer::Document { text, .. } = answer else {
            panic!("the open scene answers with its document");
        };
        assert!(text.len() >= LARGE, "only {} bytes", text.len());
    }

    fn door(x: f32) -> String {
        format!(
            "#Door\n\
             jackdaw::prefab::components::Prefab\n\
             jackdaw::prefab::components::PrefabEntityId(0)\n\
             bevy_transform::components::transform::Transform {{\n    \
             translation: glam::Vec3 {{ x: {x:?}, y: 0.0, z: 0.0 }},\n\
             }}\n"
        )
    }

    /// An editor playing `zone.bsn`, which places the prefab `door.bsn`, with the
    /// game's end of the link.
    fn playing_a_scene_with_a_prefab() -> (World, IpcChannelTransport, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::create_dir_all(dir.path().join("assets")).expect("assets");
        std::fs::write(dir.path().join("assets/door.bsn"), door(1.0)).expect("a prefab");
        let scene = "#Placed\n\
                     jackdaw::prefab::components::IsA { source: \"door.bsn\", deleted: [] }\n\
                     jackdaw::prefab::components::PrefabEntityId(0)\n";
        std::fs::write(dir.path().join("assets/zone.bsn"), scene).expect("a scene");
        AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);

        let (editor, game) = link();
        let mut world = world_with_game(editor);
        world.insert_resource(AppTypeRegistry::default());
        world.insert_resource(crate::project::ProjectRoot::new(
            dir.path().to_path_buf(),
            crate::project::ProjectConfig::default(),
        ));
        let mut tab = crate::scenes::SceneTab::new_untitled(1);
        tab.path = Some(dir.path().join("assets/zone.bsn"));
        world.insert_resource(crate::scenes::Scenes {
            tabs: vec![tab],
            active: 0,
        });
        world.init_resource::<crate::prefab::PrefabAstCache>();
        world.init_resource::<crate::play_scene::PlayAssetNames>();
        world.init_resource::<crate::status_bar::StatusNotice>();
        world.insert_resource(jackdaw_bsn::parse_bsn_text(scene).expect("the scene parses"));
        (world, game, dir)
    }

    fn set_unsaved_door(world: &mut World, dir: &std::path::Path, x: f32) {
        let assets = dir.join("assets");
        let full = assets.join("door.bsn");
        let mut ast = jackdaw_bsn::parse_bsn_text(&door(x)).expect("the prefab parses");
        jackdaw_prefab::prepare_prefab_document(&mut ast, &full, &assets);
        world
            .resource_mut::<crate::prefab::PrefabAstCache>()
            .insert(&full, ast);
    }

    /// An answer whose worker writes only when released, and says when it has.
    struct HeldAnswer {
        release: mpsc::Sender<()>,
        written: mpsc::Receiver<()>,
    }

    impl HeldAnswer {
        fn write(self) {
            self.release.send(()).expect("the worker waits");
            self.written.recv_timeout(GUARD).expect("the worker writes");
            thread::sleep(Duration::from_millis(50));
        }
    }

    fn answer_when_released(world: &mut World, request: u64) -> HeldAnswer {
        let prepared = crate::play_scene::prepare_scene(world, &key(), None, "zone.bsn", request);
        let slots = reserve_answer_slots(world);
        let (release, gate) = mpsc::channel();
        let (done, written) = mpsc::channel();
        let job = prepared.job;
        let task = WorkerTask::spawn(AsyncComputeTaskPool::get(), move || {
            gate.recv().ok();
            let written = job.write();
            done.send(()).ok();
            written
        });
        world
            .non_send_mut::<PieSession>()
            .serving
            .push(ServingScene {
                key: key(),
                slots,
                waiting: prepared.waiting,
                task,
            });
        HeldAnswer { release, written }
    }

    fn reserve_answer_slots(world: &mut World) -> AnswerSlots {
        let session = world.non_send::<PieSession>();
        let Some(ChildStage::Live { outbox, .. }) = session.children.get(&key()) else {
            panic!("the instance is live");
        };
        AnswerSlots {
            files: outbox.reserve(),
            prefabs: outbox.reserve(),
            answer: outbox.reserve(),
        }
    }

    fn deliver_all(world: &mut World) {
        let deadline = Instant::now() + GUARD;
        while world.non_send::<PieSession>().answers_being_written() > 0 {
            assert!(Instant::now() < deadline, "an answer never finished");
            deliver_served_scenes(world);
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// What the game read up to and including answer `last`.
    fn read_until_answer(game: &mut IpcChannelTransport, last: u64) -> Vec<ControlEvent> {
        let deadline = Instant::now() + GUARD;
        let mut events = Vec::new();
        while Instant::now() < deadline {
            for (_, bytes) in game.drain_received() {
                let event: ControlEvent = from_bytes(&bytes).expect("a control event");
                let done = matches!(&event, ControlEvent::SceneDocument { request, .. } if *request == last);
                events.push(event);
                if done {
                    return events;
                }
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("answer {last} did not arrive; read {events:?}");
    }

    /// The door text sent with `event`, if it carries the prefab's text or sends the
    /// game back to the file.
    fn door_files(events: &[ControlEvent]) -> Vec<Option<String>> {
        events
            .iter()
            .filter_map(|event| match event {
                ControlEvent::PrefabFiles { files } => Some(files),
                _ => None,
            })
            .flatten()
            .map(|(_, text)| text.clone())
            .collect()
    }

    /// A small answer that overtakes a large one does not overtake it in the game:
    /// the prefab's text goes ahead of the first answer that reaches it.
    #[test]
    fn a_prefab_two_answers_reach_goes_with_the_first_whichever_is_written_first() {
        let (mut world, mut game, dir) = playing_a_scene_with_a_prefab();
        set_unsaved_door(&mut world, dir.path(), 9.0);
        let first = answer_when_released(&mut world, 1);
        let second = answer_when_released(&mut world, 2);

        second.write();
        deliver_served_scenes(&mut world);
        first.write();
        deliver_all(&mut world);

        let events = read_until_answer(&mut game, 2);
        let order: Vec<&str> = events
            .iter()
            .map(|event| match event {
                ControlEvent::PrefabFiles { .. } => "prefab",
                ControlEvent::SceneDocument { request: 1, .. } => "first",
                ControlEvent::SceneDocument { request: 2, .. } => "second",
                _ => "other",
            })
            .collect();
        assert_eq!(order, ["prefab", "first", "second"]);
    }

    /// Answers written out of order leave the editor recording the text the game
    /// ends on, so a later answer has nothing to repeat.
    #[test]
    fn the_prefab_text_a_game_ends_on_is_the_one_recorded() {
        let (mut world, mut game, dir) = playing_a_scene_with_a_prefab();
        set_unsaved_door(&mut world, dir.path(), 7.0);
        let first = answer_when_released(&mut world, 1);
        set_unsaved_door(&mut world, dir.path(), 8.0);
        let second = answer_when_released(&mut world, 2);

        second.write();
        deliver_served_scenes(&mut world);
        first.write();
        deliver_all(&mut world);

        let texts = door_files(&read_until_answer(&mut game, 2));
        let last = texts.last().expect("the text").as_deref().expect("text");
        assert!(last.contains("8.0"), "{last}");
        let again = crate::play_scene::serve_scene(&mut world, &key(), None, "zone.bsn");
        assert!(again.prefabs.is_none(), "{:?}", again.prefabs);
    }

    /// A prefab saved between two answers goes back to its file with the later one,
    /// though the earlier one, not yet finished then, gave the game its text.
    #[test]
    fn a_prefab_saved_between_two_answers_goes_back_to_its_file() {
        let (mut world, mut game, dir) = playing_a_scene_with_a_prefab();
        set_unsaved_door(&mut world, dir.path(), 9.0);
        let first = answer_when_released(&mut world, 1);
        crate::prefab::operators::save_prefab_to_disk(
            &mut world,
            &dir.path().join("assets/door.bsn"),
        )
        .expect("the prefab saves");
        let second = answer_when_released(&mut world, 2);

        first.write();
        second.write();
        deliver_all(&mut world);

        let texts = door_files(&read_until_answer(&mut game, 2));
        assert_eq!(texts.len(), 2, "{texts:?}");
        assert!(texts[0].is_some());
        assert_eq!(texts[1], None);
    }

    /// A game that exits during a write counts as holding nothing, so a new game
    /// under the same key gets the prefab's text.
    #[test]
    fn a_game_gone_during_a_write_leaves_nothing_given_to_the_next() {
        let (mut world, _game, dir) = playing_a_scene_with_a_prefab();
        set_unsaved_door(&mut world, dir.path(), 9.0);
        let held = answer_when_released(&mut world, 1);

        let deadline = Instant::now() + GUARD;
        while world.non_send::<PieSession>().is_running(&key()) {
            assert!(Instant::now() < deadline, "the child never exited");
            poll_children(&mut world);
            thread::sleep(Duration::from_millis(5));
        }
        held.write();
        deliver_served_scenes(&mut world);

        assert_eq!(world.non_send::<PieSession>().answers_being_written(), 0);
        let given = world
            .resource::<crate::play_scene::PlayAssetNames>()
            .given_prefabs(&key());
        assert!(given.is_empty(), "{given:?}");
    }

    /// A worker that panics costs the game its unsaved edits, not the editor its
    /// main thread: the game loads the file and the user is told.
    #[test]
    fn a_worker_that_panics_leaves_the_game_on_the_file() {
        let (mut world, mut game, _dir) = playing_a_scene_with_a_prefab();
        let prepared = crate::play_scene::prepare_scene(&mut world, &key(), None, "zone.bsn", 1);
        let slots = reserve_answer_slots(&mut world);
        drop(prepared.job);
        let task = WorkerTask::spawn(AsyncComputeTaskPool::get(), || {
            panic!("a worker that panics, as a test expects")
        });
        world
            .non_send_mut::<PieSession>()
            .serving
            .push(ServingScene {
                key: key(),
                slots,
                waiting: prepared.waiting,
                task,
            });
        deliver_all(&mut world);

        let events = read_until_answer(&mut game, 1);
        let Some(ControlEvent::SceneDocument { answer, .. }) = events.last() else {
            panic!("no answer: {events:?}");
        };
        assert!(
            matches!(answer, jackdaw_pie_protocol::SceneAnswer::UseFile { path, .. } if path == "zone.bsn"),
            "{answer:?}"
        );
        assert!(
            world
                .resource::<crate::status_bar::StatusNotice>()
                .text()
                .contains("zone.bsn")
        );
    }

    /// With Realtime off, frames keep coming for as long as an answer is written,
    /// and a little longer from when it goes out, for what the game asks next.
    #[test]
    fn frames_keep_coming_while_an_answer_is_written_and_just_after() {
        use bevy::ecs::system::RunSystemOnce;
        let is_busy = |world: &mut World| {
            world
                .run_system_once(
                    |busy: crate::viewport_redraw::Busy, time: Res<Time<Real>>| {
                        busy.any(time.elapsed())
                    },
                )
                .expect("it runs")
        };
        let (mut world, _game, dir) = playing_a_scene_with_a_prefab();
        world.init_resource::<Time<Real>>();
        crate::viewport_redraw::init_game_request_window(&mut world);
        set_unsaved_door(&mut world, dir.path(), 9.0);
        assert!(!is_busy(&mut world), "nothing is asked for yet");

        let held = answer_when_released(&mut world, 1);
        assert!(is_busy(&mut world), "the answer is being written");
        held.write();
        deliver_all(&mut world);
        assert!(is_busy(&mut world), "the game may ask for more");
    }

    #[test]
    fn stopping_with_a_stalled_game_returns_at_once() {
        let (editor, game) = link();
        guarded("stopping", move || {
            let mut world = world_with_game(editor);
            broadcast_control(&mut world, large_event(1));
            stop_instance(&mut world, &key());
            assert!(!world.non_send::<PieSession>().is_running(&key()));
        });
        drop(game);
    }
}

#[cfg(test)]
mod transport_tests {
    use std::thread;

    use jackdaw_pie_protocol::{connect, event::from_bytes};

    use super::*;

    const GUARD: Duration = Duration::from_secs(30);

    fn key() -> InstanceKey {
        InstanceKey {
            config: "Game".to_string(),
            instance: 1,
        }
    }

    /// A process standing in for the game; it exits on its own.
    fn child() -> Child {
        Command::new(std::env::current_exe().expect("the test binary"))
            .arg("--list")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("a child process")
    }

    fn link() -> (IpcChannelTransport, IpcChannelTransport) {
        let (server, name) = serve().expect("a rendezvous");
        let accept = thread::spawn(move || server.accept());
        let game = connect(&name).expect("the game connects");
        let editor = accept
            .join()
            .expect("accept ran")
            .expect("the editor accepts");
        (editor, game)
    }

    fn world_in(state: PlayState, session: PieSession) -> World {
        let mut world = World::new();
        world.insert_resource(State::new(state));
        world.init_resource::<NextState<PlayState>>();
        world.insert_non_send(session);
        world
    }

    fn phase_of(world: &World) -> PlayPhase {
        play_phase(
            world.resource::<State<PlayState>>().get(),
            Some(world.non_send::<PieSession>()),
        )
    }

    fn controls(
        play: bool,
        pause: bool,
        step: bool,
        stop: bool,
        restart: bool,
    ) -> TransportControls {
        TransportControls {
            play,
            pause,
            step,
            stop,
            restart,
        }
    }

    #[test]
    fn each_phase_offers_its_own_controls() {
        assert_eq!(
            PlayPhase::Stopped.controls(),
            controls(true, false, false, false, false),
            "stopped: only Play"
        );
        assert_eq!(
            PlayPhase::Building.controls(),
            controls(false, false, false, true, false),
            "building: only Stop, which cancels"
        );
        assert_eq!(
            PlayPhase::Playing.controls(),
            controls(true, true, false, true, true),
            "playing: Play does nothing but stays lit, Step waits for a pause"
        );
        assert_eq!(
            PlayPhase::Paused.controls(),
            controls(true, true, true, true, true),
            "paused: Play resumes and Step steps"
        );
        assert_eq!(PlayPhase::Stopped.play_caption(), "Play");
        assert_eq!(PlayPhase::Building.play_caption(), "Building...");
        assert_eq!(PlayPhase::Playing.play_caption(), "Play");
        assert_eq!(PlayPhase::Paused.play_caption(), "Resume");
    }

    #[test]
    fn the_phase_follows_the_session_and_the_play_state() {
        let world = world_in(PlayState::Stopped, PieSession::default());
        assert_eq!(phase_of(&world), PlayPhase::Stopped);

        let mut session = PieSession::default();
        session.insert_test_pending_build(key());
        let world = world_in(PlayState::Stopped, session);
        assert_eq!(phase_of(&world), PlayPhase::Building, "waiting on a build");

        let mut session = PieSession::default();
        session.insert_test_connecting(key(), child());
        let mut world = world_in(PlayState::Stopped, session);
        assert_eq!(phase_of(&world), PlayPhase::Building, "waiting to connect");
        handle_stop(&mut world);
        assert_eq!(
            phase_of(&world),
            PlayPhase::Stopped,
            "Stop cancels a launch"
        );

        let (editor, _game) = link();
        let mut session = PieSession::default();
        session.insert_test_live(key(), child(), editor);
        let world = world_in(PlayState::Playing, session);
        assert_eq!(phase_of(&world), PlayPhase::Playing);
        assert_eq!(
            play_config(world.non_send::<PieSession>(), None),
            Some("Game".to_string())
        );

        let world = world_in(PlayState::Paused, PieSession::default());
        assert_eq!(phase_of(&world), PlayPhase::Paused);
    }

    #[test]
    fn dropping_a_build_cancels_it() {
        let task: GameBuildTask = AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new)
            .spawn(std::future::pending::<io::Result<GameBuildResult>>())
            .into();
        let cancel = Arc::clone(&task.cancel);
        assert!(!cancel.load(Ordering::Relaxed));
        let mut session = PieSession::default();
        session.builds.insert(
            PathBuf::from("game"),
            BuildState::Running {
                task,
                pending: Vec::new(),
                progress: Arc::default(),
            },
        );
        let mut world = world_in(PlayState::Stopped, session);
        handle_stop(&mut world);
        assert!(
            cancel.load(Ordering::Relaxed),
            "Stop tells the build to kill cargo"
        );
    }

    #[test]
    fn a_game_that_exits_takes_the_controls_back_to_stopped() {
        let (editor, _game) = link();
        let mut session = PieSession::default();
        session.insert_test_live(key(), child(), editor);
        let mut world = world_in(PlayState::Playing, session);

        let deadline = Instant::now() + GUARD;
        while world.non_send::<PieSession>().is_running(&key()) {
            assert!(Instant::now() < deadline, "the game never exited");
            poll_children(&mut world);
            thread::sleep(Duration::from_millis(5));
        }
        reconcile_play_state(&mut world);

        assert!(matches!(
            world.resource::<NextState<PlayState>>(),
            NextState::Pending(PlayState::Stopped)
        ));
        world.insert_resource(State::new(PlayState::Stopped));
        assert_eq!(phase_of(&world), PlayPhase::Stopped);
    }

    #[test]
    fn pause_step_and_an_edit_made_while_paused_reach_the_game() {
        let (editor, mut game) = link();
        let mut session = PieSession::default();
        session.insert_test_live(key(), child(), editor);
        let mut world = world_in(PlayState::Playing, session);

        handle_step(&mut world);
        handle_pause(&mut world);
        world.insert_resource(State::new(PlayState::Paused));
        handle_step(&mut world);
        send_control_to_instance(
            &world,
            &key(),
            ControlEvent::NodeComponents {
                node: 7,
                set: Vec::new(),
                removed: vec!["game::Spin".to_string()],
            },
        );
        handle_pause(&mut world);

        let deadline = Instant::now() + GUARD;
        let mut events = Vec::new();
        while events.len() < 4 {
            assert!(Instant::now() < deadline, "only {events:?} arrived");
            for (_, bytes) in game.drain_received() {
                events.push(from_bytes::<ControlEvent>(&bytes).expect("a control event"));
            }
            thread::sleep(Duration::from_millis(1));
        }
        let transport: Vec<&ControlEvent> = events
            .iter()
            .filter(|event| !matches!(event, ControlEvent::NodeComponents { .. }))
            .collect();
        assert_eq!(
            transport,
            vec![
                &ControlEvent::Pause,
                &ControlEvent::Step,
                &ControlEvent::Resume
            ],
            "a step while playing sends nothing; one while paused sends one Step"
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, ControlEvent::NodeComponents { node: 7, .. })),
            "the edit made while paused was sent"
        );
        assert!(matches!(
            world.resource::<NextState<PlayState>>(),
            NextState::Pending(PlayState::Playing)
        ));
    }
}

#[cfg(all(test, unix))]
mod game_exit_tests {
    use super::*;

    fn key() -> InstanceKey {
        InstanceKey {
            config: "Game".to_string(),
            instance: 1,
        }
    }

    fn shell(script: &str) -> Child {
        Command::new("sh")
            .args(["-c", script])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("a shell")
    }

    fn connecting(mut child: Child, since: Instant) -> ChildStage {
        let accept = AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new)
            .spawn(std::future::pending::<io::Result<IpcChannelTransport>>());
        let output = GameOutput::default().capture(&key(), &mut child);
        ChildStage::Connecting {
            child,
            accept,
            output,
            since,
        }
    }

    /// Poll `stage` until it is reaped.
    fn reap(mut stage: ChildStage, timeout: Duration) -> Option<GameExit> {
        let until = Instant::now() + Duration::from_secs(10);
        loop {
            match advance_child(&key(), stage, timeout) {
                ChildPoll::Running(next) => stage = next,
                ChildPoll::Failed(exit) => return Some(exit),
                ChildPoll::Exited | ChildPoll::Lost => return None,
            }
            assert!(Instant::now() < until, "the child was never reaped");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn exit_status_names_the_code_or_the_signal() {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(
            exit_status_text(std::process::ExitStatus::from_raw(3 << 8)),
            "code 3"
        );
        assert_eq!(
            exit_status_text(std::process::ExitStatus::from_raw(9)),
            "signal 9"
        );
    }

    #[test]
    fn a_game_that_exits_before_connecting_records_its_code_and_stderr() {
        let stage = connecting(
            shell("for i in 1 2 3 4 5 6 7 8 9 10; do echo line $i >&2; done; exit 3"),
            Instant::now(),
        );
        let exit = reap(stage, Duration::from_secs(30)).expect("an exit is recorded");
        assert_eq!(exit.key, key());
        assert_eq!(
            exit.reason,
            GameExitReason::ExitedBeforeConnecting("code 3".to_string())
        );
        assert_eq!(
            exit.stderr.last().map(String::as_str),
            Some("line 10"),
            "the lines written just before the exit are kept"
        );
        assert_eq!(exit.stderr.len(), EXIT_STDERR_LINES);
        assert_eq!(exit.stderr.first().map(String::as_str), Some("line 3"));
    }

    fn crash_of(instance: u32) -> GameExit {
        GameExit {
            key: InstanceKey {
                config: "Game".to_string(),
                instance,
            },
            reason: GameExitReason::Crashed("code 1".to_string()),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn another_game_exiting_cleanly_keeps_the_recorded_crash() {
        let mut last = LastGameExit(Some(crash_of(1)));
        let other = crash_of(2).key;
        record_exits(&mut last, None, std::slice::from_ref(&other));
        assert!(last.0.is_some());
        record_exits(&mut last, None, &[crash_of(1).key]);
        assert!(
            last.0.is_none(),
            "the crashed game itself exiting cleanly clears it"
        );
        record_exits(&mut last, Some(crash_of(2)), &[crash_of(1).key]);
        assert_eq!(last.0.map(|exit| exit.key), Some(other));
    }

    #[test]
    fn a_game_still_unconnected_at_the_timeout_is_stopped_and_recorded() {
        let stage = connecting(shell("sleep 600"), Instant::now());
        let exit = reap(stage, Duration::from_millis(20)).expect("an exit is recorded");
        assert_eq!(exit.reason, GameExitReason::NeverConnected);
    }

    #[test]
    fn an_unconnected_game_within_the_timeout_keeps_running() {
        let stage = connecting(shell("sleep 600"), Instant::now());
        let ChildPoll::Running(mut stage) = advance_child(&key(), stage, Duration::from_secs(30))
        else {
            panic!("the child was reaped");
        };
        stage.kill_and_reap();
    }
}
