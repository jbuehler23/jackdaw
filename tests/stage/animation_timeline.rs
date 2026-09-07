//! The Timeline tab: recording, the read-only view of an imported clip, and
//! the operators every one of its controls goes through.

use crate::util;

use bevy::prelude::*;
use jackdaw_animation::{
    AnimationTrack, Clip, ClipRecording, ImportedClipView, SelectedClip, TimelineCursor,
    TimelineDirty, Vec3Keyframe,
};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::OperatorEntity;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_scene_types::PropertyValue;

const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
const ANIMATED_FILE: &str = "jan/jan.gltf";

fn call(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id);
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
}

/// An editor with this repository's assets open, which is where the animated
/// glTF the library indexes lives.
fn editor_on_the_test_project() -> App {
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app
}

#[track_caller]
fn settle_until(app: &mut App, what: &str, ready: impl Fn(&App) -> bool) {
    for _ in 0..600 {
        if ready(app) {
            return;
        }
        app.update();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("{what} never happened");
}

/// A named entity with a clip on it, holding one key on its translation.
fn entity_with_a_clip(app: &mut App) -> Entity {
    let target = app
        .world_mut()
        .spawn((Name::new("Cube"), Transform::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), target);
    jackdaw::selection::select_only(app.world_mut(), target);
    app.update();
    call(
        app,
        "animation.toggle_keyframe",
        &[
            ("entity", target.into()),
            ("component_type_path", TRANSFORM.into()),
            ("field_path", "translation".into()),
        ],
    );
    app.update();
    app.update();
    target
}

fn keys_of(app: &mut App) -> Vec<f32> {
    let mut times: Vec<f32> = app
        .world_mut()
        .query::<&Vec3Keyframe>()
        .iter(app.world())
        .map(|key| key.time)
        .collect();
    times.sort_by(f32::total_cmp);
    times
}

#[test]
fn recording_writes_a_key_at_the_playhead_when_a_tracked_field_changes() {
    let mut app = editor_on_the_test_project();
    let target = entity_with_a_clip(&mut app);
    assert_eq!(keys_of(&mut app).len(), 1, "the clip starts with one key");

    call(&mut app, "clip.record.toggle", &[]);
    assert!(
        app.world().resource::<ClipRecording>().0,
        "the toggle should have turned recording on"
    );
    app.world_mut().resource_mut::<TimelineCursor>().seek_time = 1.0;

    // What an inspector edit leaves behind: the new value, on the entity.
    app.world_mut()
        .get_mut::<Transform>(target)
        .expect("a transform")
        .translation = Vec3::new(3.0, 0.0, 0.0);
    app.update();
    app.update();

    let times = keys_of(&mut app);
    assert_eq!(
        times.len(),
        2,
        "the edit should have written a key: {times:?}"
    );
    assert!(
        (times[1] - 1.0).abs() < 1e-4,
        "the key belongs at the playhead: {times:?}"
    );
    let recorded = app
        .world_mut()
        .query::<&Vec3Keyframe>()
        .iter(app.world())
        .find(|key| (key.time - 1.0).abs() < 1e-4)
        .expect("the recorded key")
        .value;
    assert_eq!(recorded, Vec3::new(3.0, 0.0, 0.0));
}

#[test]
fn recording_switched_off_leaves_an_edit_alone() {
    let mut app = editor_on_the_test_project();
    let target = entity_with_a_clip(&mut app);

    app.world_mut().resource_mut::<TimelineCursor>().seek_time = 1.0;
    app.world_mut()
        .get_mut::<Transform>(target)
        .expect("a transform")
        .translation = Vec3::new(3.0, 0.0, 0.0);
    app.update();
    app.update();

    assert_eq!(
        keys_of(&mut app).len(),
        1,
        "an edit with the record light off must not write a key"
    );
}

#[test]
fn an_imported_clip_shows_its_bone_tracks_read_only() {
    let mut app = editor_on_the_test_project();

    // Nothing selected wears a skeleton, so the preview gives the clip a body
    // of its own out of its file, whose bones answer to the names the clip
    // addresses.
    call(
        &mut app,
        "animation.preview",
        &[("clip", format!("{ANIMATED_FILE}#run").into())],
    );
    settle_until(
        &mut app,
        "the timeline described the imported clip",
        |app| app.world().resource::<ImportedClipView>().clip.is_some(),
    );

    let view = app.world().resource::<ImportedClipView>().clone();
    assert_eq!(view.name, "run");
    assert!(view.duration > 0.0, "{view:?}");
    assert!(
        view.curve_count > 0,
        "the skeleton group counts the clip's tracks: {view:?}"
    );
    assert!(
        !view.bones.is_empty(),
        "a clip playing on a bound skeleton should name the bones it drives: {view:?}"
    );
    assert!(
        view.bones.len() <= view.curve_count,
        "a name is read back per track, not per anything else: {view:?}"
    );
    assert!(
        app.world().resource::<SelectedClip>().0.is_none(),
        "an imported clip is not an authored one to edit"
    );
    assert_eq!(
        app.world_mut().query::<&Clip>().iter(app.world()).count(),
        0,
        "previewing must not write clip entities into the document"
    );
}

#[test]
fn stopping_the_preview_takes_the_imported_clip_off_the_timeline() {
    let mut app = editor_on_the_test_project();
    call(
        &mut app,
        "animation.preview",
        &[("clip", format!("{ANIMATED_FILE}#run").into())],
    );
    settle_until(
        &mut app,
        "the timeline described the imported clip",
        |app| app.world().resource::<ImportedClipView>().clip.is_some(),
    );

    call(&mut app, "animation.preview.stop", &[]);
    app.update();
    app.update();

    assert!(
        app.world().resource::<ImportedClipView>().clip.is_none(),
        "the read-only sheet belongs to the preview and goes with it"
    );
}

#[test]
fn every_timeline_action_has_an_operator() {
    let mut app = editor_on_the_test_project();
    entity_with_a_clip(&mut app);
    app.world_mut().spawn(jackdaw_animation::timeline_panel());
    app.world_mut().resource_mut::<TimelineDirty>().0 = true;
    app.update();
    app.update();

    let registered: Vec<String> = app
        .world_mut()
        .query::<&OperatorEntity>()
        .iter(app.world())
        .map(|op| op.id().to_string())
        .collect();

    // What the tab's buttons carry, which is what a click dispatches.
    let mut dispatched: Vec<String> = app
        .world_mut()
        .query::<&ButtonOperatorCall>()
        .iter(app.world())
        .map(|call| call.id.to_string())
        .collect();
    dispatched.sort();
    dispatched.dedup();
    assert!(
        !dispatched.is_empty(),
        "the Timeline tab should have drawn its toolbar"
    );

    // What the fields, the row labels and the two segmented toggles reach for,
    // none of which is a button and so none of which carries the call.
    let by_marker = [
        "clip.record.toggle",
        "clip.loop_mode",
        "clip.event.add",
        "clip.event.remove",
        "clip.track.enable",
        "clip.track.interpolation",
        "clip.seek",
        "clip.snap",
        "clip.view",
        "clip.select",
        "clip.onion_skin",
        "clip.zoom",
    ];

    let missing: Vec<&str> = dispatched
        .iter()
        .map(String::as_str)
        .chain(by_marker)
        .filter(|id| !registered.iter().any(|held| held == id))
        .collect();
    assert!(
        missing.is_empty(),
        "the Timeline tab names operators nothing registers: {missing:?}"
    );
}

#[test]
fn an_event_lands_on_the_clip_and_undoes() {
    let mut app = editor_on_the_test_project();
    entity_with_a_clip(&mut app);
    app.world_mut().resource_mut::<TimelineCursor>().seek_time = 0.5;

    call(&mut app, "clip.event.add", &[("name", "step".into())]);
    app.update();

    let events: Vec<(f32, String)> = app
        .world_mut()
        .query::<&jackdaw_animation::ClipEvent>()
        .iter(app.world())
        .map(|event| (event.time, event.name.clone()))
        .collect();
    assert_eq!(events, vec![(0.5, "step".to_string())]);

    app.world_mut().resource_scope(
        |world, mut history: Mut<jackdaw_commands::CommandHistory>| {
            history.undo(world);
        },
    );
    app.update();
    assert_eq!(
        app.world_mut()
            .query::<&jackdaw_animation::ClipEvent>()
            .iter(app.world())
            .count(),
        0,
        "one undo has to take the event back"
    );
}

#[test]
fn switching_a_track_off_leaves_its_keys_where_they_are() {
    let mut app = editor_on_the_test_project();
    entity_with_a_clip(&mut app);
    let track = app
        .world_mut()
        .query_filtered::<Entity, With<AnimationTrack>>()
        .single(app.world())
        .expect("one track");

    call(
        &mut app,
        "clip.track.enable",
        &[("track", track.into()), ("enabled", false.into())],
    );
    app.update();
    app.update();

    assert!(
        !app.world()
            .get::<AnimationTrack>(track)
            .expect("the track")
            .enabled
    );
    assert_eq!(
        keys_of(&mut app).len(),
        1,
        "switching a track off must not take its keys away"
    );
}
