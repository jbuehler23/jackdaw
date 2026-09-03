//! `JD_PERF_PROBE`: where the frame's CPU time went, by schedule.
//!
//! The FPS readout says a frame took 240ms. It cannot say whether that
//! was an editor system in `Update`, a scene respawn in `SpawnScene`, UI
//! layout in `PostUpdate`, or the render graph -- and the fix for each
//! is unrelated to the fix for the others. Guessing wrong here costs
//! days.
//!
//! Setting the variable inserts a marker schedule after each of the main
//! schedules and stamps the time since the frame began, then logs a
//! five-second rolling average of each slice:
//!
//! ```text
//! PERF_PROBE (26 frames): First=0.1ms PreUpdate=17.9ms FixedMain=5.6ms
//!                         Update=10.2ms SpawnScene=111.6ms PostUpdate=42.2ms Last=0.2ms
//! ```
//!
//! The slices cover the main schedule only, so the shortfall against the
//! frame time is the render sub-app.
//!
//! `SpawnScene` is why the split is worth having rather than reasoning
//! about: it sits between `Update` and `PostUpdate`, it is where scene
//! and world-asset instances are spawned, and a probe that omits it
//! bills its cost to `PostUpdate` without saying so.
//!
//! `PERF_COUNT` rides along on the same timer with the entity and UI
//! node counts, because a UI node count that climbs while nothing on
//! screen changes is its own bug and is otherwise invisible.
//!
//! Off unless asked for: the marker schedules cost a dispatch each, and
//! the counts are two full world scans.

use core::time::Duration;
use std::time::Instant;

use bevy::app::{
    First, Last, MainScheduleOrder, PostUpdate, PreUpdate, RunFixedMainLoop, SpawnScene,
};
use bevy::ecs::schedule::ScheduleLabel;
use bevy::prelude::*;

/// Turns the per-schedule split on when set to anything but `0`.
pub const ENV_PERF_PROBE: &str = "JD_PERF_PROBE";

/// How long a reported average covers.
const REPORT_INTERVAL: Duration = Duration::from_secs(5);

macro_rules! mark {
    ($name:ident) => {
        #[derive(ScheduleLabel, Clone, Debug, PartialEq, Eq, Hash)]
        struct $name;
    };
}

mark!(MarkFirst);
mark!(MarkPreUpdate);
mark!(MarkFixed);
mark!(MarkUpdate);
mark!(MarkSpawnScene);
mark!(MarkPostUpdate);
mark!(MarkLast);

/// The schedules a slice is reported for, in the order they run.
const LABELS: [&str; 7] = [
    "First",
    "PreUpdate",
    "FixedMain",
    "Update",
    "SpawnScene",
    "PostUpdate",
    "Last",
];

/// Whether this process was asked for the split.
pub fn requested() -> bool {
    enabled(
        std::env::var_os(ENV_PERF_PROBE)
            .and_then(|value| value.into_string().ok())
            .as_deref(),
    )
}

/// Whether a [`ENV_PERF_PROBE`] value asks for the split.
///
/// A shell that exports the variable permanently still has to be able to
/// launch an ordinary editor, so `0`, `false` and an empty value are
/// off.
fn enabled(raw: Option<&str>) -> bool {
    raw.is_some_and(|value| !matches!(value.trim(), "" | "0" | "false"))
}

#[derive(Resource)]
struct Probe {
    /// When the current frame's `First` ran.
    frame_start: Instant,
    /// Time since `frame_start` at the end of each schedule.
    marks: [Duration; LABELS.len()],
    /// Accumulated slice per schedule since the last report.
    sums: [Duration; LABELS.len()],
    frames: u32,
    last_report: Instant,
}

impl Default for Probe {
    fn default() -> Self {
        Self {
            frame_start: Instant::now(),
            marks: [Duration::ZERO; LABELS.len()],
            sums: [Duration::ZERO; LABELS.len()],
            frames: 0,
            last_report: Instant::now(),
        }
    }
}

fn begin_frame(mut probe: ResMut<Probe>) {
    probe.frame_start = Instant::now();
}

/// Stamp the end of the `I`th schedule.
fn stamp<const I: usize>(mut probe: ResMut<Probe>) {
    let elapsed = probe.frame_start.elapsed();
    probe.marks[I] = elapsed;
}

/// Entity and UI node counts, on the report's own timer.
///
/// Two full world scans, which is why they are not per frame.
fn count_entities(nodes: Query<(), With<Node>>, all: Query<()>, mut last: Local<Option<Instant>>) {
    let now = Instant::now();
    if last.is_some_and(|previous| now.duration_since(previous) < REPORT_INTERVAL) {
        return;
    }
    *last = Some(now);
    info!(
        "PERF_COUNT: ui_nodes={} total_entities={}",
        nodes.iter().count(),
        all.iter().count()
    );
}

fn report(mut probe: ResMut<Probe>) {
    let marks = probe.marks;
    let mut previous = Duration::ZERO;
    for (index, mark) in marks.iter().enumerate() {
        probe.sums[index] += mark.saturating_sub(previous);
        previous = *mark;
    }
    probe.frames += 1;
    if probe.last_report.elapsed() < REPORT_INTERVAL {
        return;
    }
    let frames = probe.frames.max(1);
    let parts: Vec<String> = LABELS
        .iter()
        .enumerate()
        .map(|(index, label)| {
            let ms = probe.sums[index].as_secs_f64() * 1000.0 / f64::from(frames);
            format!("{label}={ms:.1}ms")
        })
        .collect();
    info!("PERF_PROBE ({frames} frames): {}", parts.join(" "));
    probe.sums = [Duration::ZERO; LABELS.len()];
    probe.frames = 0;
    probe.last_report = Instant::now();
}

pub(crate) fn plugin(app: &mut App) {
    if !requested() {
        return;
    }
    app.init_resource::<Probe>().add_systems(First, begin_frame);

    // A marker schedule after each main schedule is the only placement
    // that survives a multithreaded executor: a stamping system inside a
    // schedule runs whenever the executor reaches it, which is not the
    // schedule's boundary.
    let mut order = app.world_mut().resource_mut::<MainScheduleOrder>();
    order.insert_after(First, MarkFirst);
    order.insert_after(PreUpdate, MarkPreUpdate);
    order.insert_after(RunFixedMainLoop, MarkFixed);
    order.insert_after(Update, MarkUpdate);
    order.insert_after(SpawnScene, MarkSpawnScene);
    order.insert_after(PostUpdate, MarkPostUpdate);
    order.insert_after(Last, MarkLast);

    app.add_systems(MarkFirst, stamp::<0>)
        .add_systems(MarkPreUpdate, stamp::<1>)
        .add_systems(MarkFixed, stamp::<2>)
        .add_systems(MarkUpdate, stamp::<3>)
        .add_systems(MarkSpawnScene, stamp::<4>)
        .add_systems(MarkPostUpdate, stamp::<5>)
        .add_systems(MarkLast, (stamp::<6>, count_entities, report).chain());
    info!("{ENV_PERF_PROBE}: logging a per-schedule CPU split every {REPORT_INTERVAL:?}");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An ordinary launch pays nothing: no marker schedules, no scans.
    #[test]
    fn an_unset_environment_is_off() {
        assert!(!enabled(None));
    }

    /// A shell that exports the variable permanently can still launch a
    /// normal editor.
    #[test]
    fn zero_false_and_empty_are_off() {
        assert!(!enabled(Some("0")));
        assert!(!enabled(Some("false")));
        assert!(!enabled(Some("  ")));
    }

    /// Anything else asks for the split; `=1` is what the docs say.
    #[test]
    fn anything_else_is_on() {
        assert!(enabled(Some("1")));
        assert!(enabled(Some("true")));
    }

    /// `SpawnScene` keeps its own slice. Folding it into `PostUpdate` is
    /// the one misattribution that sent this investigation down a blind
    /// alley, so it is asserted rather than trusted.
    #[test]
    fn spawn_scene_keeps_its_own_slice() {
        assert_eq!(LABELS[4], "SpawnScene");
        assert_eq!(LABELS[5], "PostUpdate");
    }
}
