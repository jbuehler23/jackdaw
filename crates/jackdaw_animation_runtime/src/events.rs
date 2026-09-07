//! Named moments in an authored clip, and the message they send as playback
//! reaches them.
//!
//! An event is a child of the clip it belongs to, so it travels with the clip
//! through the document the same way a keyframe does. What plays the clip
//! writes [`ClipPlayhead`] on the clip entity; this module turns the span
//! between one write and the next into the events it covered.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// A named moment in a clip, as a child of the clip entity.
///
/// `time` is in seconds from the clip's start, on the same scale as a
/// keyframe's.
#[derive(Component, Reflect, Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[reflect(Component, Serialize, Deserialize, Default)]
pub struct ClipEvent {
    /// Seconds from the start of the clip.
    pub time: f32,
    /// What the message carries, for whatever is listening to name.
    pub name: String,
}

/// How far through its clip playback has come, written by whatever plays it.
///
/// Runtime only: the pair of times is a per-frame reading, not something a
/// document holds.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ClipPlayhead {
    /// Where playback stood when events last fired.
    pub last: f32,
    /// Where it stands now.
    pub now: f32,
}

impl ClipPlayhead {
    /// Move the playhead to `now`, keeping where it came from.
    pub fn advance_to(&mut self, now: f32) {
        self.last = self.now;
        self.now = now;
    }

    /// Whether the span since the last reading covers `time`.
    ///
    /// The span is half open on the left so a playhead parked on a key does
    /// not fire it again next tick. A span that runs backwards is a clip that
    /// wrapped, so it covers the tail of the clip and the head of it both.
    fn covers(&self, time: f32) -> bool {
        if self.now >= self.last {
            time > self.last && time <= self.now
        } else {
            time > self.last || time <= self.now
        }
    }
}

/// Sent as playback crosses a [`ClipEvent`].
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct AnimationEvent {
    /// The entity the clip animates, or the clip itself when it has no parent.
    pub entity: Entity,
    /// The name the crossed [`ClipEvent`] carries.
    pub name: String,
}

/// Send an [`AnimationEvent`] for every [`ClipEvent`] the playhead has just
/// passed, then leave the playhead where it stands so the next tick reads the
/// span after this one.
pub fn fire_clip_events(
    mut clips: Query<(Entity, &mut ClipPlayhead, &Children)>,
    events: Query<&ClipEvent>,
    parents: Query<&ChildOf>,
    mut out: MessageWriter<AnimationEvent>,
) {
    for (clip, mut playhead, children) in &mut clips {
        let animated = parents.get(clip).map_or(clip, ChildOf::parent);
        for child in children.iter() {
            let Ok(event) = events.get(child) else {
                continue;
            };
            if playhead.covers(event.time) {
                out.write(AnimationEvent {
                    entity: animated,
                    name: event.name.clone(),
                });
            }
        }
        playhead.last = playhead.now;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world holding one clip with one event on it, under a named target.
    fn world_with_an_event_at(time: f32) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_message::<AnimationEvent>()
            .add_systems(Update, fire_clip_events);
        let target = app.world_mut().spawn(Name::new("Door")).id();
        let clip = app
            .world_mut()
            .spawn((ClipPlayhead::default(), ChildOf(target)))
            .id();
        app.world_mut().spawn((
            ClipEvent {
                time,
                name: "step".to_string(),
            },
            ChildOf(clip),
        ));
        (app, target, clip)
    }

    fn events_after(app: &mut App, clip: Entity, now: f32) -> Vec<AnimationEvent> {
        app.world_mut()
            .get_mut::<ClipPlayhead>(clip)
            .expect("the clip carries a playhead")
            .now = now;
        app.update();
        app.world()
            .resource::<Messages<AnimationEvent>>()
            .iter_current_update_messages()
            .cloned()
            .collect()
    }

    #[test]
    fn an_event_key_fires_once_when_playback_crosses_it() {
        let (mut app, target, clip) = world_with_an_event_at(0.5);

        assert!(
            events_after(&mut app, clip, 0.4).is_empty(),
            "playback short of the key should say nothing"
        );
        assert_eq!(
            events_after(&mut app, clip, 0.6),
            vec![AnimationEvent {
                entity: target,
                name: "step".to_string(),
            }],
            "the span that covers the key should send its name once"
        );
        assert!(
            events_after(&mut app, clip, 0.7).is_empty(),
            "a key already passed must not send again"
        );
    }

    #[test]
    fn a_clip_that_wrapped_fires_the_keys_on_both_sides_of_the_wrap() {
        let (mut app, _, clip) = world_with_an_event_at(0.1);
        events_after(&mut app, clip, 0.9);

        let fired = events_after(&mut app, clip, 0.2);

        assert_eq!(
            fired.len(),
            1,
            "wrapping past the key should send it: {fired:?}"
        );
    }
}
