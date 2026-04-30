//! Build-artifact watcher: when cargo writes a fresh
//! `target/debug/lib<name>.so` for the active project, install it
//! and hot-swap the running dylib.
//!
//! Jackdaw doesn't run cargo itself (outside scaffold + project-open
//! flows). The user runs `cargo build` in a terminal, rust-analyzer,
//! an IDE task, or CI. We watch cargo's output directory and react
//! to a completed write. This avoids fighting editor-specific save
//! behaviors (atomic rename, tempfile swap) and avoids running cargo
//! twice when the user already has.
//!
//! Hot-swap preserves game-spawned entities and `PlayState`; the
//! window doesn't flicker. Bevy's reflect registry re-registers
//! types over any stale entries. Live state carries across reloads.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::prelude::*;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use crate::ext_build::artifact_file_name;
use crate::project::ProjectRoot;

/// Watcher runs only in `AppState::Editor` while `ProjectRoot` is
/// set. Entering/exiting the Editor state binds/drops the watcher.
pub struct HotReloadPlugin;

impl Plugin for HotReloadPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HotReloadEnabled>()
            .init_resource::<HotReloadState>()
            .add_systems(OnEnter(crate::AppState::Editor), start_watcher)
            .add_systems(OnExit(crate::AppState::Editor), stop_watcher)
            .add_systems(
                Update,
                (drain_artifact_changes, poll_install_outcome)
                    .run_if(in_state(crate::AppState::Editor)),
            )
            // Install runs in Last so Update's schedule_scope isn't
            // active while the new game's build() mutates schedules.
            // Matches project_select's apply_pending_install pattern.
            .add_systems(
                Last,
                apply_pending_install.run_if(in_state(crate::AppState::Editor)),
            );
    }
}

/// File-menu toggle. Off freezes the currently-loaded dylib;
/// subsequent builds are ignored until it's flipped back on.
#[derive(Resource)]
pub struct HotReloadEnabled(pub bool);

impl Default for HotReloadEnabled {
    fn default() -> Self {
        Self(true)
    }
}

#[derive(Resource, Default)]
struct HotReloadState {
    watcher: Option<RecommendedWatcher>,
    /// Last relevant event's instant. `drain_artifact_changes` stages
    /// the install after the debounce window elapses.
    pending: Arc<Mutex<Option<Instant>>>,
    /// `<project>/target/debug/lib<name>.so` or platform equivalent.
    artifact_path: Option<PathBuf>,
    install_outcome: Option<Arc<Mutex<Option<Result<(), jackdaw_loader::LoadError>>>>>,
    pending_install: Option<PathBuf>,
}

/// Cargo's write fires a burst of events (Create, Modify, `CloseWrite`).
/// Collapse them into one install.
const DEBOUNCE_WINDOW: Duration = Duration::from_millis(200);

fn start_watcher(
    mut state: ResMut<HotReloadState>,
    project: Option<Res<ProjectRoot>>,
    enabled: Res<HotReloadEnabled>,
) {
    if !enabled.0 {
        return;
    }
    let Some(project) = project else {
        return;
    };
    let project_root = project.root.clone();

    let expected_filename = artifact_file_name(&project_root);
    let target_debug = project_root.join("target").join("debug");
    let artifact_path = target_debug.join(&expected_filename);

    // notify's `watch()` errors on a nonexistent path. A user who
    // clones a project but hasn't built it yet lacks target/debug.
    if !target_debug.is_dir()
        && let Err(e) = std::fs::create_dir_all(&target_debug)
    {
        warn!(
            "HotReload: could not create {} for watching: {e}",
            target_debug.display()
        );
        return;
    }

    let pending = Arc::clone(&state.pending);
    let pending_for_cb = Arc::clone(&pending);
    let expected_for_cb = expected_filename.clone();

    let watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
        let event = match res {
            Ok(e) => e,
            Err(e) => {
                bevy::log::warn!("HotReload watcher error: {e}");
                return;
            }
        };
        bevy::log::trace!(
            "HotReload event: kind={:?} paths={:?}",
            event.kind,
            event.paths
        );
        if !is_artifact_event(&event, &expected_for_cb) {
            return;
        }
        if let Ok(mut slot) = pending_for_cb.lock() {
            *slot = Some(Instant::now());
        }
    });

    let Ok(mut watcher) = watcher else {
        warn!("HotReload: failed to create notify watcher");
        return;
    };

    if let Err(e) = watcher.watch(&target_debug, RecursiveMode::NonRecursive) {
        warn!("HotReload: failed to watch {}: {e}", target_debug.display());
        return;
    }
    info!(
        "HotReload: watching {} for {}",
        target_debug.display(),
        expected_filename
    );

    state.watcher = Some(watcher);
    state.artifact_path = Some(artifact_path);
}

fn stop_watcher(mut state: ResMut<HotReloadState>) {
    state.watcher = None;
    state.artifact_path = None;
    if let Ok(mut slot) = state.pending.lock() {
        *slot = None;
    }
}

/// Accept Create and Modify(Data|Any|Name) events whose path ends in
/// the expected filename. Ignore Metadata-only changes (chmod/touch)
/// so a no-op touch doesn't trigger a reinstall.
fn is_artifact_event(event: &Event, expected_filename: &str) -> bool {
    let relevant_kind = matches!(
        event.kind,
        EventKind::Create(_)
            | EventKind::Modify(notify::event::ModifyKind::Data(_))
            | EventKind::Modify(notify::event::ModifyKind::Any)
            | EventKind::Modify(notify::event::ModifyKind::Name(_))
    );
    if !relevant_kind {
        return false;
    }
    event.paths.iter().any(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == expected_filename)
    })
}

fn drain_artifact_changes(
    mut state: ResMut<HotReloadState>,
    enabled: Res<HotReloadEnabled>,
    mut install_status: ResMut<crate::extensions_dialog::InstallStatus>,
) {
    if !enabled.0 {
        return;
    }
    if state.pending_install.is_some() {
        return;
    }
    let Some(artifact) = state.artifact_path.clone() else {
        return;
    };

    let should_install = {
        let Ok(mut slot) = state.pending.lock() else {
            return;
        };
        match *slot {
            Some(t) if t.elapsed() >= DEBOUNCE_WINDOW => {
                *slot = None;
                true
            }
            _ => false,
        }
    };
    if !should_install {
        return;
    }

    if !artifact.exists() {
        // notify can fire before cargo's rename settles.
        return;
    }

    let project_name = artifact
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("project")
        .trim_start_matches("lib");
    install_status.message = Some(format!("New build detected, reloading `{project_name}`..."));
    info!(
        "HotReload: build artifact changed, staging install for {}",
        artifact.display()
    );

    let outcome: Arc<Mutex<Option<Result<(), jackdaw_loader::LoadError>>>> =
        Arc::new(Mutex::new(None));
    state.install_outcome = Some(outcome);
    state.pending_install = Some(artifact);
}

/// Exclusive system in Last. Runs the full
/// `extensions_dialog::handle_install_from_path` pipeline: atomic
/// rename into the per-user games dir, teardown of the prior dylib,
/// dlopen, new `build()`, catalog update.
///
/// When `PauseHotReload` is set and the game is currently `Playing`
/// or `Paused`, the install is requeued onto `pending_install` so
/// the next `OnEnter(PlayState::Stopped)` (or a manual flush) picks
/// it up. This keeps Play sessions deterministic for users that
/// want to opt into that behaviour.
///
/// On a swap during Play, the `SubApp` world's reflect-registered
/// component state is round-tripped through a `DynamicScene` so
/// game state survives the dlopen. See `crate::migration`.
pub(crate) fn apply_pending_install(world: &mut World) {
    let artifact_opt = world
        .resource_mut::<HotReloadState>()
        .pending_install
        .take();
    let Some(artifact) = artifact_opt else {
        return;
    };

    // If the user has paused hot-reload during Play, requeue the
    // install for later. Re-using `pending_install` matches the way
    // the watcher stages new artifacts; the next call (driven from
    // OnEnter(PlayState::Stopped)) will pick it up.
    let is_playing = matches!(
        world.resource::<State<jackdaw_api::pie::PlayState>>().get(),
        jackdaw_api::pie::PlayState::Playing | jackdaw_api::pie::PlayState::Paused
    );
    let pause_during_play = world.resource::<crate::pie::PauseHotReload>().0;
    if is_playing && pause_during_play {
        let mut state = world.resource_mut::<HotReloadState>();
        state.pending_install = Some(artifact);
        return;
    }

    let outcome_arc = world.resource::<HotReloadState>().install_outcome.clone();

    // Capture the SubApp's reflect state before the dlopen swap.
    // `GameSubAppHolder` is a non-send resource; take it out so we
    // get unaliased access to its inner `World`, then reinsert it.
    let snapshot = if let Some(mut holder) =
        world.remove_non_send_resource::<jackdaw_runtime::GameSubAppHolder>()
    {
        let snap = crate::migration::capture_subapp_snapshot(holder.sub.world_mut());
        world.insert_non_send_resource(holder);
        Some(snap)
    } else {
        None
    };

    let result = crate::extensions_dialog::handle_install_from_path(world, artifact);

    // Apply the captured snapshot to the freshly-built SubApp world.
    // The new world has a different `TypeId` namespace; the scene
    // writer uses `TypePath` strings (Reflect identity), which is
    // why migration survives the dlopen swap.
    if let Some(snapshot) = snapshot
        && let Some(mut holder) =
            world.remove_non_send_resource::<jackdaw_runtime::GameSubAppHolder>()
    {
        crate::migration::apply_subapp_snapshot(snapshot, holder.sub.world_mut());
        world.insert_non_send_resource(holder);
    }

    match &result {
        Ok(jackdaw_loader::LoadedKind::Game(name)) => {
            info!("HotReload: game `{name}` swapped in place.");
        }
        Ok(jackdaw_loader::LoadedKind::Extension(name)) => {
            info!("HotReload: extension `{name}` re-registered.");
        }
        Err(_) => {}
    }
    if let Some(arc) = outcome_arc
        && let Ok(mut slot) = arc.lock()
    {
        *slot = Some(result.map(|_| ()));
    }
}

fn poll_install_outcome(
    mut state: ResMut<HotReloadState>,
    mut install_status: ResMut<crate::extensions_dialog::InstallStatus>,
) {
    let Some(outcome) = state.install_outcome.clone() else {
        return;
    };
    let taken = {
        let Ok(mut slot) = outcome.lock() else {
            return;
        };
        slot.take()
    };
    let Some(result) = taken else {
        return;
    };
    state.install_outcome = None;

    match result {
        Ok(()) => {}
        Err(err) if err.is_symbol_mismatch() => {
            warn!(
                "HotReload: SDK symbol mismatch, project was built against a different editor SDK: {err}"
            );
            install_status.message = Some(
                "Hot reload: the new build was compiled against an older editor SDK. \
                 Run `cargo clean -p <your-crate> && cargo build` and try again."
                    .into(),
            );
        }
        Err(err) => {
            warn!("HotReload: install failed: {err}");
            install_status.message = Some(format!(
                "Hot reload install failed: {err}. Rebuild to retry."
            ));
        }
    }
}
