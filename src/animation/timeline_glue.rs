//! What the Timeline tab's widgets need from the editor: the fields it reads
//! back, the selection a marquee asks for, the move a key drag reports, the
//! key an inspector edit records, and what an imported clip looks like.
//!
//! The animation crate draws and asks; nothing in it reaches for the
//! selection, the command history or the asset server, all of which live here.

use bevy::animation::AnimationTargetId;
use bevy::prelude::*;
use jackdaw_animation::{
    AnimationTrack, Clip, ClipRecording, ImportedClipView, KeyframeRetimed,
    KeyframesMarqueeSelected, SelectedClip, SelectedTrack, TimelineCursor, TimelineDirty,
    TimelineSnap, TimelineZoom,
};
use jackdaw_api::prelude::*;
use jackdaw_feathers::text_edit::{TextEditCommitEvent, TextEditValue};

use super::preview::AnimationPreview;
use crate::selection::Selection;

/// How far up from a committed field the marker that owns it can sit.
///
/// A text edit spawns its own node, so the marker goes on a wrapper; the
/// commit fires on the inner input.
const MARKER_DEPTH: usize = 4;

/// The marker of the given kind above `from`, when there is one.
fn owner<'a, M: Component>(
    from: Entity,
    markers: &'a Query<&M>,
    parents: &Query<&ChildOf>,
) -> Option<&'a M> {
    let mut at = from;
    for _ in 0..MARKER_DEPTH {
        if let Ok(marker) = markers.get(at) {
            return Some(marker);
        }
        at = parents.get(at).ok()?.parent();
    }
    None
}

/// Whether any marker of the given kind sits above `from`.
fn owned_by<M: Component>(
    from: Entity,
    markers: &Query<(), With<M>>,
    parents: &Query<&ChildOf>,
) -> bool {
    let mut at = from;
    for _ in 0..MARKER_DEPTH {
        if markers.contains(at) {
            return true;
        }
        let Ok(parent) = parents.get(at) else {
            return false;
        };
        at = parent.parent();
    }
    false
}

/// The playhead field: seconds straight into a seek.
pub(super) fn on_time_commit(
    event: On<TextEditCommitEvent>,
    markers: Query<(), With<jackdaw_animation::TimelineTimeInput>>,
    parents: Query<&ChildOf>,
    mut seek: MessageWriter<jackdaw_animation::AnimationSeek>,
) {
    if !owned_by(event.entity, &markers, &parents) {
        return;
    }
    if let Ok(time) = event.text.trim().parse::<f32>() {
        seek.write(jackdaw_animation::AnimationSeek(time.max(0.0)));
    }
}

/// The frame field: the same reading, counted at the snap rate.
pub(super) fn on_frame_commit(
    event: On<TextEditCommitEvent>,
    markers: Query<(), With<jackdaw_animation::TimelineFrameInput>>,
    parents: Query<&ChildOf>,
    snap: Res<TimelineSnap>,
    mut seek: MessageWriter<jackdaw_animation::AnimationSeek>,
) {
    if !owned_by(event.entity, &markers, &parents) {
        return;
    }
    if let Ok(frame) = event.text.trim().parse::<f32>()
        && snap.rate > 0.0
    {
        seek.write(jackdaw_animation::AnimationSeek(
            (frame / snap.rate).max(0.0),
        ));
    }
}

/// The speed field, which is authored on the clip.
pub(super) fn on_speed_commit(
    event: On<TextEditCommitEvent>,
    markers: Query<&jackdaw_animation::TimelineSpeedInput>,
    parents: Query<&ChildOf>,
    clips: Query<&Clip>,
    mut commands: Commands,
) {
    let Some(marker) = owner(event.entity, &markers, &parents) else {
        return;
    };
    let clip = marker.clip;
    let Ok(speed) = event.text.trim().parse::<f32>() else {
        return;
    };
    if clips.get(clip).is_ok_and(|held| held.speed == speed) {
        return;
    }
    commands.queue(move |world: &mut World| {
        crate::commands::field_edit_commit_on(
            world,
            clip,
            "jackdaw_animation::clip::Clip",
            "speed",
            &serde_json::json!(speed),
        );
    });
}

/// The snap rate field, which goes through the operator so a script and the
/// field agree on what a rate is.
pub(super) fn on_snap_rate_commit(
    event: On<TextEditCommitEvent>,
    markers: Query<(), With<jackdaw_animation::TimelineSnapRateInput>>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    if !owned_by(event.entity, &markers, &parents) {
        return;
    }
    if let Ok(rate) = event.text.trim().parse::<f64>() {
        commands
            .operator(super::timeline_ops::ClipSnapOp::ID)
            .param("rate", rate)
            .call();
    }
}

/// The zoom field in the footer.
pub(super) fn on_zoom_commit(
    event: On<TextEditCommitEvent>,
    markers: Query<(), With<jackdaw_animation::TimelineZoomSlider>>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    if !owned_by(event.entity, &markers, &parents) {
        return;
    }
    if let Ok(factor) = event.text.trim().parse::<f64>() {
        commands
            .operator(super::timeline_ops::ClipZoomOp::ID)
            .param("factor", factor)
            .call();
    }
}

/// The Add property row: `Component.field` starts a track, through the same
/// path the inspector's keyframe diamond takes.
pub(super) fn on_add_property_commit(
    event: On<TextEditCommitEvent>,
    markers: Query<&jackdaw_animation::TimelineAddPropertyInput>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    let Some(marker) = owner(event.entity, &markers, &parents) else {
        return;
    };
    let Some((type_path, field)) = split_property(event.text.trim()) else {
        return;
    };
    let clip = marker.clip;
    commands.queue(move |world: &mut World| {
        let Some(target) = world.get::<ChildOf>(clip).map(ChildOf::parent) else {
            return;
        };
        world
            .run_system_cached_with(
                crate::inspector::anim_diamond::toggle_keyframe,
                (target, type_path, field),
            )
            .ok();
    });
}

/// A typed `Component.field` split into the pair a track addresses a property
/// with.
///
/// A bare `translation` is taken as a `Transform` field, which is what almost
/// everything animated today is, and a full type path is left as it was typed.
fn split_property(typed: &str) -> Option<(String, String)> {
    const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
    if typed.is_empty() {
        return None;
    }
    let (head, field) = typed.rsplit_once('.')?;
    let type_path = match head {
        "Transform" => TRANSFORM.to_string(),
        other => other.to_string(),
    };
    Some((type_path, field.to_string()))
}

/// Clicking a track row chooses the track the curves view draws.
pub(super) fn on_track_row_click(
    mut event: On<Pointer<Click>>,
    rows: Query<&jackdaw_animation::TimelineTrackRow>,
    mut chosen: ResMut<SelectedTrack>,
    mut dirty: ResMut<TimelineDirty>,
) {
    let Ok(row) = rows.get(event.event_target()) else {
        return;
    };
    chosen.0 = Some(row.track);
    dirty.0 = true;
    event.propagate(false);
}

/// Clicking half of the view toggle draws that half of the sheet.
///
/// A segment reports a radio change rather than a button click, so the toggle
/// dispatches its operator from here rather than through `ButtonOperatorCall`.
pub(super) fn on_view_segment_click(
    mut event: On<Pointer<Click>>,
    segments: Query<&jackdaw_animation::TimelineViewSegment>,
    mut commands: Commands,
) {
    let Ok(segment) = segments.get(event.event_target()) else {
        return;
    };
    commands
        .operator(super::timeline_ops::ClipViewOp::ID)
        .param("mode", segment.view.as_str().to_string())
        .call();
    event.propagate(false);
}

/// Clicking half of the loop toggle writes that mode on the clip.
pub(super) fn on_loop_segment_click(
    mut event: On<Pointer<Click>>,
    segments: Query<&jackdaw_animation::TimelineLoopSegment>,
    mut commands: Commands,
) {
    let Ok(segment) = segments.get(event.event_target()) else {
        return;
    };
    commands
        .operator(super::timeline_ops::ClipLoopModeOp::ID)
        .param("mode", segment.mode.as_str().to_string())
        .call();
    event.propagate(false);
}

/// Clicking an event marker parks the playhead on it, which is also what says
/// which event `clip.event.remove` means.
pub(super) fn on_event_marker_click(
    mut event: On<Pointer<Click>>,
    markers: Query<&jackdaw_animation::TimelineEventHandle>,
    events: Query<&jackdaw_animation::ClipEvent>,
    mut seek: MessageWriter<jackdaw_animation::AnimationSeek>,
) {
    let Ok(marker) = markers.get(event.event_target()) else {
        return;
    };
    if let Ok(held) = events.get(marker.event) {
        seek.write(jackdaw_animation::AnimationSeek(held.time));
    }
    event.propagate(false);
}

/// Put a finished key drag through the document, so it undoes and saves.
///
/// The drag already wrote the live component; this writes the same value the
/// way every other field edit is written.
pub(super) fn commit_retimed_keyframes(
    mut retimed: MessageReader<KeyframeRetimed>,
    mut commands: Commands,
) {
    for moved in retimed.read().copied() {
        commands.queue(move |world: &mut World| {
            let Some(type_path) = keyframe_type_path(world, moved.keyframe) else {
                return;
            };
            crate::commands::field_edit_commit_on(
                world,
                moved.keyframe,
                type_path,
                "time",
                &serde_json::json!(moved.to),
            );
        });
    }
}

fn keyframe_type_path(world: &World, keyframe: Entity) -> Option<&'static str> {
    if world
        .get::<jackdaw_animation::Vec3Keyframe>(keyframe)
        .is_some()
    {
        Some("jackdaw_animation::clip::Vec3Keyframe")
    } else if world
        .get::<jackdaw_animation::QuatKeyframe>(keyframe)
        .is_some()
    {
        Some("jackdaw_animation::clip::QuatKeyframe")
    } else if world
        .get::<jackdaw_animation::F32Keyframe>(keyframe)
        .is_some()
    {
        Some("jackdaw_animation::clip::F32Keyframe")
    } else {
        None
    }
}

/// Turn a closed marquee into a selection.
pub(super) fn apply_marquee_selection(
    mut asked: MessageReader<KeyframesMarqueeSelected>,
    mut selection: ResMut<Selection>,
    mut commands: Commands,
) {
    for ask in asked.read() {
        if !ask.additive {
            selection.clear(&mut commands);
        }
        for &keyframe in &ask.keyframes {
            selection.extend(&mut commands, keyframe);
        }
    }
}

/// While recording, write a key at the playhead for the field just edited.
///
/// Reads the edit rather than intercepting it: the inspector has already put
/// the new value on the entity, which is exactly what the key has to hold.
pub(super) fn record_edited_fields(
    recording: Res<ClipRecording>,
    cursor: Res<TimelineCursor>,
    changed: Query<Entity, (Changed<Transform>, With<Name>)>,
    tracked: Query<(&AnimationTrack, &ChildOf)>,
    clips: Query<&ChildOf, With<Clip>>,
    mut commands: Commands,
) {
    if !recording.0 || cursor.is_playing {
        return;
    }
    for (track, on_clip) in &tracked {
        let Ok(animates) = clips.get(on_clip.parent()) else {
            continue;
        };
        let target = animates.parent();
        if !changed.contains(target) {
            continue;
        }
        let type_path = track.component_type_path.clone();
        let field = track.field_path.clone();
        commands.queue(move |world: &mut World| {
            world
                .run_system_cached_with(
                    crate::inspector::anim_diamond::toggle_keyframe,
                    (target, type_path, field),
                )
                .ok();
        });
    }
}

/// Say what the Timeline tab should show for the clip the library is
/// previewing, so a glTF clip has a read-only sheet of its own.
pub(super) fn describe_previewed_clip(
    preview: Res<AnimationPreview>,
    selected: Res<SelectedClip>,
    mut view: ResMut<ImportedClipView>,
    mut dirty: ResMut<TimelineDirty>,
    gltfs: Res<Assets<bevy::gltf::Gltf>>,
    asset_server: Res<AssetServer>,
    clips: Res<Assets<AnimationClip>>,
    targets: Query<(&AnimationTargetId, &Name)>,
) {
    let wanted = preview
        .clip()
        .filter(|_| selected.0.is_none())
        .map(|(file, name)| (file.to_string(), name.to_string()));
    let Some((file, name)) = wanted else {
        if view.clip.is_some() {
            *view = ImportedClipView::default();
            dirty.0 = true;
        }
        return;
    };
    let spec = format!("{file}#{name}");
    if view.clip.as_deref() == Some(spec.as_str()) {
        return;
    }

    let handle = asset_server.get_handle(crate::entity_ops::to_asset_path(&file));
    let clip = handle
        .and_then(|handle: Handle<bevy::gltf::Gltf>| gltfs.get(&handle))
        .and_then(|gltf| gltf.named_animations.get(name.as_str()).cloned())
        .and_then(|clip| clips.get(&clip));
    let Some(clip) = clip else {
        return;
    };

    // A clip addresses its bones by a hash of the name path, so a name can be
    // read back only where a skeleton wearing those ids is in the scene. Where
    // it is not, the count is all there is to say.
    let mut bones: Vec<String> = clip
        .curves()
        .keys()
        .filter_map(|wanted| {
            targets
                .iter()
                .find(|(id, _)| *id == wanted)
                .map(|(_, name)| name.as_str().to_string())
        })
        .collect();
    bones.sort_unstable();
    bones.dedup();

    *view = ImportedClipView {
        clip: Some(spec),
        name,
        duration: clip.duration(),
        bones,
        curve_count: clip.curves().len(),
    };
    dirty.0 = true;
}

/// Keep the toolbar's readouts on what they read: the playhead in seconds and
/// in frames, and how far the sheet is zoomed.
///
/// A text edit publishes its own value rather than taking one, so a field is
/// refreshed by writing the text under it, the way the inspector refreshes the
/// name field. A field being typed into or dragged is left alone.
pub(super) fn refresh_timeline_readouts(world: &mut World) {
    let cursor = world.resource::<TimelineCursor>();
    let seek_time = cursor.seek_time;
    let snap = *world.resource::<TimelineSnap>();
    let zoom = world.resource::<TimelineZoom>().0;

    let wanted: [(Vec<Entity>, String); 3] = [
        (
            fields_of::<jackdaw_animation::TimelineTimeInput>(world),
            format!("{seek_time:.2}"),
        ),
        (
            fields_of::<jackdaw_animation::TimelineFrameInput>(world),
            format!("{}", snap.frame_of(seek_time)),
        ),
        (
            fields_of::<jackdaw_animation::TimelineZoomSlider>(world),
            format!("{zoom:.1}"),
        ),
    ];
    let focused = world.resource::<bevy::input_focus::InputFocus>().get();
    for (holders, text) in wanted {
        for holder in holders {
            write_field(world, holder, &text, focused);
        }
    }
}

fn fields_of<M: Component>(world: &mut World) -> Vec<Entity> {
    world
        .query_filtered::<Entity, With<M>>()
        .iter(world)
        .collect()
}

/// Write `text` into the text edit under `holder`, unless the field is being
/// typed into or dragged.
fn write_field(world: &mut World, holder: Entity, text: &str, focused: Option<Entity>) {
    let Some((wrapper, inner)) = text_edit_under(world, holder) else {
        return;
    };
    if world
        .get::<jackdaw_feathers::text_edit::TextEditDragging>(wrapper)
        .is_some()
        || focused == Some(inner)
    {
        return;
    }
    if world
        .get::<TextEditValue>(holder)
        .is_some_and(|value| value.0 == text)
    {
        return;
    }
    if let Some(mut editable) = world.get_mut::<bevy::text::EditableText>(inner) {
        jackdaw_feathers::text_edit::set_text_input_value(&mut editable, text.to_string());
    }
}

/// The `(wrapper, editable)` pair of the text edit under `holder`, which sits
/// a level deeper than the marker because the field brings its own node.
fn text_edit_under(world: &World, holder: Entity) -> Option<(Entity, Entity)> {
    let mut frontier = vec![holder];
    for _ in 0..MARKER_DEPTH {
        let mut next = Vec::new();
        for entity in frontier {
            if let Some(wrapper) = world.get::<jackdaw_feathers::text_edit::TextEditWrapper>(entity)
            {
                return Some((entity, wrapper.0));
            }
            next.extend(world.get::<Children>(entity).into_iter().flatten());
        }
        frontier = next;
    }
    None
}

pub(super) fn plugin(app: &mut App) {
    app.add_observer(on_time_commit)
        .add_observer(on_frame_commit)
        .add_observer(on_speed_commit)
        .add_observer(on_snap_rate_commit)
        .add_observer(on_zoom_commit)
        .add_observer(on_add_property_commit)
        .add_observer(on_track_row_click)
        .add_observer(on_view_segment_click)
        .add_observer(on_loop_segment_click)
        .add_observer(on_event_marker_click)
        .add_systems(
            Update,
            (
                commit_retimed_keyframes,
                apply_marquee_selection,
                record_edited_fields,
                describe_previewed_clip,
                refresh_timeline_readouts,
            )
                .run_if(in_state(crate::AppState::Editor)),
        );
}
