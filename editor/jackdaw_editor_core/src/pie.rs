//! Play-In-Editor runtime.
//!
//! Builds the project with plain `cargo build`, launches the game's own
//! executable as a child process, and drives it over an `ipc-channel`
//! connection. Children stream `StateEvent`s back and respond to
//! `ControlEvent`s (Pause / Resume / Stop). Stop reaps the children; the
//! authored scene is never mutated. Messages to a child leave on its own
//! writer thread (`GameOutbox`), so the main thread never waits on a game.
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

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use jackdaw_api::pie::PlayState;
use jackdaw_api::prelude::*;
use jackdaw_pie_protocol::event::{from_bytes, to_bytes};
use jackdaw_pie_protocol::manifest::RunConfig;
use jackdaw_pie_protocol::{
    ControlEvent, IpcChannelTransport, PieChannel, PieTransport, StateEvent, serve,
};

use crate::ext_build::BuildProgress;
use crate::pie_mirror::{PieInstances, PieViewMode};
use crate::pie_outbox::{GameOutbox, NodeEdit};
use crate::play_scene::LaunchScene;
use crate::project_build::BuildLoad;
use crate::project_build::shim::ShimSpec;
use crate::run_config::RunConfigs;

/// How many trailing stderr lines to keep from a game process, so a
/// crash can be reported without buffering unbounded output.
const STDERR_TAIL_LINES: usize = 40;

/// How long to wait for a launched child to connect back before giving
/// up on it.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Marker for the toolbar transport buttons. `PiePlugin` installs
/// an `On<Add<PieButton>>` observer that wires each button's
/// `PointerClick` to the corresponding handler.
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
    /// The child is connected and running. Messages to it go through `outbox`;
    /// `transport` receives from it.
    Live {
        child: Child,
        transport: IpcChannelTransport,
        outbox: GameOutbox,
        stderr_tail: StderrTail,
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
        task: Task<io::Result<GameBuildResult>>,
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

/// One instance waiting for its build to finish before spawning.
pub(crate) struct PendingSpawn {
    key: InstanceKey,
    run: RunConfig,
    scene: Option<LaunchScene>,
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
    fn launch_failed(&self) -> bool {
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

/// What play-in-editor is doing: `building` while a launch waits on its
/// cargo build, `running` once a game process is up (a paused game
/// included), `failed` when a launch's build did not compile, `stopped`
/// otherwise.
pub fn play_status(world: &World) -> &'static str {
    let session = world.get_non_send::<PieSession>();
    let playing = world
        .get_resource::<State<PlayState>>()
        .is_some_and(|state| *state.get() != PlayState::Stopped);
    if playing || session.is_some_and(|session| !session.children.is_empty()) {
        return "running";
    }
    if session.is_some_and(PieSession::is_launching) {
        return "building";
    }
    if session.is_some_and(PieSession::launch_failed) {
        return "failed";
    }
    "stopped"
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
            .init_resource::<crate::play_scene::PlayAssetNames>()
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
                    crate::play_scene::forget_asset_names,
                    crate::play_overlay::clear_play_sidecars,
                ),
            )
            .add_systems(
                OnExit(PlayState::Stopped),
                (
                    crate::live_edits_ui::review_handoff_on_replay,
                    crate::play_overlay::clear_play_sidecars,
                ),
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
        .register_operator::<ProjectTogglePrebuildOp>()
        .register_operator::<ProjectToggleSaveBeforePlayOp>()
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
    id = "pie.play", writes_disk = true,
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

/// Turn saving the modified scenes before Play starts the game on or off.
/// On by default, so the game loads what the editor shows.
#[operator(
    id = "project.toggle_save_before_play",
    writes_disk = true,
    label = "Toggle Save Before Play",
    description = "Turn saving modified scenes before Play starts the game on or off."
)]
pub(crate) fn project_toggle_save_before_play(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let enabled = {
            let mut settings = world.resource_mut::<crate::project_build::EditorBuildSettings>();
            settings.save_before_play = !settings.save_before_play;
            settings.save_before_play
        };
        crate::project_build::persist_project_build_settings(world);
        info!(
            "PIE: save before play {}",
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
        self.children.insert(
            key,
            ChildStage::Live {
                child,
                transport,
                outbox,
                stderr_tail: StderrTail::default(),
            },
        );
    }

    /// Add an instance that is still connecting and never connects.
    pub(crate) fn insert_test_connecting(&mut self, key: InstanceKey, child: Child) {
        let accept = AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new)
            .spawn(std::future::pending::<io::Result<IpcChannelTransport>>());
        self.children.insert(
            key,
            ChildStage::Connecting {
                child,
                accept,
                stderr_tail: StderrTail::default(),
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
                task,
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
    trigger: On<Add<PieButton>>,
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
        .observe(move |_: On<PointerClick>, mut commands: Commands| {
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

    if let Err(reason) = save_before_play(world) {
        warn!("PIE: not playing: {reason}");
        crate::status_bar::notify_error(world, reason);
        return;
    }
    crate::play_overlay::notice_unsaved_model_settings(world);
    let scene = crate::play_scene::launch_scene(world, &run);
    let run = run_with_open_scene(world, run);

    // If the game binary is built AND still current, spawn from it
    // immediately. A source edit since the last build invalidates the
    // cache so Play runs the new code: each Play is a fresh process, so
    // rebuilding is always safe.
    if let Some(BuildState::Done(path)) = world.non_send::<PieSession>().builds.get(&root) {
        if source_is_current(&root, path) {
            let path = path.clone();
            if let Some((stage, mode)) = spawn_instance(world, &key, &run, &path) {
                let mut session = world.non_send_mut::<PieSession>();
                session.launch_modes.insert(key.clone(), mode);
                session.note_launch_scene(&key, scene);
                session.children.insert(key, stage);
                crate::viewport_redraw::note_game_started(world);
            }
            return;
        }
        info!("PIE: project source changed; rebuilding before Play");
        world.non_send_mut::<PieSession>().builds.remove(&root);
    }

    let spec = match crate::project_build::shim_spec_for_project(&root) {
        Ok(spec) => spec,
        Err(error) => {
            crate::project_build::report_unbuildable(world, &root, &error);
            return;
        }
    };

    // Join an in-flight build's pending list, or start a new build.
    let mut session = world.non_send_mut::<PieSession>();
    match session.builds.get_mut(&root) {
        Some(BuildState::Running { pending, .. }) => {
            pending.push(PendingSpawn { key, run, scene });
        }
        Some(BuildState::Done(_)) => unreachable!("handled above"),
        Some(BuildState::Failed { .. }) | None => {
            info!("PIE: building the game for {key}");
            let (task, progress) = spawn_game_build(&root, spec, BuildLoad::Foreground);
            session.builds.insert(
                root,
                BuildState::Running {
                    task,
                    pending: vec![PendingSpawn { key, run, scene }],
                    progress,
                },
            );
        }
    }
}

/// The scene a game loads as its main one, inside the asset folder: what the
/// game template loads and what the editor opens for a project with no
/// remembered tabs. No project setting names another.
pub(crate) const MAIN_SCENE: &str = "scene.bsn";

/// `run` launching the scene open in front in place of the game's main scene:
/// the game loads `JACKDAW_PIE_SCENE` wherever it asks for
/// `JACKDAW_PIE_REPLACES`, both paths inside the asset folder. Nothing is
/// named when the main scene is in front, when the scene in front has no file
/// in the asset folder, or when the run configuration already names a scene.
pub fn run_with_open_scene(world: &World, mut run: RunConfig) -> RunConfig {
    let Some(scene) = open_scene_asset_path(world) else {
        return run;
    };
    if scene == MAIN_SCENE || run.env.contains_key("JACKDAW_PIE_SCENE") {
        return run;
    }
    run.env.insert("JACKDAW_PIE_SCENE".to_string(), scene);
    run.env
        .entry("JACKDAW_PIE_REPLACES".to_string())
        .or_insert_with(|| MAIN_SCENE.to_string());
    run
}

pub(crate) fn open_scene_asset_path(world: &World) -> Option<String> {
    let scenes = world.get_resource::<crate::scenes::Scenes>()?;
    let tab = scenes.tabs.get(scenes.active)?;
    if tab.kind != crate::scenes::TabKind::Scene {
        return None;
    }
    world
        .get_resource::<crate::project::ProjectRoot>()?
        .to_asset_path(tab.path.as_deref()?)
}

/// Save the modified scenes the game may load, unless the project turned
/// that off. A scene with no file is served to the game unsaved. So is one a
/// save refuses for naming an asset by an absolute path, which warns. A prefab
/// that does not save stops Play.
fn save_before_play(world: &mut World) -> Result<(), String> {
    if !world
        .get_resource::<crate::project_build::EditorBuildSettings>()
        .is_none_or(|settings| settings.save_before_play)
    {
        return Ok(());
    }
    let (absolute, failures): (Vec<_>, Vec<_>) =
        crate::scenes::operators::save_modified_scenes(world)
            .into_iter()
            .partition(|failure| {
                failure.kind == crate::scenes::TabKind::Scene
                    && failure
                        .message
                        .contains(crate::scene_io::asset_fields::ABSOLUTE_PATH_REFUSAL)
            });
    let messages = |failures: Vec<crate::scenes::operators::TabSaveFailure>| {
        failures
            .into_iter()
            .map(|failure| failure.message)
            .collect::<Vec<_>>()
            .join("; ")
    };
    if !failures.is_empty() {
        return Err(format!(
            "Not playing: could not save {}",
            messages(failures)
        ));
    }
    if !absolute.is_empty() {
        let message = format!("Playing unsaved: could not save {}", messages(absolute));
        crate::status_bar::notify_warn(world, message);
    }
    Ok(())
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
        let result = crate::project_build::build_project_binary_with_load(
            &spec,
            &jackdaw_dir,
            load,
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
    (task, progress)
}

/// Stop one instance: ask a live child to exit, reap it, and drop it.
/// Returns to authoring mode once no children remain.
pub(crate) fn stop_instance(world: &mut World, key: &InstanceKey) {
    if let Some(mut names) = world.get_resource_mut::<crate::play_scene::PlayAssetNames>() {
        names.retain_games(|held| held != key);
    }
    let mut session = world.non_send_mut::<PieSession>();
    session.launch_modes.remove(key);
    session.launch_scenes.remove(key);
    let Some(mut stage) = session.children.remove(key) else {
        return;
    };
    if let ChildStage::Live { outbox, .. } = &stage {
        send_control_to(outbox, ControlEvent::Stop);
    }
    stage.kill_and_reap();
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
    session.launch_modes.clear();
    session.launch_scenes.clear();
    for (_key, mut stage) in session.children.drain() {
        stage.kill_and_reap();
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

/// Answer a scene root's request for its document. The reply goes through the
/// instance's outbox, ahead of any edit sent after this.
fn answer_scene_request(world: &mut World, key: &InstanceKey, request: u64, path: &str) {
    let launch = world
        .non_send::<PieSession>()
        .launch_scenes
        .get(key)
        .cloned();
    let served = crate::play_scene::serve_scene(world, key, launch.as_ref(), path);
    crate::viewport_redraw::note_game_request(world);
    for ahead in [served.files, served.prefabs].into_iter().flatten() {
        send_control_to_instance(world, key, ahead);
    }
    send_control_to_instance(
        world,
        key,
        ControlEvent::SceneDocument {
            request,
            answer: served.answer,
        },
    );
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

/// Send a control message (live edits, frame stream start/stop) only to the
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

    let skip_ids = crate::scene_io::doc_skip_type_ids();

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
    // The values came from the running game: send what was pending before
    // the write, then count the write itself as already sent.
    crate::play_sync::sync_document_to_game(world);
    cmd.execute(world);
    world.resource_mut::<CommandHistory>().push_executed(cmd);
    crate::play_sync::absorb_document_changes(world);
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
    crate::project_build::reconcile_build_status(world);
}

/// Poll each in-flight build. On success, spawn its pending instances
/// and mark the build `Done`; on failure mark it `Failed` and drop
/// pending instances. The builds map is taken with `mem::take` before
/// spawning so `spawn_instance` can read the world without aliasing.
fn poll_builds(world: &mut World) {
    let mut builds = std::mem::take(&mut world.non_send_mut::<PieSession>().builds);
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
                *state = BuildState::Failed {
                    launch: !pending.is_empty(),
                };
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
    let children = std::mem::take(&mut world.non_send_mut::<PieSession>().children);

    let mut survivors: HashMap<InstanceKey, ChildStage> = HashMap::with_capacity(children.len());
    for (key, stage) in children {
        if let Some(next) = advance_child(&key, stage) {
            survivors.insert(key, next);
        }
    }

    let mut session = world.non_send_mut::<PieSession>();
    session
        .launch_modes
        .retain(|key, _| survivors.contains_key(key));
    session
        .launch_scenes
        .retain(|key, _| survivors.contains_key(key));
    if let Some(mut names) = world.get_resource_mut::<crate::play_scene::PlayAssetNames>() {
        names.retain_games(|key| survivors.contains_key(key));
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
                Some(Ok(transport)) => match GameOutbox::new(&transport) {
                    Ok(outbox) => {
                        info!("PIE: {key} connected");
                        Some(ChildStage::Live {
                            child,
                            transport,
                            outbox,
                            stderr_tail,
                        })
                    }
                    Err(err) => {
                        error!("PIE: {key} could not start its writer thread: {err}");
                        child.kill().ok();
                        child.wait().ok();
                        None
                    }
                },
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
            outbox,
            stderr_tail,
        } => match child.try_wait() {
            Ok(None) => Some(ChildStage::Live {
                child,
                transport,
                outbox,
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
        .envs(&run.env)
        .env("BEVY_ASSET_ROOT", &cwd)
        .env("JACKDAW_PIE", &server_name)
        .args(&run.args)
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
    Some((
        ChildStage::Connecting {
            child,
            accept,
            stderr_tail,
            since: Instant::now(),
        },
        mode,
    ))
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

/// Drain `StateEvent`s from every live child. Events for every instance are
/// always accumulated into that instance's [`InstanceBuffer`](crate::pie_mirror::InstanceBuffer) regardless of
/// view mode, so the buffers always hold current game state and a Scene->Live
/// toggle re-projects fresh data. Additionally, events from the focused instance
/// are projected into the preview ECS via `project_event`, but only in Live mode.
pub(crate) fn drain_game_events(world: &mut World) {
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
            if let StateEvent::RequestScene { request, path } = &event {
                answer_scene_request(world, &key, *request, path);
                continue;
            }
            if let StateEvent::SceneRequestAbandoned { path } = &event {
                let message = format!(
                    "{key} stopped waiting for {path} and loaded the file, so it has no unsaved \
                     edits and may not follow further ones"
                );
                warn!("PIE: {message}");
                crate::status_bar::notify_warn(world, message);
                continue;
            }
            if matches!(event, StateEvent::OwnWindow) {
                if is_focused {
                    world
                        .resource_mut::<crate::live_frame::LiveFrameStream>()
                        .game_has_own_window = true;
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
                task,
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
                task: AsyncComputeTaskPool::get().spawn(std::future::pending()),
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
    /// state: neither main thread stalls on the other.
    #[test]
    fn a_large_document_is_answered_while_the_game_streams_a_large_state() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let assets = dir.path().to_path_buf();
        let (editor, mut game) = link();
        let game_side = thread::spawn(move || {
            let state = StateEvent::ComponentChanged {
                entity: 1,
                type_path: "x".repeat(LARGE),
                value: serde_json::Value::Null,
            };
            game.send(PieChannel::Reliable, &to_bytes(&state).expect("encode"));
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
                buffers: HashMap::default(),
                focused: Some(key()),
            });
            world.insert_resource(PieViewMode::Scene);
            world.init_resource::<crate::pie_projection::PieProjection>();
            let patch = jackdaw_bsn::BsnPatch::Name("x".repeat(LARGE));
            let mut ast = jackdaw_bsn::SceneBsnAst::default();
            let node = ast.create_entity_node(vec![patch]);
            ast.add_to_roots(node);
            world.insert_resource(ast);

            let deadline = Instant::now() + GUARD;
            while Instant::now() < deadline
                && !world
                    .resource::<PieInstances>()
                    .buffers
                    .contains_key(&key())
            {
                drain_game_events(&mut world);
                thread::sleep(Duration::from_millis(1));
            }
            for _ in 0..100 {
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
