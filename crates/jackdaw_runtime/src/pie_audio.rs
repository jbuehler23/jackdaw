//! Muting the game from the editor. While muted every sound the game plays is
//! silenced, and unmuting gives back what the editor silenced. A sound the game
//! muted itself stays muted.

use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, PlaybackSettings, SpatialAudioSink};
use bevy::ecs::component::Mutable;
use bevy::ecs::lifecycle::Discard;
use bevy::prelude::*;
use bevy::transform::TransformSystems;

/// Whether the editor asked for the game to be silent.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GameMuted(pub(crate) bool);

/// What the editor silenced on a sound, so unmuting gives back only that.
#[derive(Component, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MutedByEditor {
    /// The playing sink was muted.
    sink: bool,
    /// The sound was muted before it started, through its settings, so its
    /// sink starts muted too.
    settings: bool,
}

pub(crate) fn install(app: &mut App) {
    app.init_resource::<GameMuted>();
    // Sounds start in the audio systems after transform propagation, so muting
    // before it silences a sound spawned this frame before it plays.
    app.add_systems(
        PostUpdate,
        (
            mute_queued,
            follow_mute::<AudioSink>,
            follow_mute::<SpatialAudioSink>,
            give_back_settings,
        )
            .before(TransformSystems::Propagate),
    );
    app.add_observer(release_on_replace::<AudioPlayer>)
        .add_observer(release_on_replace::<AudioSink>)
        .add_observer(release_on_replace::<SpatialAudioSink>);
}

pub(crate) fn set_muted(world: &mut World, muted: bool) {
    let mut current = world.get_resource_or_init::<GameMuted>();
    if current.0 != muted {
        current.0 = muted;
    }
}

/// The part of a sink muting touches.
pub(crate) trait Muting {
    fn is_muted(&self) -> bool;
    fn mute(&mut self);
    fn unmute(&mut self);
}

impl Muting for AudioSink {
    fn is_muted(&self) -> bool {
        AudioSinkPlayback::is_muted(self)
    }
    fn mute(&mut self) {
        AudioSinkPlayback::mute(self);
    }
    fn unmute(&mut self) {
        AudioSinkPlayback::unmute(self);
    }
}

impl Muting for SpatialAudioSink {
    fn is_muted(&self) -> bool {
        AudioSinkPlayback::is_muted(self)
    }
    fn mute(&mut self) {
        AudioSinkPlayback::mute(self);
    }
    fn unmute(&mut self) {
        AudioSinkPlayback::unmute(self);
    }
}

/// While muted, mute every playing sink that is not, including one the game
/// unmuted after the editor muted it. Once unmuted, unmute the sinks the editor
/// muted.
fn follow_mute<S: Component<Mutability = Mutable> + Muting>(
    muted: Res<GameMuted>,
    mut sinks: Query<(Entity, &mut S, Option<&mut MutedByEditor>)>,
    mut commands: Commands,
) {
    for (entity, mut sink, marker) in &mut sinks {
        if muted.0 {
            if sink.is_muted() {
                continue;
            }
            sink.mute();
            match marker {
                Some(mut marker) => marker.sink = true,
                None => {
                    commands.entity(entity).insert(MutedByEditor {
                        sink: true,
                        settings: false,
                    });
                }
            }
        } else if let Some(marker) = marker
            && (marker.sink || marker.settings)
        {
            sink.unmute();
        }
    }
}

/// While muted, start every sound that has not started yet muted.
fn mute_queued(
    muted: Res<GameMuted>,
    mut queued: Query<
        (Entity, &mut PlaybackSettings, Option<&mut MutedByEditor>),
        (Without<AudioSink>, Without<SpatialAudioSink>),
    >,
    mut commands: Commands,
) {
    if !muted.0 {
        return;
    }
    for (entity, mut settings, marker) in &mut queued {
        if settings.muted {
            continue;
        }
        settings.muted = true;
        match marker {
            Some(mut marker) => marker.settings = true,
            None => {
                commands.entity(entity).insert(MutedByEditor {
                    sink: false,
                    settings: true,
                });
            }
        }
    }
}

/// Once unmuted, give back the settings the editor muted and forget what it
/// silenced.
fn give_back_settings(
    muted: Res<GameMuted>,
    mut silenced: Query<(Entity, &MutedByEditor, Option<&mut PlaybackSettings>)>,
    mut commands: Commands,
) {
    if muted.0 {
        return;
    }
    for (entity, marker, settings) in &mut silenced {
        if marker.settings
            && let Some(mut settings) = settings
        {
            settings.muted = false;
        }
        commands.entity(entity).try_remove::<MutedByEditor>();
    }
}

/// Give back what the editor silenced on a sound whose player or sink goes
/// away or is replaced, so a reused entity starts over. The next mute pass
/// silences it again while muted.
fn release_on_replace<C: Component>(
    replaced: On<Discard<C>>,
    mut silenced: Query<(
        &MutedByEditor,
        Option<&mut AudioSink>,
        Option<&mut SpatialAudioSink>,
        Option<&mut PlaybackSettings>,
    )>,
    mut commands: Commands,
) {
    let entity = replaced.entity;
    let Ok((marker, sink, spatial, settings)) = silenced.get_mut(entity) else {
        return;
    };
    if marker.sink || marker.settings {
        if let Some(mut sink) = sink {
            Muting::unmute(&mut *sink);
        }
        if let Some(mut sink) = spatial {
            Muting::unmute(&mut *sink);
        }
    }
    if marker.settings
        && let Some(mut settings) = settings
    {
        settings.muted = false;
    }
    commands.entity(entity).try_remove::<MutedByEditor>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::audio::AudioSource;

    /// A sink that only holds whether it is muted.
    #[derive(Component, Default)]
    struct FakeSink {
        muted: bool,
    }

    impl Muting for FakeSink {
        fn is_muted(&self) -> bool {
            self.muted
        }
        fn mute(&mut self) {
            self.muted = true;
        }
        fn unmute(&mut self) {
            self.muted = false;
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.init_resource::<GameMuted>();
        app.add_systems(
            PostUpdate,
            (mute_queued, follow_mute::<FakeSink>, give_back_settings),
        );
        app.add_observer(release_on_replace::<AudioPlayer>);
        app
    }

    fn set(app: &mut App, muted: bool) {
        app.world_mut().resource_mut::<GameMuted>().0 = muted;
        app.update();
    }

    fn sink_muted(app: &App, entity: Entity) -> bool {
        app.world().get::<FakeSink>(entity).is_some_and(|s| s.muted)
    }

    fn settings_muted(app: &App, entity: Entity) -> bool {
        app.world()
            .get::<PlaybackSettings>(entity)
            .is_some_and(|s| s.muted)
    }

    #[test]
    fn a_queued_sound_starts_muted_and_gets_its_settings_back_on_unmute() {
        let mut app = app();
        let sound = app.world_mut().spawn(PlaybackSettings::LOOP).id();
        let own = app.world_mut().spawn(PlaybackSettings::LOOP.muted()).id();

        set(&mut app, true);
        assert!(settings_muted(&app, sound));

        set(&mut app, false);
        assert!(!settings_muted(&app, sound));
        assert!(
            settings_muted(&app, own),
            "a sound the game muted itself stays muted"
        );
    }

    #[test]
    fn a_sound_spawned_in_update_is_muted_in_the_same_frame() {
        let mut app = App::new();
        install(&mut app);
        app.world_mut().resource_mut::<GameMuted>().0 = true;
        app.add_systems(Update, |mut commands: Commands, mut once: Local<bool>| {
            if !std::mem::replace(&mut *once, true) {
                commands.spawn(PlaybackSettings::ONCE);
            }
        });
        app.update();
        let muted = app
            .world_mut()
            .query::<&PlaybackSettings>()
            .single(app.world())
            .expect("the sound")
            .muted;
        assert!(muted, "muted before the audio systems start it");
    }

    #[test]
    fn a_sink_the_game_muted_stays_muted_after_unmute() {
        let mut app = app();
        let theirs = app.world_mut().spawn(FakeSink { muted: true }).id();
        let ours = app.world_mut().spawn(FakeSink::default()).id();

        set(&mut app, true);
        assert!(sink_muted(&app, theirs) && sink_muted(&app, ours));
        assert!(app.world().get::<MutedByEditor>(theirs).is_none());

        set(&mut app, false);
        assert!(sink_muted(&app, theirs), "the game's own mute holds");
        assert!(!sink_muted(&app, ours));
        assert!(app.world().get::<MutedByEditor>(ours).is_none());
    }

    #[test]
    fn a_sink_the_game_unmutes_while_muted_is_muted_again() {
        let mut app = app();
        let sound = app.world_mut().spawn(FakeSink::default()).id();
        set(&mut app, true);
        app.world_mut()
            .get_mut::<FakeSink>(sound)
            .expect("the sink")
            .muted = false;
        app.update();
        assert!(sink_muted(&app, sound));

        set(&mut app, false);
        assert!(!sink_muted(&app, sound));
    }

    #[test]
    fn a_replaced_player_gets_its_settings_back_and_is_muted_again() {
        let mut app = app();
        let sound = app
            .world_mut()
            .spawn(AudioPlayer::<AudioSource>(Handle::default()))
            .id();
        set(&mut app, true);
        assert!(settings_muted(&app, sound));

        app.world_mut()
            .entity_mut(sound)
            .insert(AudioPlayer::<AudioSource>(Handle::default()));
        app.world_mut().flush();
        assert!(!settings_muted(&app, sound), "the replaced sound is let go");
        assert!(app.world().get::<MutedByEditor>(sound).is_none());

        app.update();
        assert!(
            settings_muted(&app, sound),
            "and silenced again while muted"
        );
        assert_eq!(
            app.world().get::<MutedByEditor>(sound).copied(),
            Some(MutedByEditor {
                sink: false,
                settings: true,
            })
        );
    }
}
