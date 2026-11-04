//! Clip event markers away from an authored clip: the row an imported clip's
//! events hang under, and the events the editor's own playback crosses.
//!
//! A glTF clip is not an entity the document holds, so its markers cannot hang
//! under it. They hang under a named child of the entity the clip plays on,
//! which is the same row [`jackdaw_animation_runtime`] looks for in a running
//! game.

use bevy::prelude::*;
use jackdaw_animation::{
    AnimationStop, ClipEvent, FiredClipEvents, ImportedClipView, SelectedClip, TimelineCursor,
    TimelineEngagement,
};
use jackdaw_animation_runtime::{AnimationEvent, ClipPass, ClipPlayhead, stands_for_clip};

use super::preview::AnimationPreview;
use crate::status_bar::StatusNotice;

/// The clip-event row the Timeline is driving, so a fired event can be traced
/// back to the marker that stands for it.
#[derive(Resource, Default, Debug)]
struct DrivenClipRow(Option<Entity>);

/// What a row holding a library clip's events is called, which is the spelling
/// the runtime reads back when two files hold the same clip name.
pub(super) fn clip_event_row_name(file: &str, clip: &str) -> String {
    format!("{file}#{clip}")
}

/// The child of `owner` carrying `clip`'s events: named for the clip and, unlike
/// a bone or a prefab part, holding no place in the scene and so no `Transform`.
pub(super) fn clip_event_row(
    owner: Entity,
    file: &str,
    clip: &str,
    children: &Query<&Children>,
    names: &Query<&Name>,
    placed: &Query<(), With<Transform>>,
) -> Option<Entity> {
    children
        .get(owner)
        .into_iter()
        .flatten()
        .copied()
        .find(|child| {
            !placed.contains(*child)
                && names
                    .get(*child)
                    .is_ok_and(|name| stands_for_clip(name.as_str(), file, clip))
        })
}

/// The same, read off a world rather than a query.
pub(super) fn clip_event_row_in(
    world: &World,
    owner: Entity,
    file: &str,
    clip: &str,
) -> Option<Entity> {
    world
        .get::<Children>(owner)
        .into_iter()
        .flatten()
        .copied()
        .find(|child| {
            world.get::<Transform>(*child).is_none()
                && world
                    .get::<Name>(*child)
                    .is_some_and(|name| stands_for_clip(name.as_str(), file, clip))
        })
}

/// Move the playhead of the row the Timeline is showing, so its events fire in
/// the editor as they would in a game, and only where the runtime writes none.
fn write_shown_clip_playhead(
    selected: Res<SelectedClip>,
    engagement: Res<TimelineEngagement>,
    imported: Res<ImportedClipView>,
    cursor: Res<TimelineCursor>,
    preview: Res<AnimationPreview>,
    mut rewinds: MessageReader<AnimationStop>,
    mut driven: ResMut<DrivenClipRow>,
    marked: Query<(), With<ClipEvent>>,
    children: Query<&Children>,
    mut playheads: Query<&mut ClipPlayhead>,
    mut commands: Commands,
) {
    let rewound = rewinds.read().last().is_some();
    let transport_holds_the_player = *engagement == TimelineEngagement::Active;
    let row = selected
        .0
        .filter(|_| transport_holds_the_player)
        .or(imported.row)
        .filter(|row| {
            children
                .get(*row)
                .is_ok_and(|keys| keys.iter().any(|key| marked.contains(key)))
        });
    let row_left_behind = driven.0.filter(|left| Some(*left) != row);
    if let Some(left) = row_left_behind
        && let Ok(mut held) = commands.get_entity(left)
    {
        held.try_remove::<ClipPlayhead>();
    }
    let Some(row) = row else {
        driven.0 = None;
        return;
    };

    let seek = cursor.seek_time.max(0.0);
    let taken_up_this_frame = rewound || driven.0 != Some(row);
    driven.0 = Some(row);
    let seeded = ClipPlayhead {
        last: seek,
        now: seek,
        pass: ClipPass::Forward,
    };
    match playheads.get_mut(row) {
        Err(_) => {
            commands.entity(row).insert(seeded);
        }
        Ok(mut playhead) if taken_up_this_frame => *playhead = seeded,
        Ok(mut playhead) => {
            let playing = cursor.is_playing || preview.is_playing();
            let pass = pass_between(playhead.now, seek, playing);
            playhead.advance_to(seek, pass);
        }
    }
}

/// How the playhead travelled between two readings: playback runs forward, so a
/// reading behind the last one wrapped, while a parked one was scrubbed back.
fn pass_between(previous: f32, now: f32, playing: bool) -> ClipPass {
    match (now < previous, playing) {
        (true, true) => ClipPass::ForwardWrapped,
        (true, false) => ClipPass::Backward,
        (false, _) => ClipPass::Forward,
    }
}

/// Light the marker of every event the shown clip has just crossed, and say
/// which one it was.
fn report_fired_clip_events(
    mut fired: MessageReader<AnimationEvent>,
    driven: Res<DrivenClipRow>,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
    events: Query<&ClipEvent>,
    mut lit: ResMut<FiredClipEvents>,
    mut notice: ResMut<StatusNotice>,
) {
    let Some(row) = driven.0 else {
        fired.clear();
        return;
    };
    let animated = parents.get(row).map_or(row, ChildOf::parent);
    for message in fired.read().filter(|message| message.entity == animated) {
        let crossed: Vec<(Entity, f32)> = children
            .get(row)
            .into_iter()
            .flatten()
            .filter_map(|child| {
                events
                    .get(*child)
                    .ok()
                    .filter(|event| event.name == message.name)
                    .map(|event| (*child, event.time))
            })
            .collect();
        for (event, _) in &crossed {
            lit.light(*event);
        }
        if let Some((_, time)) = crossed.first() {
            notice.show(format!("{} at {time:.2} s", message.name));
        }
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<DrivenClipRow>().add_systems(
        Update,
        (
            write_shown_clip_playhead
                .after(jackdaw_animation::sync_cursor_from_player)
                .after(super::preview::drive_preview_player)
                .after(jackdaw_animation_runtime::write_clip_playheads)
                .before(jackdaw_animation_runtime::fire_clip_events),
            report_fired_clip_events.after(jackdaw_animation_runtime::fire_clip_events),
        )
            .run_if(in_state(crate::AppState::Editor)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_playing_clip_that_reads_back_has_wrapped() {
        assert_eq!(pass_between(0.9, 0.1, true), ClipPass::ForwardWrapped);
        assert_eq!(pass_between(0.1, 0.9, true), ClipPass::Forward);
    }

    #[test]
    fn a_parked_clip_that_reads_back_was_scrubbed_back() {
        assert_eq!(pass_between(0.9, 0.1, false), ClipPass::Backward);
        assert_eq!(pass_between(0.1, 0.9, false), ClipPass::Forward);
    }

    #[test]
    fn a_playhead_that_did_not_move_reads_as_a_forward_step() {
        assert_eq!(pass_between(0.5, 0.5, true), ClipPass::Forward);
        assert_eq!(pass_between(0.5, 0.5, false), ClipPass::Forward);
    }

    #[test]
    fn a_row_name_tells_two_files_holding_the_same_clip_apart() {
        let row = clip_event_row_name("jan/jan.gltf", "run");
        assert!(stands_for_clip(&row, "jan/jan.gltf", "run"));
        assert!(!stands_for_clip(&row, "other/other.gltf", "run"));
    }

    #[test]
    fn a_bone_named_like_the_clip_is_not_the_event_row() {
        let mut world = World::new();
        let rig = world.spawn(Name::new("Rig")).id();
        world.spawn((Name::new("run"), Transform::default(), ChildOf(rig)));
        assert_eq!(clip_event_row_in(&world, rig, "jan/jan.gltf", "run"), None);

        let row = world.spawn((Name::new("run"), ChildOf(rig))).id();
        assert_eq!(
            clip_event_row_in(&world, rig, "jan/jan.gltf", "run"),
            Some(row)
        );
    }
}
