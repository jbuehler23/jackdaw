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
//! [`BuildSpec`](crate::ext_build::BuildSpec): several instances of the
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
use crate::pie_mirror::{PieLiveSelection, PieMirror, PieViewMode};
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
            .init_non_send_resource::<PieSession>()
            .init_resource::<PieMirror>()
            .init_resource::<PieViewMode>()
            .init_resource::<PieLiveSelection>()
            .add_systems(Update, (advance_pie_session, drain_game_events))
            .add_systems(OnEnter(PlayState::Stopped), reset_view_mode_on_stop)
            .add_observer(wire_pie_button);
    }
}

/// Reset the outliner/inspector view back to Scene when play stops, and
/// drop any Live selection so the next play session starts clean.
fn reset_view_mode_on_stop(
    mut mode: ResMut<PieViewMode>,
    mut live_selection: ResMut<PieLiveSelection>,
) {
    *mode = PieViewMode::Scene;
    live_selection.clear();
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<PiePlayOp>()
        .register_operator::<PiePauseOp>()
        .register_operator::<PieStopOp>()
        .register_operator::<PieReloadOp>();
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

/// Stop every running instance, drop the cached build path so the next
/// launch re-runs the (incremental) cargo build, then relaunch each
/// instance that was running. The game reloads its scene from disk.
pub fn handle_reload(world: &mut World) {
    let keys: Vec<InstanceKey> = world
        .non_send_resource::<PieSession>()
        .children
        .keys()
        .cloned()
        .collect();

    if keys.is_empty() {
        return;
    }

    let run_configs = world.resource::<RunConfigs>().manifest.clone();

    handle_stop(world);

    if let Some(mut live_selection) = world.get_resource_mut::<PieLiveSelection>() {
        live_selection.clear();
    }

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
        let session = world.non_send_resource::<PieSession>();
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
    if let Some(BuildState::Done(path)) = world.non_send_resource::<PieSession>().builds.get(&spec)
    {
        let path = path.clone();
        if let Some(stage) = spawn_instance(world, &key, &run, &path) {
            world
                .non_send_resource_mut::<PieSession>()
                .children
                .insert(key, stage);
        }
        return;
    }

    // Join an in-flight build's pending list, or start a new build.
    let mut session = world.non_send_resource_mut::<PieSession>();
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
    let Some(mut stage) = world
        .non_send_resource_mut::<PieSession>()
        .children
        .remove(key)
    else {
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

    if world.non_send_resource::<PieSession>().children.is_empty()
        && *world.resource::<State<PlayState>>().get() != PlayState::Stopped
    {
        world
            .resource_mut::<NextState<PlayState>>()
            .set(PlayState::Stopped);
        if let Some(mut mirror) = world.get_resource_mut::<PieMirror>() {
            mirror.clear();
        }
        if let Some(mut live_selection) = world.get_resource_mut::<PieLiveSelection>() {
            live_selection.clear();
        }
    }
}

/// Begin play. From Stopped, launches the first run config's instance
/// (building it if needed). From Paused, resumes every live child. No-op
/// if already Playing or if the project has no run configurations.
pub fn handle_play(world: &mut World) {
    let current = world.resource::<State<PlayState>>().get().clone();
    match current {
        PlayState::Stopped => {
            let runs = world.resource::<RunConfigs>().manifest.runs.clone();
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

    let mut session = world.non_send_resource_mut::<PieSession>();
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
        if let Some(mut mirror) = world.get_resource_mut::<PieMirror>() {
            mirror.clear();
        }
        if let Some(mut live_selection) = world.get_resource_mut::<PieLiveSelection>() {
            live_selection.clear();
        }
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
    let mut session = world.non_send_resource_mut::<PieSession>();
    for stage in session.children.values_mut() {
        if let ChildStage::Live { transport, .. } = stage {
            send_control_to(transport, event.clone());
        }
    }
}

/// Send a live edit (`SetComponent` / `AddComponent` / `RemoveComponent`)
/// to every live child. The owning game applies it; other children skip
/// the unknown entity (their apply warns and skips), so broadcasting is
/// fine while a single game runs.
pub(crate) fn send_edit(world: &mut World, edit: ControlEvent) {
    broadcast_control(world, edit);
}

/// Whether the Live "Save to Scene" action can run right now: the inspector
/// is in Live mode and the selected mirror entity maps back to an authored
/// node (`scene_node_id` is `Some`). Runtime-only entities (spawned by the
/// game with no authored origin) return `false` so the button stays dimmed.
pub(crate) fn can_save_live_to_scene(world: &World) -> bool {
    if *world.resource::<PieViewMode>() != PieViewMode::Live {
        return false;
    }
    let Some(bits) = world.resource::<PieLiveSelection>().selected else {
        return false;
    };
    world
        .resource::<PieMirror>()
        .entities
        .get(&bits)
        .and_then(|entry| entry.scene_node_id)
        .is_some()
}

/// Promote the selected running entity's current component values into the
/// authored scene node it came from.
///
/// Looks up the mirror entry for [`PieLiveSelection`], resolves its
/// `scene_node_id` to the authored node (and that node's preview ECS entity)
/// via [`SceneJsnAst::entity_for_node_id`], then writes each runtime
/// component into the node through [`SetJsnField`] (full-component replace,
/// empty field path). The edits are grouped into one undoable
/// [`CommandGroup`] so the same path Scene edits use also refreshes the
/// preview ECS entity (`apply_jsn_field_to_ecs` inside `SetJsnField`).
///
/// Runtime-only / internal components (render, picking, the structural node
/// id, etc.) are filtered with [`should_skip_component`] so the authored
/// node only gains values that round-trip through save.
///
/// A no-op with a `warn!` when nothing is selected or the matching node was
/// deleted during play.
pub(crate) fn save_live_entity_to_scene(world: &mut World) {
    use jackdaw_jsn::ast::JsnNodeId;

    use crate::commands::{CommandGroup, CommandHistory, EditorCommand, SetJsnField};

    if *world.resource::<PieViewMode>() != PieViewMode::Live {
        return;
    }
    let Some(bits) = world.resource::<PieLiveSelection>().selected else {
        warn!("save to scene: no live entity selected");
        return;
    };

    let Some((node_id, components)) = world
        .resource::<PieMirror>()
        .entities
        .get(&bits)
        .and_then(|entry| entry.scene_node_id.map(|id| (id, entry.components.clone())))
    else {
        warn!("save to scene: live entity {bits:x} has no authored node to save into");
        return;
    };

    let Some(editor_entity) = world
        .resource::<jackdaw_jsn::SceneJsnAst>()
        .entity_for_node_id(JsnNodeId(node_id))
    else {
        warn!("save to scene: authored node {node_id} not found (deleted during play?)");
        return;
    };

    // Stable iteration order keeps the grouped undo deterministic.
    let mut entries: Vec<(String, serde_json::Value)> = components
        .into_iter()
        .filter(|(type_path, _)| !crate::scene_io::should_skip_component(type_path))
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut sub_commands: Vec<Box<dyn EditorCommand>> = Vec::new();
    for (type_path, new_value) in entries {
        let old_value = world
            .resource::<jackdaw_jsn::SceneJsnAst>()
            .get_component(editor_entity, &type_path)
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        sub_commands.push(Box::new(SetJsnField {
            entity: editor_entity,
            type_path,
            // Empty field path replaces the whole component; `SetJsnField`
            // then re-inserts it on the preview ECS entity.
            field_path: String::new(),
            old_value,
            new_value,
            was_derived: false,
        }));
    }

    let count = sub_commands.len();
    let mut cmd: Box<dyn EditorCommand> = if count == 0 {
        warn!("save to scene: live entity {bits:x} had no saveable components");
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

/// Mirror the active game build into the editor's `BuildStatus` so the
/// footer shows what is compiling, and clear it once no build remains.
fn reconcile_build_status(world: &mut World) {
    let building = world
        .non_send_resource::<PieSession>()
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
    let mut builds = std::mem::take(&mut world.non_send_resource_mut::<PieSession>().builds);
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

    let mut session = world.non_send_resource_mut::<PieSession>();
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
    let children = std::mem::take(&mut world.non_send_resource_mut::<PieSession>().children);

    let mut survivors: HashMap<InstanceKey, ChildStage> = HashMap::with_capacity(children.len());
    for (key, stage) in children {
        if let Some(next) = advance_child(&key, stage) {
            survivors.insert(key, next);
        }
    }

    world.non_send_resource_mut::<PieSession>().children = survivors;
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
    let session = world.non_send_resource::<PieSession>();
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

    // Ask the kernel to SIGKILL this child when the editor (its parent) dies by
    // any means -- including a SIGKILL the editor can never trap, or the
    // `process::exit` the Ctrl+C handler takes (which skips `Drop`). Without
    // this, killing the editor leaves the games running.
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: the closure runs in the forked child before `exec`; `prctl`,
        // `getppid`, and `raise` are all async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong) == -1 {
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

/// Drain `StateEvent`s from every live child and apply them to [`PieMirror`].
fn drain_game_events(mut session: NonSendMut<PieSession>, mut mirror: ResMut<PieMirror>) {
    for (key, stage) in session.children.iter_mut() {
        let ChildStage::Live { transport, .. } = stage else {
            continue;
        };
        let frames = transport.drain_received();
        if frames.is_empty() {
            continue;
        }
        let mut count = 0usize;
        for (_channel, bytes) in frames {
            match from_bytes::<StateEvent>(&bytes) {
                Ok(event) => {
                    mirror.apply(event);
                    count += 1;
                }
                Err(err) => warn!("PIE: {key} dropping malformed state event: {err}"),
            }
        }
        if count > 0 {
            debug!("PIE: {key} received {count} state event(s)");
        }
    }
}

#[cfg(test)]
mod save_to_scene_tests {
    use bevy::ecs::reflect::AppTypeRegistry;
    use bevy::reflect::serde::TypedReflectSerializer;
    use jackdaw_commands::CommandHistory;
    use jackdaw_jsn::SceneJsnAst;

    use super::*;
    use crate::pie_mirror::PieMirrorEntry;

    const TRANSFORM_PATH: &str = "bevy_transform::components::transform::Transform";

    /// Canonical reflect JSON for a value, matching what the PIE mirror
    /// stores and what `SetJsnField` deserializes back onto the ECS entity.
    fn canonical(value: &Transform, registry: &AppTypeRegistry) -> serde_json::Value {
        let reg = registry.read();
        let serializer = TypedReflectSerializer::new(value, &reg);
        serde_json::to_value(&serializer).expect("serialize transform")
    }

    /// Build a minimal world wired the way the editor-side promote expects:
    /// an authored node bound to a preview ECS entity that carries
    /// `Transform`, plus the PIE resources in Live mode with one mirror
    /// entry mapped back to that node.
    fn setup(scene_node_id: Option<u64>) -> (World, Entity, u64, serde_json::Value) {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        registry.write().register::<Transform>();
        world.insert_resource(registry);
        world.init_resource::<CommandHistory>();
        world.init_resource::<PieMirror>();
        world.init_resource::<PieLiveSelection>();
        world.insert_resource(PieViewMode::Live);

        // Authored preview entity starts at the origin.
        let editor_entity = world.spawn(Transform::IDENTITY).id();
        let mut ast = SceneJsnAst::default();
        let node = ast.create_node(editor_entity, None);
        let node_id = ast.nodes[node].id.expect("created node has an id");
        // Seed the authored Transform so the promote has an old value to
        // capture for undo.
        let registry = world.resource::<AppTypeRegistry>().clone();
        ast.set_component(
            editor_entity,
            TRANSFORM_PATH,
            canonical(&Transform::IDENTITY, &registry),
        );
        // The mirror reports the node by its stable id, not the editor entity.
        let on_disk_id = scene_node_id.unwrap_or(node_id.0);
        world.insert_resource(ast);

        // Mirror entry: runtime Transform moved to (1, 2, 3).
        let runtime = canonical(&Transform::from_xyz(1.0, 2.0, 3.0), &registry);
        let bits = 0xABCDu64;
        let mut components = std::collections::HashMap::new();
        components.insert(TRANSFORM_PATH.to_string(), runtime.clone());
        world.resource_mut::<PieMirror>().entities.insert(
            bits,
            PieMirrorEntry {
                components,
                scene_node_id: Some(on_disk_id),
            },
        );
        world.resource_mut::<PieLiveSelection>().selected = Some(bits);

        (world, editor_entity, bits, runtime)
    }

    #[test]
    fn promote_writes_runtime_values_to_ast_and_preview_ecs() {
        let (mut world, editor_entity, _bits, runtime) = setup(None);

        assert!(
            can_save_live_to_scene(&world),
            "selection maps to an authored node, so the action is available"
        );

        save_live_entity_to_scene(&mut world);

        // AST node now holds the runtime Transform.
        let stored = world
            .resource::<SceneJsnAst>()
            .get_component(editor_entity, TRANSFORM_PATH)
            .cloned()
            .expect("transform present in node after promote");
        assert_eq!(stored, runtime);

        // The preview ECS entity was updated through the SetJsnField path.
        let tf = world.get::<Transform>(editor_entity).copied().unwrap();
        assert_eq!(tf.translation, Vec3::new(1.0, 2.0, 3.0));

        // One undoable command was recorded.
        assert_eq!(world.resource::<CommandHistory>().undo_stack.len(), 1);
    }

    #[test]
    fn promote_is_undoable_back_to_authored_value() {
        let (mut world, editor_entity, _bits, _runtime) = setup(None);
        save_live_entity_to_scene(&mut world);

        // Undo the most recently pushed command.
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

    #[test]
    fn runtime_only_entity_cannot_save_and_promote_is_noop() {
        // scene_node_id None -> a runtime-only entity with no authored origin.
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        registry.write().register::<Transform>();
        world.insert_resource(registry);
        world.init_resource::<CommandHistory>();
        world.init_resource::<PieMirror>();
        world.init_resource::<PieLiveSelection>();
        world.insert_resource(PieViewMode::Live);
        world.insert_resource(SceneJsnAst::default());

        let bits = 0x99u64;
        let mut components = std::collections::HashMap::new();
        components.insert(TRANSFORM_PATH.to_string(), serde_json::json!({}));
        world.resource_mut::<PieMirror>().entities.insert(
            bits,
            PieMirrorEntry {
                components,
                scene_node_id: None,
            },
        );
        world.resource_mut::<PieLiveSelection>().selected = Some(bits);

        assert!(
            !can_save_live_to_scene(&world),
            "runtime-only entity has no authored node, so saving is disabled"
        );

        save_live_entity_to_scene(&mut world);
        assert_eq!(
            world.resource::<CommandHistory>().undo_stack.len(),
            0,
            "no authored node means nothing is recorded"
        );
    }

    #[test]
    fn missing_node_is_noop_when_authored_node_deleted() {
        // Mirror points at a stable id that no AST node carries (the node
        // was deleted during play). Promote must warn and do nothing.
        let (mut world, _editor_entity, _bits, _runtime) = setup(Some(987_654));
        // Sanity: the gate is still "available" (scene_node_id is Some), but
        // the node lookup fails, so the promote is inert.
        assert!(can_save_live_to_scene(&world));
        save_live_entity_to_scene(&mut world);
        assert_eq!(
            world.resource::<CommandHistory>().undo_stack.len(),
            0,
            "an unresolved node id records no command"
        );
    }
}
