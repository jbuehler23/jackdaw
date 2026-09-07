//! Operators for everything the Timeline tab offers.
//!
//! Every control in the tab dispatches one of these, so a script and a click
//! take the same path and the tab can be driven with no pointer at all.

use bevy::prelude::*;
use jackdaw_animation::{
    AnimationTrack, Clip, ClipEvent, ClipRecording, Interpolation, LoopMode, OnionSkin,
    SelectedClip, SelectedTrack, TimelineCursor, TimelineDirty, TimelineSnap, TimelineView,
    TimelineZoom,
};
use jackdaw_api::prelude::*;
use jackdaw_commands::{CommandHistory, EditorCommand};

use crate::selection::Selection;

/// The reflected type path of the authored clip, which is what a field edit
/// has to name to reach the document.
const CLIP: &str = "jackdaw_animation::clip::Clip";

/// The same for a track.
const TRACK: &str = "jackdaw_animation::clip::AnimationTrack";

/// Zoom beyond which the sheet is wider than any sensible panel.
const MAX_ZOOM: f32 = 20.0;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ClipRecordToggleOp>()
        .register_operator::<ClipLoopModeOp>()
        .register_operator::<ClipEventAddOp>()
        .register_operator::<ClipEventRemoveOp>()
        .register_operator::<ClipTrackEnableOp>()
        .register_operator::<ClipTrackInterpolationOp>()
        .register_operator::<ClipSeekOp>()
        .register_operator::<ClipSnapOp>()
        .register_operator::<ClipViewOp>()
        .register_operator::<ClipSelectOp>()
        .register_operator::<ClipOnionSkinOp>()
        .register_operator::<ClipZoomOp>();
}

/// Turn recording on or off.
#[operator(
    id = "clip.record.toggle",
    label = "Record Keys",
    description = "While recording, an inspector edit of a tracked property writes a key at the \
                   playhead instead of only changing the entity.",
    allows_undo = false,
    params(enabled(bool, doc = "On or off. Omit to flip whichever way it currently is."))
)]
pub(crate) fn clip_record_toggle(
    params: In<OperatorParameters>,
    mut recording: ResMut<ClipRecording>,
) -> OperatorResult {
    recording.0 = params.as_bool("enabled").unwrap_or(!recording.0);
    OperatorResult::Finished
}

/// Say what playback does at the end of the clip.
#[operator(
    id = "clip.loop_mode",
    label = "Clip Loop Mode",
    description = "Hold the last frame at the end of the clip, or start again from the top.",
    is_available = a_clip_is_up,
    params(mode(String, doc = "`clamp` holds the last frame; `wrap` starts again."))
)]
pub(crate) fn clip_loop_mode(
    params: In<OperatorParameters>,
    selected: Res<SelectedClip>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(mode) = params.as_str("mode").and_then(LoopMode::from_name) else {
        warn!("clip.loop_mode: `mode` has to be `clamp` or `wrap`");
        return OperatorResult::Cancelled;
    };
    let clip = selected.0?;
    commands.queue(move |world: &mut World| {
        set_field(world, clip, CLIP, "loop_mode", serde_json::json!(mode));
    });
    OperatorResult::Finished
}

/// Put a named moment on the clip.
#[operator(
    id = "clip.event.add",
    label = "Add Clip Event",
    description = "Put a named moment on the clip, which sends an animation event as playback \
                   crosses it.",
    is_available = a_clip_is_up,
    params(
        time(f64, doc = "Seconds from the start of the clip. Defaults to the playhead."),
        name(String, doc = "What the message the event sends is called."),
    )
)]
pub(crate) fn clip_event_add(
    params: In<OperatorParameters>,
    selected: Res<SelectedClip>,
    cursor: Res<TimelineCursor>,
    snap: Res<TimelineSnap>,
    mut commands: Commands,
) -> OperatorResult {
    let clip = selected.0?;
    let time = params
        .as_float("time")
        .map_or_else(|| snap.round(cursor.seek_time), |time| time as f32)
        .max(0.0);
    let name = params
        .as_str("name")
        .filter(|name| !name.is_empty())
        .unwrap_or("Event")
        .to_string();
    commands.queue(move |world: &mut World| {
        run_event_edit(world, ClipEventEdit::adding(clip, time, name));
    });
    OperatorResult::Finished
}

/// Take away the clip event nearest the playhead.
#[operator(
    id = "clip.event.remove",
    label = "Remove Clip Event",
    description = "Take away the clip event nearest the playhead.",
    is_available = a_clip_is_up
)]
pub(crate) fn clip_event_remove(
    _: In<OperatorParameters>,
    selected: Res<SelectedClip>,
    cursor: Res<TimelineCursor>,
    children: Query<&Children>,
    events: Query<&ClipEvent>,
    mut commands: Commands,
) -> OperatorResult {
    let clip = selected.0?;
    let nearest = children
        .get(clip)
        .into_iter()
        .flatten()
        .filter_map(|child| events.get(*child).ok().map(|event| (*child, event.time)))
        .min_by(|a, b| {
            (a.1 - cursor.seek_time)
                .abs()
                .total_cmp(&(b.1 - cursor.seek_time).abs())
        })
        .map(|(entity, _)| entity)?;
    commands.queue(move |world: &mut World| {
        let Some(held) = world.get::<ClipEvent>(nearest).cloned() else {
            return;
        };
        run_event_edit(
            world,
            ClipEventEdit {
                clip,
                event: Some(nearest),
                time: held.time,
                name: held.name,
                adding: false,
            },
        );
    });
    OperatorResult::Finished
}

/// Take a track out of the compiled clip, or put it back.
#[operator(
    id = "clip.track.enable",
    label = "Enable Track",
    description = "A track switched off keeps its keys and stops driving its property.",
    allows_undo = false,
    params(
        track(Entity, doc = "The track to switch. Omit to switch the chosen track."),
        enabled(bool, doc = "On or off. Omit to flip whichever way it currently is."),
    )
)]
pub(crate) fn clip_track_enable(
    params: In<OperatorParameters>,
    chosen: Res<SelectedTrack>,
    tracks: Query<&AnimationTrack>,
    mut commands: Commands,
) -> OperatorResult {
    let track = params.as_entity("track").or(chosen.0)?;
    let held = tracks.get(track).ok()?;
    let enabled = params.as_bool("enabled").unwrap_or(!held.enabled);
    commands.queue(move |world: &mut World| {
        set_field(world, track, TRACK, "enabled", serde_json::json!(enabled));
    });
    OperatorResult::Finished
}

/// Say how a track reads between its keys.
#[operator(
    id = "clip.track.interpolation",
    label = "Track Interpolation",
    description = "Blend between a track's keys, ease along a spline through them, or hold each \
                   until the next.",
    allows_undo = false,
    params(
        track(Entity, doc = "The track to set. Omit to set the chosen track."),
        mode(
            String,
            doc = "`linear`, `cubic` or `step`. Omit to move on to the next."
        ),
    )
)]
pub(crate) fn clip_track_interpolation(
    params: In<OperatorParameters>,
    chosen: Res<SelectedTrack>,
    tracks: Query<&AnimationTrack>,
    mut commands: Commands,
) -> OperatorResult {
    let track = params.as_entity("track").or(chosen.0)?;
    let held = tracks.get(track).ok()?;
    let mode = match params.as_str("mode") {
        Some(name) => match Interpolation::from_name(name) {
            Some(mode) => mode,
            None => {
                warn!("clip.track.interpolation: no mode is called `{name}`");
                return OperatorResult::Cancelled;
            }
        },
        None => held.interpolation.next(),
    };
    commands.queue(move |world: &mut World| {
        set_field(
            world,
            track,
            TRACK,
            "interpolation",
            serde_json::json!(mode),
        );
    });
    OperatorResult::Finished
}

/// Park the playhead at a time.
///
/// The transport's own jumps are gated on the Timeline tab holding the
/// keypress, which a caller with no pointer never does; this is how a script
/// puts the playhead where it wants a key recorded.
#[operator(
    id = "clip.seek",
    label = "Seek Playhead",
    description = "Park the playhead at a time in the clip.",
    allows_undo = false,
    params(
        time(f64, doc = "Seconds from the start of the clip."),
        frame(f64, doc = "The frame at the snap rate, instead of a time."),
    )
)]
pub(crate) fn clip_seek(
    params: In<OperatorParameters>,
    snap: Res<TimelineSnap>,
    mut seek: MessageWriter<jackdaw_animation::AnimationSeek>,
) -> OperatorResult {
    let wanted = match params.as_float("time") {
        Some(time) => time as f32,
        None => {
            let frame = params.as_float("frame")? as f32;
            if snap.rate <= 0.0 {
                return OperatorResult::Cancelled;
            }
            frame / snap.rate
        }
    };
    seek.write(jackdaw_animation::AnimationSeek(wanted.max(0.0)));
    OperatorResult::Finished
}

/// Set the rate the sheet rounds a time to, and whether it rounds at all.
#[operator(
    id = "clip.snap",
    label = "Timeline Snapping",
    description = "Round a scrubbed or dragged time to a frame at this rate.",
    allows_undo = false,
    params(
        rate(f64, doc = "Frames per second. Omit to leave the rate alone."),
        enabled(bool, doc = "On or off. Omit to leave it as it is."),
    )
)]
pub(crate) fn clip_snap(
    params: In<OperatorParameters>,
    mut snap: ResMut<TimelineSnap>,
) -> OperatorResult {
    if let Some(rate) = params.as_float("rate") {
        snap.rate = (rate as f32).clamp(1.0, 240.0);
    }
    if let Some(enabled) = params.as_bool("enabled") {
        snap.enabled = enabled;
    }
    OperatorResult::Finished
}

/// Choose which half of the sheet is drawn.
#[operator(
    id = "clip.view",
    label = "Timeline View",
    description = "Draw the clip's keys as a dope sheet, or the chosen track's value as curves.",
    allows_undo = false,
    params(mode(String, doc = "`dopesheet` or `curves`."))
)]
pub(crate) fn clip_view(
    params: In<OperatorParameters>,
    mut view: ResMut<TimelineView>,
) -> OperatorResult {
    let Some(mode) = params.as_str("mode").and_then(TimelineView::from_name) else {
        warn!("clip.view: `mode` has to be `dopesheet` or `curves`");
        return OperatorResult::Cancelled;
    };
    *view = mode;
    OperatorResult::Finished
}

/// Put one clip up in the Timeline tab.
#[operator(
    id = "clip.select",
    label = "Show Clip",
    description = "Put one of an entity's clips up in the Timeline tab.",
    allows_undo = false,
    params(
        entity(
            Entity,
            doc = "The entity whose clip to show. Defaults to the selection."
        ),
        name(String, doc = "Which of its clips. Omit to take the first one."),
    )
)]
pub(crate) fn clip_select(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    mut selected: ResMut<SelectedClip>,
    mut dirty: ResMut<TimelineDirty>,
    children: Query<&Children>,
    clips: Query<(), With<Clip>>,
    names: Query<&Name>,
) -> OperatorResult {
    let entity = params.as_entity("entity").or_else(|| selection.primary())?;
    let wanted = params.as_str("name").filter(|name| !name.is_empty());
    let clip = children
        .get(entity)
        .into_iter()
        .flatten()
        .copied()
        .filter(|child| clips.contains(*child))
        .find(|child| match wanted {
            None => true,
            Some(wanted) => names.get(*child).is_ok_and(|name| name.as_str() == wanted),
        })?;
    selected.0 = Some(clip);
    dirty.0 = true;
    OperatorResult::Finished
}

/// Turn the onion skin on or off.
#[operator(
    id = "clip.onion_skin",
    label = "Onion Skin",
    description = "Draw the pose either side of the playhead. The switch is kept; nothing draws \
                   the neighbouring poses yet.",
    allows_undo = false,
    params(enabled(bool, doc = "On or off. Omit to flip whichever way it currently is."))
)]
pub(crate) fn clip_onion_skin(
    params: In<OperatorParameters>,
    mut onion_skin: ResMut<OnionSkin>,
) -> OperatorResult {
    onion_skin.0 = params.as_bool("enabled").unwrap_or(!onion_skin.0);
    OperatorResult::Finished
}

/// Set how much wider than its column the sheet is drawn.
#[operator(
    id = "clip.zoom",
    label = "Timeline Zoom",
    description = "Draw the sheet this many times wider than the panel, so a long clip can be \
                   read frame by frame.",
    allows_undo = false,
    params(factor(f64, doc = "A multiple of the panel's width, from 1 to 20."))
)]
pub(crate) fn clip_zoom(
    params: In<OperatorParameters>,
    mut zoom: ResMut<TimelineZoom>,
) -> OperatorResult {
    let factor = params.as_float("factor")?;
    zoom.0 = (factor as f32).clamp(1.0, MAX_ZOOM);
    OperatorResult::Finished
}

fn a_clip_is_up(selected: Res<SelectedClip>) -> bool {
    selected.0.is_some()
}

/// Write one field through the inspector's own commit path, so the edit lands
/// in the document, in undo and on the entity all at once.
fn set_field(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    field: &str,
    value: serde_json::Value,
) {
    if !crate::commands::field_edit_commit_on(world, entity, type_path, field, &value) {
        warn!("the timeline could not write {type_path}.{field} on {entity}");
        return;
    }
    mark_dirty(world);
}

/// Adding or removing one clip event, as a step undo can take back.
///
/// The event is respawned rather than restored by id: an id handed back is
/// free to be someone else's by the time undo reaches for it.
struct ClipEventEdit {
    clip: Entity,
    /// The live event, once one exists. Rewritten on every undo and redo.
    event: Option<Entity>,
    time: f32,
    name: String,
    /// Whether executing adds the event or takes it away.
    adding: bool,
}

impl ClipEventEdit {
    fn adding(clip: Entity, time: f32, name: String) -> Self {
        Self {
            clip,
            event: None,
            time,
            name,
            adding: true,
        }
    }

    fn spawn(&mut self, world: &mut World) {
        let event = world
            .spawn((
                ClipEvent {
                    time: self.time,
                    name: self.name.clone(),
                },
                Name::new(self.name.clone()),
                ChildOf(self.clip),
            ))
            .id();
        crate::scene_io::register_entity_in_ast(world, event);
        self.event = Some(event);
    }

    fn despawn(&mut self, world: &mut World) {
        if let Some(event) = self.event.take()
            && let Ok(held) = world.get_entity_mut(event)
        {
            held.despawn();
        }
    }
}

impl EditorCommand for ClipEventEdit {
    fn execute(&mut self, world: &mut World) {
        if self.adding {
            self.spawn(world);
        } else {
            self.despawn(world);
        }
    }

    fn undo(&mut self, world: &mut World) {
        if self.adding {
            self.despawn(world);
        } else {
            self.spawn(world);
        }
    }

    fn description(&self) -> &str {
        if self.adding {
            "Add clip event"
        } else {
            "Remove clip event"
        }
    }
}

fn run_event_edit(world: &mut World, edit: ClipEventEdit) {
    let mut history = world
        .remove_resource::<CommandHistory>()
        .unwrap_or_default();
    history.execute(Box::new(edit), world);
    world.insert_resource(history);
    mark_dirty(world);
}

fn mark_dirty(world: &mut World) {
    if let Some(mut dirty) = world.get_resource_mut::<TimelineDirty>() {
        dirty.0 = true;
    }
}
