//! Muting the game from the editor. While muted every sound the game plays is
//! silenced, and unmuting gives each one back the volume it had.

use bevy::prelude::*;

/// Whether the editor asked for the game to be silent.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GameMuted(pub(crate) bool);

pub(crate) fn install(app: &mut App) {
    app.init_resource::<GameMuted>();
    #[cfg(feature = "audio")]
    app.add_systems(Update, follow_mute::apply_mute);
}

pub(crate) fn set_muted(world: &mut World, muted: bool) {
    let mut current = world.get_resource_or_init::<GameMuted>();
    if current.0 != muted {
        current.0 = muted;
    }
}

#[cfg(feature = "audio")]
mod follow_mute {
    use bevy::audio::{AudioSink, AudioSinkPlayback, PlaybackSettings, SpatialAudioSink};
    use bevy::prelude::*;

    use super::GameMuted;

    /// On a sound the editor silenced, so unmuting leaves the game's own
    /// muted sounds alone.
    #[derive(Component)]
    pub(super) struct MutedByEditor;

    /// Silence playing sounds and sounds about to start while muted, and give
    /// back what was silenced once unmuted. Sounds start in `PostUpdate`, so a
    /// queued one is muted before its sink exists.
    pub(super) fn apply_mute(
        muted: Res<GameMuted>,
        mut sinks: Query<(Entity, &mut AudioSink), Without<MutedByEditor>>,
        mut spatial: Query<(Entity, &mut SpatialAudioSink), Without<MutedByEditor>>,
        mut queued: Query<
            (Entity, &mut PlaybackSettings),
            (
                Without<AudioSink>,
                Without<SpatialAudioSink>,
                Without<MutedByEditor>,
            ),
        >,
        mut silenced: Query<
            (
                Entity,
                Option<&mut AudioSink>,
                Option<&mut SpatialAudioSink>,
                Option<&mut PlaybackSettings>,
            ),
            With<MutedByEditor>,
        >,
        mut commands: Commands,
    ) {
        if muted.0 {
            for (entity, mut sink) in &mut sinks {
                if !sink.is_muted() {
                    sink.mute();
                    commands.entity(entity).insert(MutedByEditor);
                }
            }
            for (entity, mut sink) in &mut spatial {
                if !sink.is_muted() {
                    sink.mute();
                    commands.entity(entity).insert(MutedByEditor);
                }
            }
            for (entity, mut settings) in &mut queued {
                if !settings.muted {
                    settings.muted = true;
                    commands.entity(entity).insert(MutedByEditor);
                }
            }
            return;
        }
        for (entity, sink, spatial, settings) in &mut silenced {
            if let Some(mut sink) = sink {
                sink.unmute();
            }
            if let Some(mut sink) = spatial {
                sink.unmute();
            }
            if let Some(mut settings) = settings {
                settings.muted = false;
            }
            commands.entity(entity).remove::<MutedByEditor>();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_queued_sound_starts_muted_and_gets_its_settings_back_on_unmute() {
            let mut app = App::new();
            app.init_resource::<GameMuted>();
            app.add_systems(Update, apply_mute);
            let sound = app.world_mut().spawn(PlaybackSettings::LOOP).id();
            let own = app.world_mut().spawn(PlaybackSettings::LOOP.muted()).id();

            app.world_mut().resource_mut::<GameMuted>().0 = true;
            app.update();
            assert!(app.world().get::<PlaybackSettings>(sound).unwrap().muted);

            app.world_mut().resource_mut::<GameMuted>().0 = false;
            app.update();
            assert!(!app.world().get::<PlaybackSettings>(sound).unwrap().muted);
            assert!(
                app.world().get::<PlaybackSettings>(own).unwrap().muted,
                "a sound the game muted itself stays muted"
            );
        }
    }
}
