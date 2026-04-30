//! Play-In-Editor runtime.
//!
//! Jackdaw hosts a game's systems in its own `App` (same World,
//! not a `SubApp`). Games are dylibs loaded at startup via the
//! `jackdaw_game_entry_v1` FFI symbol; their `build(&mut App)`
//! callback registers systems into the editor's schedule. Game
//! systems gate their execution on [`PlayState::Playing`] so they
//! only tick when the user has Play engaged.
//!
//! This module provides:
//! - [`PlayState`]; the `Stopped` / `Playing` / `Paused` state.
//! - [`PrePlayScene`]; scene AST snapshot captured at Play time,
//!   restored on Stop so the authored scene is the revert baseline.
//! - [`PieButton`]; marker component for the toolbar transport
//!   buttons; the `PiePlugin` auto-wires a click observer to each.
//! - [`GameSpawned`]; marker added automatically to any entity that
//!   receives a `Transform` during `PlayState::Playing`. Editor
//!   surfaces (hierarchy, inspector) use it to distinguish
//!   authored-then-played entities from ones the game spawned.
//! - [`PiePlugin`]; registers state, resource, and observers.
//!
//! Handlers [`handle_play`], [`handle_pause`], [`handle_stop`] are
//! exposed for direct `commands.queue(...)` use in case other
//! surfaces (keybinds, menu entries) want to trigger PIE
//! transitions without going through a button.

use bevy::prelude::*;
use jackdaw_api::pie::PlayState;
use jackdaw_api::prelude::*;
use jackdaw_jsn::SceneJsnAst;

/// How the user's game runs when they hit Play.
///
/// Stored as a Resource so the toolbar's play-mode chevron can flip
/// it at runtime, and persisted into `project.jsn` so each project
/// remembers its preferred mode across editor restarts.
#[derive(Resource, Reflect, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[reflect(Resource)]
pub enum PlayMode {
    /// Game runs in-process inside the editor via dlopen + `SubApp`.
    /// Renders into the editor's viewport.
    #[default]
    InEditor,
    /// Game runs as a child process (`cargo run`). Opens its own
    /// OS window. Editor stays as the editor.
    Standalone,
}

impl From<jackdaw_jsn::JsnPlayMode> for PlayMode {
    fn from(value: jackdaw_jsn::JsnPlayMode) -> Self {
        match value {
            jackdaw_jsn::JsnPlayMode::InEditor => PlayMode::InEditor,
            jackdaw_jsn::JsnPlayMode::Standalone => PlayMode::Standalone,
        }
    }
}

impl From<PlayMode> for jackdaw_jsn::JsnPlayMode {
    fn from(value: PlayMode) -> Self {
        match value {
            PlayMode::InEditor => jackdaw_jsn::JsnPlayMode::InEditor,
            PlayMode::Standalone => jackdaw_jsn::JsnPlayMode::Standalone,
        }
    }
}

/// Marker for the play-mode chevron sitting next to the
/// Play / Pause / Stop transport buttons. `PiePlugin` installs an
/// `On<Add, PlayModeDropdown>` observer that wires the click to a
/// toggle on the [`PlayMode`] resource.
#[derive(Component, Clone, Copy, Debug)]
pub struct PlayModeDropdown;

/// User-facing toggle (default off): when `true`, hot-reload during
/// `PlayState::Playing` is queued instead of applied. The next click
/// of Stop applies it; next click of Play uses the new dylib.
///
/// Lets the user opt into deterministic Play sessions (no surprise
/// dylib swap mid-playtest). When `false` (default) the build-artifact
/// watcher swaps the dylib in place and Bevy `Reflect` is used to
/// round-trip `SubApp` state through a `DynamicScene` so game state
/// survives.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct PauseHotReload(pub bool);

/// Marker for the toolbar toggle that flips [`PauseHotReload`]. The
/// `PiePlugin` installs an `On<Add, PauseHotReloadToggle>` observer
/// that wires the click.
#[derive(Component, Clone, Copy, Debug)]
pub struct PauseHotReloadToggle;

/// Frozen AST captured when the user clicks Play from `Stopped`.
/// Restored on Stop so any game-spawned entities or authored-entity
/// mutations are reverted.
#[derive(Resource, Default)]
pub struct PrePlayScene {
    snapshot: Option<SceneJsnAst>,
}

/// Marker for the toolbar transport buttons. `PiePlugin` installs
/// an `On<Add, PieButton>` observer that wires each button's
/// `Pointer<Click>` to the corresponding handler.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PieButton {
    Play,
    Pause,
    Stop,
}

/// Marker added to any entity spawned while the editor is in
/// [`PlayState::Playing`]. The hierarchy tints these rows a
/// distinct colour so it's visually obvious which entities are
/// game-owned (and therefore will disappear on Stop) versus
/// authored.
///
/// Tagged automatically via the `On<Add, Transform>` observer in
/// `tag_game_spawned`. Entities that spawn without a `Transform`
/// aren't tagged; in practice this covers the 99% of game-spawned
/// entities that have one (meshes, lights, cameras, sprites, UI).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct GameSpawned;

pub struct PiePlugin;

impl Plugin for PiePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<PlayState>()
            .init_resource::<PrePlayScene>()
            .init_resource::<PlayMode>()
            .init_resource::<PauseHotReload>()
            .register_type::<PlayMode>()
            .add_observer(wire_pie_button)
            .add_observer(wire_play_mode_dropdown)
            .add_observer(wire_pause_hot_reload_toggle)
            .add_observer(tag_game_spawned)
            // Drive the user's `GameSubAppHolder` (installed by
            // `EditorPlugins::with_game::<P>()` or by the dylib
            // loader) only while `PlayState::Playing`. The SubApp's
            // `extract` callback syncs editor-world state into the
            // game world; its `Main` schedule then ticks the user's
            // `Update` / `FixedUpdate` etc.
            .add_systems(Update, drive_game_sub_app.run_if(play_is_playing))
            .add_systems(
                OnEnter(PlayState::Playing),
                crate::pie_camera::swap_active_camera_on_play_enter,
            )
            .add_systems(
                OnEnter(PlayState::Stopped),
                (
                    crate::pie_camera::swap_active_camera_on_stop_enter,
                    // If the user paused hot-reload during Play and a
                    // build was queued, drain it now that we're back
                    // in Stopped. The gate inside `apply_pending_install`
                    // checks `is_playing`, so calling it from here
                    // proceeds with the install rather than re-queueing.
                    crate::hot_reload::apply_pending_install,
                ),
            );
    }
}

fn drive_game_sub_app(world: &mut World) {
    // `GameSubAppHolder` is a non-send resource so the SubApp can
    // own `!Send` state (asset loaders, render queues). Take it out
    // for the duration of the tick and reinsert; this also gives us
    // `&mut World` access during `extract` without aliasing.
    let Some(mut holder) = world.remove_non_send_resource::<jackdaw_runtime::GameSubAppHolder>()
    else {
        return;
    };
    holder.update(world);
    world.insert_non_send_resource(holder);
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<PiePlayOp>()
        .register_operator::<PiePauseOp>()
        .register_operator::<PieStopOp>();
}

fn play_is_stopped_or_paused(
    state: Res<State<PlayState>>,
    new_project: Option<Res<crate::project_select::NewProjectState>>,
) -> bool {
    if matches!(state.get(), PlayState::Playing) {
        return false;
    }
    // Block Play while a background project build is still running.
    // The status bar's "Building..." indicator tells the user what
    // they're waiting for; this just prevents an accidental Play
    // click from no-op-ing silently against a half-built project.
    if let Some(np) = new_project.as_deref()
        && let Some(progress) = np.build_progress_snapshot.as_ref()
        && !progress.finished
    {
        return false;
    }
    true
}

fn play_is_playing(state: Res<State<PlayState>>) -> bool {
    *state.get() == PlayState::Playing
}

fn play_is_running(state: Res<State<PlayState>>) -> bool {
    *state.get() != PlayState::Stopped
}

/// Start the game running in the editor. From Stopped, captures a
/// snapshot of the scene first so Stop can restore it; from Paused,
/// resumes.
#[operator(
    id = "pie.play",
    label = "Play",
    description = "Start the game running in the editor.",
    is_available = play_is_stopped_or_paused
)]
pub(crate) fn pie_play(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let mode = *world.resource::<PlayMode>();
        match mode {
            PlayMode::InEditor => handle_play(world),
            PlayMode::Standalone => crate::standalone_play::start_standalone_play(world),
        }
    });
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
    commands.queue(|world: &mut World| {
        let mode = *world.resource::<PlayMode>();
        // Pause only makes sense for in-editor PIE; the standalone
        // child process owns its own loop and there's no SubApp to
        // freeze. Silently ignore the click in Standalone mode.
        if matches!(mode, PlayMode::InEditor) {
            handle_pause(world);
        }
    });
    OperatorResult::Finished
}

/// Stop the running game and restore the scene to the state it was in
/// before Play was pressed.
#[operator(
    id = "pie.stop",
    label = "Stop",
    description = "Stop the running game and restore the scene.",
    is_available = play_is_running
)]
pub(crate) fn pie_stop(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(|world: &mut World| {
        let mode = *world.resource::<PlayMode>();
        match mode {
            PlayMode::InEditor => handle_stop(world),
            PlayMode::Standalone => crate::standalone_play::stop_standalone_play(world),
        }
    });
    OperatorResult::Finished
}

/// Observer: tag entities that receive a `Transform` while
/// `PlayState::Playing` is active with [`GameSpawned`]. Fires once
/// per entity because `On<Add, Transform>` is a one-shot event.
fn tag_game_spawned(
    trigger: On<Add, Transform>,
    state: Res<State<PlayState>>,
    already_tagged: Query<(), With<GameSpawned>>,
    mut commands: Commands,
) {
    if *state.get() != PlayState::Playing {
        return;
    }
    let entity = trigger.event_target();
    if already_tagged.get(entity).is_ok() {
        return;
    }
    commands.entity(entity).insert(GameSpawned);
}

/// Spawn a click observer on each `PauseHotReloadToggle` as it's
/// added. Click flips the [`PauseHotReload`] resource. v1 surface
/// is a tiny labeled pill in the toolbar; a proper checkbox menu
/// item under the play-mode dropdown is a polish item.
fn wire_pause_hot_reload_toggle(trigger: On<Add, PauseHotReloadToggle>, mut commands: Commands) {
    let entity = trigger.event_target();
    commands.entity(entity).observe(
        |_: On<Pointer<Click>>, mut toggle: ResMut<PauseHotReload>| {
            toggle.0 = !toggle.0;
            info!("PauseHotReload toggled: {}", toggle.0);
        },
    );
}

/// Spawn a click observer on each `PlayModeDropdown` as it's added.
/// Click toggles the [`PlayMode`] resource between `InEditor` and
/// `Standalone`. A proper popup menu is a polish item; click-to-toggle
/// is enough for v1.
fn wire_play_mode_dropdown(trigger: On<Add, PlayModeDropdown>, mut commands: Commands) {
    let entity = trigger.event_target();
    commands
        .entity(entity)
        .observe(|_: On<Pointer<Click>>, mut mode: ResMut<PlayMode>| {
            *mode = match *mode {
                PlayMode::InEditor => PlayMode::Standalone,
                PlayMode::Standalone => PlayMode::InEditor,
            };
            info!("PlayMode toggled: {:?}", *mode);
        });
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

/// Transition into `Playing`. If currently `Stopped`, snapshot the
/// scene first so Stop has something to restore. No-op if already
/// `Playing`.
pub fn handle_play(world: &mut World) {
    let current = world.resource::<State<PlayState>>().get().clone();
    match current {
        PlayState::Stopped => {
            let snapshot = world.resource::<SceneJsnAst>().clone();
            world.resource_mut::<PrePlayScene>().snapshot = Some(snapshot);
            world
                .resource_mut::<NextState<PlayState>>()
                .set(PlayState::Playing);
            info!("PIE: Play (fresh start, scene snapshot captured)");
        }
        PlayState::Paused => {
            world
                .resource_mut::<NextState<PlayState>>()
                .set(PlayState::Playing);
            info!("PIE: Play (resumed)");
        }
        PlayState::Playing => {}
    }
}

/// Transition `Playing` → `Paused`. No-op otherwise.
pub fn handle_pause(world: &mut World) {
    if *world.resource::<State<PlayState>>().get() == PlayState::Playing {
        world
            .resource_mut::<NextState<PlayState>>()
            .set(PlayState::Paused);
        info!("PIE: Pause");
    }
}

/// Transition to `Stopped`. Resets the `GameSubAppHolder` so the
/// `SubApp`'s world is reconstructed from the user's plugin
/// factory; next Play starts from a fresh game world. The editor's
/// authoring world is not touched (gameplay never mutated it — the
/// `SubApp`'s world is separate).
///
/// Backwards-compat note: prior to the `SubApp` redesign, Stop
/// applied a `PrePlayScene` snapshot to the editor's authoring
/// world to revert game-system mutations. With the `SubApp`
/// boundary, the authoring world is the persistent state and the
/// `SubApp` is ephemeral; the snapshot path is dormant for now
/// (kept around in case some editor-side surface still relies on
/// `PrePlayScene` until full migration).
pub fn handle_stop(world: &mut World) {
    let current = world.resource::<State<PlayState>>().get().clone();
    if current == PlayState::Stopped {
        return;
    }

    if let Some(mut holder) = world.remove_non_send_resource::<jackdaw_runtime::GameSubAppHolder>()
    {
        holder.reset();
        world.insert_non_send_resource(holder);
        info!("PIE: Stop (game subapp reset)");
    } else {
        info!("PIE: Stop (no game loaded)");
    }
    // Clear any leftover snapshot resource; the SubApp boundary now
    // handles state isolation, so the snapshot is no longer the
    // source of truth on Stop.
    world.resource_mut::<PrePlayScene>().snapshot = None;

    world
        .resource_mut::<NextState<PlayState>>()
        .set(PlayState::Stopped);
}
