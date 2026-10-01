//! Progress of long editor tasks: the stage a task is in and, where it knows
//! its total, how far through that stage it is.
//!
//! Any task reports through [`begin_progress`], [`progress_stage`],
//! [`progress_count`] and [`finish_progress`] or [`fail_progress`]. The footer
//! names the most recent task, the remote reports it, and a modal overlay draws
//! its stage and a bar while it runs.

use std::time::{Duration, Instant};

use bevy::prelude::*;
use jackdaw_feathers::status_bar::StatusBarLeft;
use jackdaw_feathers::tokens;
use jackdaw_localization::LocalizedText;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<EditorProgress>()
        .add_systems(PostUpdate, (sync_progress_overlay, sync_status_left));
}

/// Where one long task has got to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskProgress {
    /// What the task is, such as "Opening village".
    pub title: String,
    /// The step it is on, such as "Loading models".
    pub stage: String,
    pub done: usize,
    /// How much the stage holds, or `None` while that cannot be known.
    pub total: Option<usize>,
}

impl TaskProgress {
    /// The stage with its count, as the footer and the overlay word it.
    pub fn label(&self) -> String {
        match self.total {
            Some(total) => format!("{} {} / {}", self.stage, self.done, total),
            None => self.stage.clone(),
        }
    }

    /// How far through the stage the task is, or `None` when it cannot say.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total?;
        if total == 0 {
            return Some(1.0);
        }
        Some((self.done as f32 / total as f32).clamp(0.0, 1.0))
    }
}

struct RunningTask {
    owner: &'static str,
    progress: TaskProgress,
    began: Instant,
    /// Draw the overlay from the first frame rather than after
    /// [`OVERLAY_DELAY`].
    immediate: bool,
}

/// The long tasks running now, in the order they began.
#[derive(Resource, Default)]
pub struct EditorProgress {
    running: Vec<RunningTask>,
}

impl EditorProgress {
    /// The task begun most recently that is still running.
    pub fn current(&self) -> Option<&TaskProgress> {
        self.running.last().map(|task| &task.progress)
    }

    /// The task `owner` is running, if any.
    pub fn get(&self, owner: &str) -> Option<&TaskProgress> {
        self.task(owner).map(|task| &task.progress)
    }

    pub fn is_running(&self, owner: &str) -> bool {
        self.task(owner).is_some()
    }

    fn task(&self, owner: &str) -> Option<&RunningTask> {
        self.running.iter().find(|task| task.owner == owner)
    }

    fn task_mut(&mut self, owner: &str) -> Option<&mut RunningTask> {
        self.running.iter_mut().find(|task| task.owner == owner)
    }

    /// Whether the overlay should be drawn for the current task by now.
    fn overlay_due(&self) -> bool {
        self.running
            .last()
            .is_some_and(|task| task.immediate || task.began.elapsed() >= OVERLAY_DELAY)
    }
}

/// How long a task runs before the overlay covers the editor, so that a task
/// over in a moment does not flash it up.
const OVERLAY_DELAY: Duration = Duration::from_millis(300);

/// Sent each time a task moves on to a new stage.
#[derive(Event, Clone, Debug, PartialEq, Eq)]
pub struct ProgressStageBegan {
    pub owner: &'static str,
    pub stage: String,
}

/// Sent when a task ends, with what went wrong if it failed.
#[derive(Event, Clone, Debug, PartialEq, Eq)]
pub struct ProgressEnded {
    pub owner: &'static str,
    pub error: Option<String>,
}

/// Start a task titled `title` for `owner`, replacing any task it was running.
/// An `immediate` task draws the overlay from its first frame, for a caller
/// that is about to hold the frame itself.
pub fn begin_progress(
    world: &mut World,
    owner: &'static str,
    title: impl Into<String>,
    immediate: bool,
) {
    let mut progress = world.get_resource_or_init::<EditorProgress>();
    progress.running.retain(|task| task.owner != owner);
    progress.running.push(RunningTask {
        owner,
        progress: TaskProgress {
            title: title.into(),
            stage: String::new(),
            done: 0,
            total: None,
        },
        began: Instant::now(),
        immediate,
    });
}

/// Move `owner`'s task on to `stage`, counting from zero towards `total`.
pub fn progress_stage(
    world: &mut World,
    owner: &'static str,
    stage: impl Into<String>,
    total: Option<usize>,
) {
    let stage = stage.into();
    let Some(mut progress) = world.get_resource_mut::<EditorProgress>() else {
        return;
    };
    let Some(task) = progress.task_mut(owner) else {
        return;
    };
    let moved_on = task.progress.stage != stage;
    task.progress.stage = stage.clone();
    task.progress.done = 0;
    task.progress.total = total;
    let label = task.progress.label();
    crate::status_bar::begin_phase(world, owner, label);
    if moved_on {
        world.trigger(ProgressStageBegan { owner, stage });
    }
}

/// Say how far `owner`'s task is through its current stage.
pub fn progress_count(world: &mut World, owner: &'static str, done: usize, total: Option<usize>) {
    let Some(mut progress) = world.get_resource_mut::<EditorProgress>() else {
        return;
    };
    let Some(task) = progress.task_mut(owner) else {
        return;
    };
    if task.progress.done == done && task.progress.total == total {
        return;
    }
    task.progress.done = done;
    task.progress.total = total;
    let label = task.progress.label();
    crate::status_bar::begin_phase(world, owner, label);
}

/// End `owner`'s task.
pub fn finish_progress(world: &mut World, owner: &'static str) {
    if end_task(world, owner) {
        world.trigger(ProgressEnded { owner, error: None });
    }
}

/// End `owner`'s task because it failed, and put `error` in front of the user.
pub fn fail_progress(world: &mut World, owner: &'static str, error: impl Into<String>) {
    let error = error.into();
    crate::status_bar::notify_error(world, error.clone());
    if end_task(world, owner) {
        world.trigger(ProgressEnded {
            owner,
            error: Some(error),
        });
    }
}

fn end_task(world: &mut World, owner: &'static str) -> bool {
    crate::status_bar::finish_phase(world, owner);
    let Some(mut progress) = world.get_resource_mut::<EditorProgress>() else {
        return false;
    };
    let before = progress.running.len();
    progress.running.retain(|task| task.owner != owner);
    progress.running.len() != before
}

/// The modal overlay drawn while a task runs.
#[derive(Component, Default, Clone)]
pub struct ProgressOverlay;

#[derive(Component, Default, Clone)]
struct ProgressTitle;

#[derive(Component, Default, Clone)]
struct ProgressStageText;

#[derive(Component, Default, Clone)]
struct ProgressCountText;

#[derive(Component, Default, Clone)]
struct ProgressFill;

/// Dims the editor behind the overlay.
const BACKDROP: Color = Color::srgba(0.0, 0.0, 0.0, 0.6);

/// Above the editor's panels, so nothing under it can be clicked.
const OVERLAY_Z: i32 = 190;

/// Above an open dialog, where the card moves aside to the bottom of the window
/// and lets the pointer through, so the dialog can still be answered.
const BESIDE_DIALOG_Z: i32 = 300;

/// Gap between the card and the bottom of the window while a dialog is open.
const BESIDE_DIALOG_MARGIN: f32 = 48.0;

/// Width of the moving block an indeterminate bar draws, as a share of the track.
const INDETERMINATE_SHARE: f32 = 0.3;

/// Seconds for the indeterminate block to cross the track once.
const INDETERMINATE_PERIOD: f32 = 1.4;

fn progress_overlay() -> impl Scene {
    bsn! {
        ProgressOverlay
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
        }
        BackgroundColor(BACKDROP)
        GlobalZIndex(OVERLAY_Z)
        Children [
            (
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(10),
                    padding: UiRect::all(px(20)),
                    width: px(440),
                    border: UiRect::all(px(1)),
                    border_radius: BorderRadius::all(tokens::CORNER_RADIUS_LG),
                }
                BackgroundColor(tokens::PANEL_BG)
                BorderColor::all(tokens::BORDER_SUBTLE)
                Children [
                    (
                        ProgressTitle
                        Text("")
                        TextFont { font_size: tokens::TEXT_SIZE_LG }
                        TextColor(tokens::TEXT_PRIMARY)
                    ),
                    (
                        ProgressStageText
                        Text("")
                        TextFont { font_size: tokens::TEXT_SIZE }
                        TextColor(tokens::TEXT_SECONDARY)
                    ),
                    (
                        Node {
                            width: percent(100),
                            height: px(6),
                            border: UiRect::all(px(1)),
                            overflow: Overflow::clip(),
                        }
                        BackgroundColor(tokens::INPUT_BG)
                        BorderColor::all(tokens::BORDER_SUBTLE)
                        Children [
                            (
                                ProgressFill
                                Node {
                                    position_type: PositionType::Absolute,
                                    height: percent(100),
                                    width: percent(0),
                                }
                                BackgroundColor(tokens::ACCENT_BLUE)
                            )
                        ]
                    ),
                    (
                        ProgressCountText
                        Text("")
                        TextFont { font_size: tokens::TEXT_SIZE_SM }
                        TextColor(tokens::TEXT_SECONDARY)
                    ),
                ]
            )
        ]
    }
}

/// Spawn, update or take down the overlay to match the current task.
fn sync_progress_overlay(world: &mut World) {
    let shown = world
        .get_resource::<EditorProgress>()
        .filter(|progress| progress.overlay_due())
        .and_then(EditorProgress::current)
        .cloned();
    let mut overlays = world.query_filtered::<Entity, With<ProgressOverlay>>();
    let existing: Vec<Entity> = overlays.iter(world).collect();

    let Some(task) = shown else {
        for overlay in existing {
            world.entity_mut(overlay).despawn();
        }
        return;
    };
    if existing.is_empty()
        && let Err(err) = world.spawn_scene(progress_overlay())
    {
        error!("progress overlay failed to spawn: {err}");
    }

    let dialog_open = world
        .query_filtered::<(), With<jackdaw_feathers::dialog::EditorDialog>>()
        .iter(world)
        .next()
        .is_some();
    place_overlay(world, dialog_open);

    let font = world
        .get_resource::<jackdaw_feathers::icons::EditorFont>()
        .map(|font| font.0.clone());
    set_text::<ProgressTitle>(world, &task.title, font.as_ref());
    set_text::<ProgressStageText>(world, &task.stage, font.as_ref());
    let count = match task.total {
        Some(total) => format!("{} / {}", task.done, total),
        None => String::new(),
    };
    set_text::<ProgressCountText>(world, &count, font.as_ref());

    let seconds = world
        .get_resource::<Time<Real>>()
        .map_or(0.0, Time::elapsed_secs);
    let (left, width) = match task.fraction() {
        Some(fraction) => (0.0, fraction),
        None => {
            let travel = 1.0 + INDETERMINATE_SHARE;
            let phase = (seconds / INDETERMINATE_PERIOD).fract();
            (phase * travel - INDETERMINATE_SHARE, INDETERMINATE_SHARE)
        }
    };
    let mut fills = world.query_filtered::<&mut Node, With<ProgressFill>>();
    for mut node in fills.iter_mut(world) {
        node.left = percent(left * 100.0);
        node.width = percent(width * 100.0);
    }
}

/// Name the running task where the footer otherwise says the editor is ready,
/// and hand the slot back to its own wording once the task is over.
fn sync_status_left(
    progress: Res<EditorProgress>,
    mut labels: Query<(&mut Text, &mut LocalizedText), With<StatusBarLeft>>,
    mut naming: Local<bool>,
) {
    match progress.current() {
        Some(task) => {
            for (mut text, _) in &mut labels {
                if text.0 != task.title {
                    text.0.clone_from(&task.title);
                }
            }
            *naming = true;
        }
        None if *naming => {
            for (_, mut localized) in &mut labels {
                localized.set_changed();
            }
            *naming = false;
        }
        None => {}
    }
}

/// Cover the editor with the card in the middle, or, while a dialog is open,
/// leave the dialog the middle of the window and the pointer, and show the card
/// at the bottom above the dialog's own backdrop.
fn place_overlay(world: &mut World, dialog_open: bool) {
    let (z, backdrop, align, padding) = if dialog_open {
        (
            BESIDE_DIALOG_Z,
            Color::NONE,
            AlignItems::FlexEnd,
            UiRect::bottom(px(BESIDE_DIALOG_MARGIN)),
        )
    } else {
        (OVERLAY_Z, BACKDROP, AlignItems::Center, UiRect::ZERO)
    };
    let mut overlays = world.query_filtered::<(
        Entity,
        &mut Node,
        &mut BackgroundColor,
        &mut GlobalZIndex,
    ), With<ProgressOverlay>>();
    let mut moved = Vec::new();
    for (entity, mut node, mut color, mut global_z) in overlays.iter_mut(world) {
        if global_z.0 == z {
            continue;
        }
        global_z.0 = z;
        color.0 = backdrop;
        node.align_items = align;
        node.padding = padding;
        moved.push(entity);
    }
    for entity in moved {
        let mut overlay = world.entity_mut(entity);
        if dialog_open {
            overlay.insert(Pickable::IGNORE);
        } else {
            overlay.remove::<Pickable>();
        }
    }
}

fn set_text<M: Component>(world: &mut World, value: &str, font: Option<&Handle<Font>>) {
    let mut texts = world.query_filtered::<(&mut Text, &mut TextFont), With<M>>();
    for (mut text, mut text_font) in texts.iter_mut(world) {
        if text.0 != value {
            text.0 = value.to_string();
        }
        if let Some(font) = font {
            let wanted = font.clone().into();
            if text_font.font != wanted {
                text_font.font = wanted;
            }
        }
    }
}
