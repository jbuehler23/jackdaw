//! Play-In-Editor runtime.
//!
//! Builds a run configuration's binary with the `jackdaw_runtime/pie`
//! feature, launches it as a child process, and drives it over an
//! `ipc-channel` connection. Children stream `StateEvent`s back and
//! respond to `ControlEvent`s (Pause / Resume / Stop). Stop reaps the
//! children; the authored scene is never mutated.
//!
//! Instances are keyed by [`InstanceKey`] (config label plus 1-based
//! instance number). Builds are deduped by
//! [`BuildSpec`]: several instances of the
//! same config wait on one build and spawn together when it finishes.

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
use crate::ext_build::{BuildProgress, BuildSpec};
use crate::pie_mirror::{PieInstances, PieViewMode};
use crate::run_config::{CargoMeta, RunConfigs, resolve_build_spec};

/// How many trailing stderr lines to keep from a game process, so a
/// crash can be reported without buffering unbounded output.
const STDERR_TAIL_LINES: usize = 40;

/// How long to wait for a launched child to connect back before giving
/// up on it. A child that runs but never connects usually means its
/// build lacks the `jackdaw_runtime/pie` feature.
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

/// An in-flight or finished build, deduped by `BuildSpec`. Instances
/// waiting on it are spawned when it finishes.
enum BuildState {
    /// `cargo build` is running on a task pool; `pending` lists the
    /// instances to spawn once the binary is ready. `progress` is the
    /// sink cargo writes compile progress into, surfaced in the footer.
    Running {
        task: Task<io::Result<PathBuf>>,
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
    builds: HashMap<BuildSpec, BuildState>,
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
    let Some(meta) = CargoMeta::load(&root) else {
        warn!("PIE: cargo metadata failed for {}", root.display());
        return;
    };
    let Some(spec) = resolve_build_spec(&meta, &run) else {
        warn!("PIE: no buildable package for bin `{}`", run.bin);
        return;
    };

    // If the build is cached, spawn from it immediately; the session
    // borrow is dropped before spawn_instance reads the world.
    if let Some(BuildState::Done(path)) = world.non_send::<PieSession>().builds.get(&spec) {
        let path = path.clone();
        if let Some(stage) = spawn_instance(world, &key, &run, &path) {
            world
                .non_send_mut::<PieSession>()
                .children
                .insert(key, stage);
        }
        return;
    }

    // Join an in-flight build's pending list, or start a new build.
    let mut session = world.non_send_mut::<PieSession>();
    match session.builds.get_mut(&spec) {
        Some(BuildState::Running { pending, .. }) => {
            pending.push(PendingSpawn { key, run });
        }
        Some(BuildState::Done(_)) => unreachable!("handled above"),
        Some(BuildState::Failed) | None => {
            let progress = Arc::new(Mutex::new(BuildProgress::default()));
            let build_spec = spec.clone();
            let sink = Arc::clone(&progress);
            let task = AsyncComputeTaskPool::get().spawn(async move {
                crate::ext_build::build_game_bin_with_progress(&root, &build_spec, Some(sink), None)
                    .map_err(|err| io::Error::other(err.to_string()))
            });
            info!("PIE: building game for {key}");
            session.builds.insert(
                spec,
                BuildState::Running {
                    task,
                    pending: vec![PendingSpawn { key, run }],
                    progress,
                },
            );
        }
    }
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
        .resource::<jackdaw_jsn::SceneJsnAst>()
        .contains_entity(preview)
}

/// Serialize all non-skipped reflected components from `entity` into a
/// `Vec<(type_path, json_value)>`, using the same processor and filter that
/// `register_entity_in_ast` uses when it first captures a live entity.
///
/// Components with no `ReflectComponent` data, types not registered in the
/// `AppTypeRegistry`, and paths matched by [`should_skip_component`](crate::scene_io::should_skip_component) are
/// silently omitted. The result is sorted by type path for deterministic undo
/// batching.
fn serialize_preview_entity_components(
    world: &World,
    entity: Entity,
) -> Vec<(String, serde_json::Value)> {
    use std::any::TypeId;

    use bevy::reflect::serde::TypedReflectSerializer;
    use bevy::{
        ecs::reflect::AppTypeRegistry,
        prelude::{ChildOf, Children, GlobalTransform, InheritedVisibility, ViewVisibility},
    };

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let processor = crate::scene_io::AstSerializerProcessor;

    let Ok(entity_ref) = world.get_entity(entity) else {
        return Vec::new();
    };

    // The same structural/derived components that register_entity_in_ast skips.
    let skip_ids = [
        TypeId::of::<GlobalTransform>(),
        TypeId::of::<InheritedVisibility>(),
        TypeId::of::<ViewVisibility>(),
        TypeId::of::<ChildOf>(),
        TypeId::of::<Children>(),
    ];

    let mut out: Vec<(String, serde_json::Value)> = registry
        .iter()
        .filter(|reg| !skip_ids.contains(&reg.type_id()))
        .filter_map(|reg| {
            let type_path = reg.type_info().type_path_table().path();
            if crate::scene_io::should_skip_component(type_path) {
                return None;
            }
            let reflect_component = reg.data::<ReflectComponent>()?;
            let component = reflect_component.reflect(entity_ref)?;
            let serializer =
                TypedReflectSerializer::with_processor(component, &registry, &processor);
            let value = serde_json::to_value(&serializer).ok()?;
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
/// through a [`SetJsnField`](crate::commands::SetJsnField) command. The commands are grouped into one
/// undoable [`CommandGroup`](jackdaw_commands::CommandGroup) so a single Ctrl+Z reverts the whole promote.
///
/// Path B: the preview entity carries [`PieEphemeral`](crate::pie_projection::PieEphemeral) (the game spawned it
/// at runtime with no authored counterpart). A new [`JsnEntityNode`](jackdaw_jsn::ast::JsnEntityNode) is
/// appended to the AST, its `components` filled from the preview entity's
/// reflected components, its `ecs_entity` bound to this entity. The entity
/// receives a [`JsnNodeId`](jackdaw_jsn::JsnNodeId) component and loses [`PieEphemeral`](crate::pie_projection::PieEphemeral) so it is
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

/// Path A: preview entity is bound to an existing AST node. Serialize its
/// current component values and write them through `SetJsnField` commands.
fn promote_authored_overlay(world: &mut World, preview: Entity, bits: u64) {
    use crate::commands::{CommandGroup, CommandHistory, EditorCommand, SetJsnField};

    let ast = world.resource::<jackdaw_jsn::SceneJsnAst>();
    let Some(&node_idx) = ast.ecs_to_jsn.get(&preview) else {
        warn!("save to scene: preview entity {preview:?} (bits {bits:x}) has no AST node");
        return;
    };
    let node_id = ast
        .nodes
        .get(node_idx)
        .and_then(|n| n.id)
        .map(|id| id.0)
        .unwrap_or(0);

    let entries = serialize_preview_entity_components(world, preview);

    let mut sub_commands: Vec<Box<dyn EditorCommand>> = Vec::new();
    for (type_path, new_value) in entries {
        let old_value = world
            .resource::<jackdaw_jsn::SceneJsnAst>()
            .get_component(preview, &type_path)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        sub_commands.push(Box::new(SetJsnField {
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
/// Create a new authored node from its reflected components, bind it, and
/// remove the ephemeral marker. Not undoable in v1.
pub(crate) fn promote_ephemeral_to_authored(world: &mut World, preview: Entity, bits: u64) {
    use crate::pie_projection::PieEphemeral;

    let entries = serialize_preview_entity_components(world, preview);

    let node_id = jackdaw_jsn::ast::JsnNodeId::next();
    let idx = {
        let mut ast = world.resource_mut::<jackdaw_jsn::SceneJsnAst>();
        let idx = ast.nodes.len();
        ast.nodes.push(jackdaw_jsn::ast::JsnEntityNode {
            id: Some(node_id),
            parent: None,
            components: entries.into_iter().collect(),
            derived_components: std::collections::HashSet::new(),
            ecs_entity: Some(preview),
        });
        ast.ecs_to_jsn.insert(preview, idx);
        idx
    };

    world.entity_mut(preview).remove::<PieEphemeral>();
    world.entity_mut(preview).insert(node_id);

    info!(
        "save to scene: promoted ephemeral {preview:?} (bits {bits:x}) to new AST node {} (idx {idx})",
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

/// Pre-build the default Play target in the background once the editor opens, so
/// the first Play reuses a warm cache instead of compiling the game's Bevy
/// variant on demand. Mirrors `launch_instance`'s build but with no pending
/// spawn: it only drives `PieSession.builds` to `Done`, which a later Play
/// reuses immediately.
fn prebuild_play_target(world: &mut World) {
    if world.resource::<PiePrebuildState>().attempted {
        return;
    }
    // Wait until the project's run configs have loaded.
    let run = match world.get_resource::<RunConfigs>() {
        None => return,
        Some(rc) => rc.manifest.runs.first().cloned(),
    };
    // Run configs are loaded; this is the one-shot attempt regardless of outcome.
    world.resource_mut::<PiePrebuildState>().attempted = true;
    let Some(run) = run else {
        return; // no run config to pre-build
    };
    let Some(root) = project_root(world) else {
        return;
    };
    let Some(meta) = CargoMeta::load(&root) else {
        return;
    };
    let Some(spec) = resolve_build_spec(&meta, &run) else {
        return;
    };
    let mut session = world.non_send_mut::<PieSession>();
    if session.builds.contains_key(&spec) {
        return; // already built or building (the user may have hit Play already)
    }
    let progress = Arc::new(Mutex::new(BuildProgress::default()));
    let sink = Arc::clone(&progress);
    let build_spec = spec.clone();
    let task = AsyncComputeTaskPool::get().spawn(async move {
        crate::ext_build::build_game_bin_with_progress(&root, &build_spec, Some(sink), None)
            .map_err(|err| io::Error::other(err.to_string()))
    });
    info!("PIE: pre-building the Play target in the background for a fast first Play");
    session.builds.insert(
        spec,
        BuildState::Running {
            task,
            pending: Vec::new(),
            progress,
        },
    );
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
        None => {
            if matches!(
                status.state,
                crate::build_status::BuildState::Building { .. }
            ) {
                status.state = crate::build_status::BuildState::Idle;
            }
        }
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
            Some(Ok(path)) => {
                for spawn in pending.drain(..) {
                    if let Some(stage) = spawn_instance(world, &spawn.key, &spawn.run, &path) {
                        spawned.push((spawn.key, stage));
                    }
                }
                *state = BuildState::Done(path);
            }
            Some(Err(err)) => {
                let keys: Vec<String> = pending.iter().map(|p| p.key.to_string()).collect();
                error!("PIE: game build failed for {}: {err}", keys.join(", "));
                *state = BuildState::Failed;
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

/// Launch one instance's game binary, point it at a fresh rendezvous,
/// start draining its stderr, and begin awaiting its connection on a
/// task pool. Returns the `Connecting` stage; on rendezvous or spawn
/// failure logs and returns `None` so the caller skips it.
fn spawn_instance(
    world: &World,
    key: &InstanceKey,
    run: &RunConfig,
    bin: &Path,
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
    // path replaces it (standard `Path::join` semantics).
    let cwd = match run.cwd.as_ref() {
        Some(dir) => root.join(dir),
        None => root.clone(),
    };

    let mut command = Command::new(bin);
    command
        .current_dir(&cwd)
        .envs(&run.env)
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
            error!("PIE: {key} failed to launch ({}): {err}", bin.display());
            return None;
        }
    };

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
    use bevy::reflect::serde::TypedReflectSerializer;
    use jackdaw_commands::CommandHistory;
    use jackdaw_jsn::SceneJsnAst;
    use jackdaw_jsn::ast::JsnNodeId;

    use super::*;
    use crate::pie_projection::{PieEphemeral, PieProjection};
    use crate::selection::Selection;

    const TRANSFORM_PATH: &str = "bevy_transform::components::transform::Transform";

    /// Canonical reflect JSON for a Transform, matching what
    /// `TypedReflectSerializer` produces and `SetJsnField` deserializes back.
    fn canonical(value: &Transform, registry: &AppTypeRegistry) -> serde_json::Value {
        let reg = registry.read();
        let serializer = TypedReflectSerializer::new(value, &reg);
        serde_json::to_value(&serializer).expect("serialize transform")
    }

    /// Build a minimal world for Path A tests: an authored preview entity
    /// carrying the LIVE transform value, bound to an AST node that stores
    /// the authored (identity) value, and a `PieProjection` entry mapping
    /// `bits -> preview_entity`.
    fn setup_path_a() -> (World, Entity, u64, serde_json::Value) {
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

        // AST node stores the authored (identity) value.
        let registry = world.resource::<AppTypeRegistry>().clone();
        let authored_json = canonical(&Transform::IDENTITY, &registry);
        let mut ast = SceneJsnAst::default();
        let node = ast.create_node(editor_entity, None);
        ast.set_component(editor_entity, TRANSFORM_PATH, authored_json);
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

        let live_json = canonical(&live_transform, &registry);
        let _ = node;
        (world, editor_entity, bits, live_json)
    }

    // ---- Path A tests ---------------------------------------------------

    #[test]
    fn path_a_promote_writes_live_values_to_ast_and_preview_ecs() {
        let (mut world, editor_entity, _bits, live_json) = setup_path_a();

        assert!(
            can_save_live_to_scene(&world),
            "authored entity with an AST node is saveable"
        );

        save_live_entity_to_scene(&mut world);

        // AST node now holds the live Transform.
        let stored = world
            .resource::<SceneJsnAst>()
            .get_component(editor_entity, TRANSFORM_PATH)
            .cloned()
            .expect("transform present in node after promote");
        assert_eq!(stored, live_json);

        // The preview ECS entity was refreshed through SetJsnField.
        let tf = world.get::<Transform>(editor_entity).copied().unwrap();
        assert_eq!(tf.translation, Vec3::new(1.0, 2.0, 3.0));

        // One undoable command was pushed.
        assert_eq!(world.resource::<CommandHistory>().undo_stack.len(), 1);
    }

    #[test]
    fn path_a_promote_is_undoable_back_to_authored_value() {
        let (mut world, editor_entity, _bits, _live_json) = setup_path_a();
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
        world.insert_resource(SceneJsnAst::default());

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

        // A new AST node should exist and be bound to the former ephemeral entity.
        let ast = world.resource::<SceneJsnAst>();
        assert_eq!(ast.nodes.len(), 1, "one node was created");
        assert_eq!(
            ast.nodes[0].ecs_entity,
            Some(ephemeral),
            "node is bound to the promoted entity"
        );
        assert!(
            ast.nodes[0].components.contains_key(TRANSFORM_PATH),
            "node carries the serialized Transform"
        );

        // The entity now has a JsnNodeId and no longer PieEphemeral.
        assert!(
            world.get::<JsnNodeId>(ephemeral).is_some(),
            "promoted entity carries a JsnNodeId"
        );
        assert!(
            world.get::<PieEphemeral>(ephemeral).is_none(),
            "PieEphemeral is removed after promotion"
        );

        // The AST node's id matches the entity's JsnNodeId.
        let node_id = world.get::<JsnNodeId>(ephemeral).copied().unwrap();
        assert_eq!(ast.nodes[0].id, Some(node_id));
    }

    // ---- guard / no-op tests -------------------------------------------

    #[test]
    fn no_selection_is_noop() {
        let mut world = World::new();
        world.init_resource::<PieProjection>();
        world.insert_resource(PieViewMode::Live);
        world.insert_resource(SceneJsnAst::default());
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
        world.insert_resource(SceneJsnAst::default());
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
    use jackdaw_commands::CommandHistory;
    use jackdaw_jsn::SceneJsnAst;
    use jackdaw_jsn::ast::{JsnEntityNode, JsnNodeId};

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
        let node_id = JsnNodeId::next();
        let mut ast = SceneJsnAst::default();
        ast.nodes.push(JsnEntityNode {
            id: Some(node_id),
            parent: None,
            components: std::collections::HashMap::new(),
            derived_components: std::collections::HashSet::new(),
            ecs_entity: Some(preview_entity),
        });
        ast.ecs_to_jsn.insert(preview_entity, 0);
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
