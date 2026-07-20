//! First-run SDK setup screen.
//!
//! A packaged jackdaw ships without a prebuilt SDK; the first launch
//! builds it once into `~/.jackdaw/sdk/...`. This module gates the
//! project launcher behind a progress screen while that build runs, so
//! the user sees live progress instead of a frozen window and cannot
//! start a project build concurrently (which would fight the SDK build
//! for memory). In a dev checkout there is no embedded recipe, so
//! [`bootstrap::needs_setup`] is false and none of this runs.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::{
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
};
use jackdaw_feathers::{icons::EditorFont, progress, tokens};
use jackdaw_project_build::bootstrap::{self, SetupProgress};

use crate::AppState;

/// How long the finished "ready" state lingers before the overlay is
/// removed and the launcher becomes usable.
const READY_LINGER: Duration = Duration::from_millis(1600);

/// Cap on the retained log tail.
const LOG_TAIL: usize = 40;

pub struct SdkSetupPlugin;

impl Plugin for SdkSetupPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SdkSetup>()
            .add_systems(OnEnter(AppState::ProjectSelect), start_sdk_setup)
            .add_systems(
                Update,
                (poll_sdk_setup, refresh_sdk_setup_ui)
                    .chain()
                    .run_if(in_state(AppState::ProjectSelect)),
            );
    }
}

/// Progress shared between the background `ensure_sdk` thread (writer)
/// and the UI refresh (reader).
#[derive(Default, Clone)]
struct SetupShared {
    phase: String,
    current_crate: Option<String>,
    done: u32,
    total: Option<u32>,
    log: VecDeque<String>,
}

impl SetupShared {
    fn push_log(&mut self, line: String) {
        if self.log.len() >= LOG_TAIL {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }
}

#[derive(Resource, Default)]
struct SdkSetup {
    /// Live progress, written by the background build thread.
    shared: Option<Arc<Mutex<SetupShared>>>,
    /// The in-flight `ensure_sdk` build.
    task: Option<Task<Result<PathBuf, String>>>,
    /// Copy of `shared` taken each frame, off the writer's lock.
    snapshot: Option<SetupShared>,
    /// Set when the build finishes. `Err` keeps the overlay up with a
    /// retry; `Ok` lingers, then tears the overlay down.
    outcome: Option<Result<(), String>>,
    /// When a successful build completed, for the linger timer.
    done_at: Option<Instant>,
    /// Raised by the retry button; consumed by `poll_sdk_setup`.
    retry: bool,
}

#[derive(Component)]
struct SetupOverlay;
#[derive(Component)]
struct SetupPhaseLabel;
#[derive(Component)]
struct SetupCrateLabel;
#[derive(Component)]
struct SetupBarSlot;
#[derive(Component)]
struct SetupLogText;
#[derive(Component)]
struct SetupErrorRow;
#[derive(Component)]
struct SetupErrorText;

/// On entering the launcher, kick off the SDK build if one is owed.
/// A running task (a re-enter) or an auto-open handoff (no launcher
/// shell) is left alone.
fn start_sdk_setup(
    mut commands: Commands,
    mut setup: ResMut<SdkSetup>,
    editor_font: Res<EditorFont>,
    pending: Option<Res<crate::project_select::PendingAutoOpen>>,
) {
    if pending.is_some() || setup.task.is_some() || matches!(setup.outcome, Some(Ok(()))) {
        return;
    }
    if !bootstrap::needs_setup() {
        return;
    }
    kick_off(&mut commands, &mut setup, &editor_font.0);
}

/// Spawn the build task and the gating overlay.
fn kick_off(commands: &mut Commands, setup: &mut SdkSetup, font: &Handle<Font>) {
    let shared = Arc::new(Mutex::new(SetupShared {
        phase: "Preparing setup...".to_string(),
        ..Default::default()
    }));
    let writer = Arc::clone(&shared);
    let task = AsyncComputeTaskPool::get().spawn(async move {
        bootstrap::ensure_sdk(move |event| {
            let Ok(mut g) = writer.lock() else {
                return;
            };
            match event {
                SetupProgress::Phase(phase) => {
                    g.phase = phase.to_string();
                    g.push_log(format!("== {phase}"));
                }
                SetupProgress::Total(total) => g.total = Some(total),
                SetupProgress::Compiled { crate_name, done } => {
                    g.current_crate = Some(crate_name);
                    g.done = done;
                }
                SetupProgress::Log(line) => g.push_log(line),
            }
        })
    });

    setup.shared = Some(shared);
    setup.task = Some(task);
    setup.snapshot = None;
    setup.outcome = None;
    setup.done_at = None;
    spawn_overlay(commands, font);
}

/// Copy the latest progress, drain the finished task, and tear the
/// overlay down once a successful build has lingered.
fn poll_sdk_setup(
    mut commands: Commands,
    mut setup: ResMut<SdkSetup>,
    overlays: Query<Entity, With<SetupOverlay>>,
    editor_font: Res<EditorFont>,
) {
    // A retry after a failure: clear the old overlay and start again.
    if setup.retry && setup.task.is_none() {
        setup.retry = false;
        for entity in overlays.iter() {
            commands.entity(entity).despawn();
        }
        let font = editor_font.0.clone();
        kick_off(&mut commands, &mut setup, &font);
        return;
    }

    if let Some(shared) = setup.shared.clone()
        && let Ok(guard) = shared.lock()
    {
        setup.snapshot = Some(guard.clone());
    }

    if let Some(task) = setup.task.as_mut()
        && let Some(result) = future::block_on(future::poll_once(task))
    {
        setup.task = None;
        match result {
            Ok(cache) => {
                info!("SDK setup complete: {}", cache.display());
                setup.outcome = Some(Ok(()));
                setup.done_at = Some(Instant::now());
                if let Some(shared) = &setup.shared
                    && let Ok(mut guard) = shared.lock()
                {
                    guard.phase = "jackdaw is ready".to_string();
                    guard.current_crate = None;
                }
            }
            Err(err) => {
                warn!("SDK setup failed: {err}");
                setup.outcome = Some(Err(err));
            }
        }
    }

    if matches!(setup.outcome, Some(Ok(())))
        && setup.done_at.is_some_and(|at| at.elapsed() >= READY_LINGER)
    {
        for entity in overlays.iter() {
            commands.entity(entity).despawn();
        }
        setup.shared = None;
        setup.snapshot = None;
        setup.done_at = None;
    }
}

/// Reflect the snapshot into the overlay: phase, "compiling X (n/total)",
/// the progress bar, the log tail, and the error row.
fn refresh_sdk_setup_ui(
    setup: Res<SdkSetup>,
    mut texts: Query<(
        &mut Text,
        Option<&SetupPhaseLabel>,
        Option<&SetupCrateLabel>,
        Option<&SetupLogText>,
        Option<&SetupErrorText>,
    )>,
    bar_slots: Query<&Children, With<SetupBarSlot>>,
    children_q: Query<&Children>,
    mut fill_q: Query<&mut Node, (With<progress::ProgressBarFill>, Without<SetupErrorRow>)>,
    mut error_rows: Query<&mut Node, (With<SetupErrorRow>, Without<progress::ProgressBarFill>)>,
) {
    let Some(snap) = setup.snapshot.as_ref() else {
        return;
    };
    let ready = matches!(setup.outcome, Some(Ok(())));
    let error = match &setup.outcome {
        Some(Err(err)) => Some(err.clone()),
        _ => None,
    };

    let phase_line = snap.phase.clone();
    let crate_line = match (&snap.current_crate, snap.total) {
        (Some(name), Some(total)) => {
            // The estimate can undershoot the real unit count; clamp so
            // the counter never reads past the total.
            let total = total.max(snap.done);
            format!("Compiling {name}  ({}/{total})", snap.done)
        }
        (Some(name), None) => format!("Compiling {name}  ({} so far)", snap.done),
        (None, Some(total)) => format!("0 / {total}"),
        (None, None) => String::new(),
    };
    let log_line = snap.log.iter().cloned().collect::<Vec<_>>().join("\n");
    let error_line = error.clone().unwrap_or_default();

    for (mut text, is_phase, is_crate, is_log, is_error) in texts.iter_mut() {
        let target = if is_phase.is_some() {
            &phase_line
        } else if is_crate.is_some() {
            &crate_line
        } else if is_log.is_some() {
            &log_line
        } else if is_error.is_some() {
            &error_line
        } else {
            continue;
        };
        if &text.0 != target {
            text.0 = target.clone();
        }
    }

    let fraction = if ready {
        1.0
    } else {
        match (snap.total, snap.done) {
            (Some(total), done) if total > 0 => (done as f32 / total as f32).clamp(0.0, 1.0),
            _ => 0.0,
        }
    };
    let desired_width = Val::Percent(fraction * 100.0);
    for slot_children in bar_slots.iter() {
        for bar in slot_children.iter() {
            let Ok(inner) = children_q.get(bar) else {
                continue;
            };
            for fill in inner.iter() {
                if let Ok(mut node) = fill_q.get_mut(fill)
                    && node.width != desired_width
                {
                    node.width = desired_width;
                }
            }
        }
    }

    let desired = if error.is_some() {
        Display::Flex
    } else {
        Display::None
    };
    for mut node in error_rows.iter_mut() {
        if node.display != desired {
            node.display = desired;
        }
    }
}

/// Spawn the full-window gating overlay: a scrim plus a card with the
/// heading, phase, per-crate counter, progress bar, log tail, and a
/// hidden error row with a retry button.
fn spawn_overlay(commands: &mut Commands, font: &Handle<Font>) {
    let scrim = commands
        .spawn((
            SetupOverlay,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(tokens::DIALOG_BACKDROP),
            GlobalZIndex(200),
        ))
        .id();

    let card = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(12.0),
                padding: UiRect::all(Val::Px(28.0)),
                min_width: Val::Px(520.0),
                max_width: Val::Px(640.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_LG)),
                ..default()
            },
            BackgroundColor(tokens::PANEL_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(scrim),
        ))
        .id();

    commands.spawn((
        Text::new("Setting up jackdaw"),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_LG,
            ..default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        ChildOf(card),
    ));
    commands.spawn((
        Text::new(
            "First-time setup builds the editor SDK once, into your home directory. \
             This can take 10-15 minutes and happens only on the first launch; \
             later launches skip straight to your projects.",
        ),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    commands.spawn((
        SetupPhaseLabel,
        Text::new("Preparing setup..."),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE,
            ..default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        Node {
            margin: UiRect::top(Val::Px(6.0)),
            ..default()
        },
        ChildOf(card),
    ));
    commands.spawn((
        SetupCrateLabel,
        Text::new(String::new()),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(card),
    ));

    let bar_slot = commands
        .spawn((
            SetupBarSlot,
            Node {
                width: Val::Percent(100.0),
                ..default()
            },
            ChildOf(card),
        ))
        .id();
    commands.spawn((progress::progress_bar(0.0), ChildOf(bar_slot)));

    commands.spawn((
        SetupLogText,
        Text::new(String::new()),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            max_height: Val::Px(180.0),
            overflow: Overflow::clip(),
            ..default()
        },
        ChildOf(card),
    ));

    // Error row: hidden until the build fails, then shows the reason and
    // a retry button.
    let error_row = commands
        .spawn((
            SetupErrorRow,
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(8.0),
                margin: UiRect::top(Val::Px(6.0)),
                display: Display::None,
                ..default()
            },
            ChildOf(card),
        ))
        .id();
    commands.spawn((
        SetupErrorText,
        Text::new(String::new()),
        TextFont {
            font: font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_ERROR),
        ChildOf(error_row),
    ));
    let retry = commands
        .spawn((
            Node {
                align_self: AlignSelf::FlexStart,
                padding: UiRect::axes(Val::Px(16.0), Val::Px(8.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..default()
            },
            BackgroundColor(tokens::SELECTED_BG),
            children![(
                Text::new("Retry"),
                TextFont {
                    font: font.clone().into(),
                    font_size: tokens::TEXT_SIZE,
                    ..default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
            ChildOf(error_row),
        ))
        .id();
    commands
        .entity(retry)
        .observe(|_: On<Pointer<Click>>, mut setup: ResMut<SdkSetup>| {
            setup.retry = true;
        });
}
