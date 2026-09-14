//! Markers on a library clip: the row the first one makes under the entity
//! playing it, what a save carries, and what the editor's own playback fires.

use crate::util;

use bevy::prelude::*;
use jackdaw_animation::{ImportedClipView, SelectedClip};
use jackdaw_animation_runtime::{AnimationEvent, ClipEvent};
use jackdaw_api::prelude::*;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::PropertyValue;

const ANIMATED_FILE: &str = "jan/jan.gltf";
const CLIP: &str = "run";
/// What the row holding the clip's events answers to, spelled so two files
/// holding a clip of the same name cannot share one row.
const ROW: &str = "jan/jan.gltf#run";

/// Every animation event the editor sent, kept because a message outlives the
/// frame it was written on by one tick only.
#[derive(Resource, Default)]
struct FiredEvents(Vec<(Entity, String)>);

fn record_fired_events(mut fired: MessageReader<AnimationEvent>, mut seen: ResMut<FiredEvents>) {
    seen.0
        .extend(fired.read().map(|event| (event.entity, event.name.clone())));
}

fn call(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id);
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
}

fn editor_on_the_test_project() -> App {
    let mut app = util::editor_test_app();
    util::fixed_frame_clock(&mut app);
    app.init_resource::<FiredEvents>();
    app.add_systems(Last, record_fired_events);
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

/// The step [`util::fixed_frame_clock`] gives every frame, so a count of frames
/// stands for a span of the previewed clip's own time.
const FRAME_SECS: f64 = 0.016;

/// Carry the preview `seconds` further into its clip, in whole frames of the
/// fixed clock. The clip is 0.67 s long and does not loop, so spans stay under it.
fn play_seconds(app: &mut App, seconds: f64) {
    let frames = (seconds / FRAME_SECS).ceil() as u32;
    for _ in 0..frames {
        app.update();
    }
}

/// A selected entity with a skeleton, holding one library clip on the
/// Timeline, paused on its first frame.
fn previewing_a_library_clip(app: &mut App) -> Entity {
    let rig = app
        .world_mut()
        .spawn((Name::new("Rig"), Transform::default()))
        .id();
    app.world_mut()
        .spawn((Name::new("Bone"), Transform::default(), ChildOf(rig)));
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), rig);
    jackdaw::selection::select_only(app.world_mut(), rig);

    call(
        app,
        "animation.preview",
        &[
            ("clip", format!("{ANIMATED_FILE}#{CLIP}").into()),
            ("entity", rig.into()),
        ],
    );
    settle_until(app, "the timeline described the imported clip", |app| {
        app.world().resource::<ImportedClipView>().clip.is_some()
    });
    call(app, "animation.preview.pause", &[]);
    seek(app, 0.0);
    rig
}

fn seek(app: &mut App, time: f64) {
    call(app, "clip.seek", &[("time", time.into())]);
    app.update();
    app.update();
}

fn add_event(app: &mut App, name: &'static str, time: f64) {
    call(
        app,
        "clip.event.add",
        &[("name", name.into()), ("time", time.into())],
    );
    app.update();
    app.update();
}

/// The child of `owner` the clip's markers hang under.
fn clip_row(app: &App, owner: Entity) -> Option<Entity> {
    app.world()
        .get::<Children>(owner)
        .into_iter()
        .flatten()
        .copied()
        .find(|child| {
            app.world()
                .get::<Name>(*child)
                .is_some_and(|name| name.as_str() == ROW)
        })
}

/// Every `ClipEvent` the document would write, read off the emitted text
/// rather than off the world, so an undone edit left in the AST is caught.
fn saved_document(app: &mut App) -> String {
    jackdaw::scene_io::emit_bsn_scene_with_inline_assets(app.world_mut(), std::path::Path::new("."))
}

/// The `(time, name)` of every event under a row.
fn events_on(app: &App, row: Entity) -> Vec<(f32, String)> {
    let mut held: Vec<(f32, String)> = app
        .world()
        .get::<Children>(row)
        .into_iter()
        .flatten()
        .filter_map(|child| app.world().get::<ClipEvent>(*child))
        .map(|event| (event.time, event.name.clone()))
        .collect();
    held.sort_by(|a, b| a.0.total_cmp(&b.0));
    held
}

fn fired(app: &App) -> Vec<(Entity, String)> {
    app.world().resource::<FiredEvents>().0.clone()
}

#[test]
fn an_event_on_a_library_clip_makes_a_row_under_the_entity() {
    let mut app = editor_on_the_test_project();
    let rig = previewing_a_library_clip(&mut app);
    assert!(
        app.world().resource::<SelectedClip>().0.is_none(),
        "the test means nothing unless the clip on the timeline is the imported one"
    );

    add_event(&mut app, "hit", 0.2);

    let row = clip_row(&app, rig).expect("the first event makes the row");
    assert_eq!(
        events_on(&app, row),
        vec![(0.2, "hit".to_string())],
        "the event hangs under the row, not under the entity"
    );
    assert!(
        app.world().get::<jackdaw::EditorEntity>(row).is_none()
            && app.world().get::<jackdaw::EditorHidden>(row).is_none(),
        "the row belongs to the document, so the outliner has to show it"
    );
    assert_eq!(
        app.world().resource::<ImportedClipView>().row,
        Some(row),
        "the Events row of the read-only sheet draws off the same row"
    );

    add_event(&mut app, "step", 0.4);
    assert_eq!(
        clip_row(&app, rig),
        Some(row),
        "a second event joins the row the first one made"
    );
    assert_eq!(
        events_on(&app, row),
        vec![(0.2, "hit".to_string()), (0.4, "step".to_string())]
    );
}

#[test]
fn a_library_clip_row_comes_back_when_the_document_is_reopened() {
    let mut app = editor_on_the_test_project();
    let rig = previewing_a_library_clip(&mut app);
    add_event(&mut app, "hit", 0.2);
    let saved = clip_row(&app, rig).expect("the row the event made");
    call(&mut app, "animation.preview.stop", &[]);
    app.update();

    let text = saved_document(&mut app);
    assert!(
        text.contains("ClipEvent"),
        "the marker has to reach the document:\n{text}"
    );
    jackdaw::prefab::watcher::respawn_from_sparse_text(app.world_mut(), &text);
    app.update();

    let reopened = app
        .world_mut()
        .query_filtered::<Entity, With<Name>>()
        .iter(app.world())
        .find(|entity| {
            app.world()
                .get::<Name>(*entity)
                .is_some_and(|name| name.as_str() == "Rig")
        })
        .expect("the entity came back");
    let row = clip_row(&app, reopened).expect("and so did its clip row");
    assert_ne!(row, saved, "the reopened scene is spawned afresh");
    assert_eq!(
        events_on(&app, row),
        vec![(0.2, "hit".to_string())],
        "with the marker still on it"
    );
}

#[test]
fn one_undo_takes_back_the_event_and_the_row_it_made() {
    let mut app = editor_on_the_test_project();
    let rig = previewing_a_library_clip(&mut app);
    add_event(&mut app, "hit", 0.2);
    add_event(&mut app, "step", 0.4);

    undo(&mut app);
    let row = clip_row(&app, rig).expect("the row outlives the event that joined it");
    assert_eq!(
        events_on(&app, row),
        vec![(0.2, "hit".to_string())],
        "undoing the second event takes back only that event"
    );

    undo(&mut app);
    assert!(
        clip_row(&app, rig).is_none(),
        "undoing the first event takes back the row it made"
    );
    assert_eq!(
        app.world_mut()
            .query::<&ClipEvent>()
            .iter(app.world())
            .count(),
        0,
        "and nothing of the edit is left behind"
    );
}

#[test]
fn playing_past_a_marker_sends_an_animation_event_naming_the_entity() {
    let mut app = editor_on_the_test_project();
    let rig = previewing_a_library_clip(&mut app);
    add_event(&mut app, "hit", 0.1);

    call(&mut app, "animation.preview", &[]);
    play_seconds(&mut app, 0.2);

    assert_eq!(
        fired(&app),
        vec![(rig, "hit".to_string())],
        "the event names the entity the clip is playing on"
    );
    play_seconds(&mut app, 0.2);
    assert_eq!(
        fired(&app).len(),
        1,
        "one pass over the marker is one event: {:?}",
        fired(&app)
    );
}

#[test]
fn scrubbing_back_over_a_marker_sends_it_once_per_pass() {
    let mut app = editor_on_the_test_project();
    let rig = previewing_a_library_clip(&mut app);
    add_event(&mut app, "hit", 0.2);

    seek(&mut app, 0.1);
    assert!(
        fired(&app).is_empty(),
        "a playhead short of the marker says nothing: {:?}",
        fired(&app)
    );

    seek(&mut app, 0.3);
    assert_eq!(fired(&app), vec![(rig, "hit".to_string())]);

    seek(&mut app, 0.1);
    assert_eq!(
        fired(&app),
        vec![(rig, "hit".to_string()), (rig, "hit".to_string())],
        "scrubbing back over the marker crosses it once more, not twice"
    );

    seek(&mut app, 0.05);
    assert_eq!(
        fired(&app).len(),
        2,
        "and moving on past it says nothing again: {:?}",
        fired(&app)
    );
    let cursor = app.world().resource::<jackdaw_animation::TimelineCursor>();
    assert!(
        (cursor.seek_time - 0.05).abs() < 1e-3,
        "the transport must leave the playhead where the scrub put it, not drag \
         it back to the borrowed player's own time: {}",
        cursor.seek_time
    );
}

#[test]
fn an_undone_event_leaves_nothing_behind_for_a_save_to_write() {
    let mut app = editor_on_the_test_project();
    let rig = previewing_a_library_clip(&mut app);
    add_event(&mut app, "hit", 0.2);
    assert!(saved_document(&mut app).contains("ClipEvent"));

    undo(&mut app);

    let text = saved_document(&mut app);
    assert!(
        !text.contains("ClipEvent"),
        "an undone event must leave the document as well as the world:\n{text}"
    );
    assert!(!text.contains(ROW), "and so must the row it made:\n{text}");
    assert_eq!(clip_row(&app, rig), None);
}

#[test]
fn a_removed_event_goes_out_of_the_document_too() {
    let mut app = editor_on_the_test_project();
    let rig = previewing_a_library_clip(&mut app);
    add_event(&mut app, "hit", 0.2);
    add_event(&mut app, "step", 0.4);

    seek(&mut app, 0.4);
    call(&mut app, "clip.event.remove", &[]);
    app.update();
    app.update();

    let row = clip_row(&app, rig).expect("the row stays behind its last event");
    assert_eq!(events_on(&app, row), vec![(0.2, "hit".to_string())]);
    let text = saved_document(&mut app);
    assert!(
        !text.contains("step"),
        "a removed event must go out of the document, not only the world:\n{text}"
    );
}

#[test]
fn redoing_an_undone_event_puts_the_row_and_the_marker_back() {
    let mut app = editor_on_the_test_project();
    let rig = previewing_a_library_clip(&mut app);
    add_event(&mut app, "hit", 0.2);
    undo(&mut app);
    redo(&mut app);

    let row = clip_row(&app, rig).expect("redo makes the row again");
    assert_eq!(events_on(&app, row), vec![(0.2, "hit".to_string())]);

    add_event(&mut app, "step", 0.4);
    assert_eq!(
        clip_row(&app, rig),
        Some(row),
        "and the next event joins that row rather than making a second one"
    );
    assert_eq!(
        app.world().get::<Children>(rig).map_or(0, |kids| kids
            .iter()
            .filter(|child| app
                .world()
                .get::<Name>(*child)
                .is_some_and(|name| name.as_str() == ROW))
            .count()),
        1,
        "one row per clip"
    );
}

#[test]
fn a_clip_previewed_on_a_mannequin_takes_no_events() {
    let mut app = editor_on_the_test_project();
    let bare = app.world_mut().spawn(Name::new("Bare")).id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), bare);
    jackdaw::selection::select_only(app.world_mut(), bare);
    call(
        &mut app,
        "animation.preview",
        &[("clip", format!("{ANIMATED_FILE}#{CLIP}").into())],
    );
    settle_until(&mut app, "the clip came up on a preview model", |app| {
        app.world().resource::<ImportedClipView>().clip.is_some()
    });

    let refused = app
        .world_mut()
        .operator("clip.event.add")
        .param("name", "hit")
        .call();

    assert!(
        matches!(
            refused,
            Ok(OperatorResult::Cancelled) | Err(CallOperatorError::NotAvailable)
        ),
        "a preview model goes away with the preview, so it can hold no marker: {refused:?}"
    );
    assert_eq!(
        app.world_mut()
            .query::<&ClipEvent>()
            .iter(app.world())
            .count(),
        0,
        "and nothing was put anywhere else instead"
    );
    assert!(
        app.world().get::<Children>(bare).is_none(),
        "least of all under the selection"
    );
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
    app.update();
}

fn redo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    app.update();
    app.update();
}

/// A set whose skeleton is built at run time has nothing to play on here, so a
/// model of the clip's own file stands in. The set is still what the marker
/// belongs to, and the row has to hang under it rather than nowhere.
#[test]
fn a_set_whose_skeleton_spawns_later_still_owns_its_markers() {
    use jackdaw_animation_runtime::AnimationSet;

    let mut app = editor_on_the_test_project();
    let actor = app
        .world_mut()
        .spawn((
            Name::new("Player"),
            Transform::default(),
            AnimationSet {
                sources: vec![ANIMATED_FILE.to_string()],
                skeleton_root: "SpawnedLater".to_string(),
                ..default()
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), actor);
    jackdaw::selection::select_only(app.world_mut(), actor);

    call(
        &mut app,
        "animation.preview",
        &[("clip", format!("{ANIMATED_FILE}#{CLIP}").into())],
    );
    settle_until(&mut app, "the clip came up on a preview model", |app| {
        app.world().resource::<ImportedClipView>().clip.is_some()
    });
    call(&mut app, "animation.preview.pause", &[]);
    add_event(&mut app, "hit", 0.0);

    let row = clip_row(&app, actor).expect("the row hangs under the set that was previewed");
    let names: Vec<String> = app
        .world()
        .get::<Children>(row)
        .into_iter()
        .flatten()
        .filter_map(|&child| app.world().get::<ClipEvent>(child))
        .map(|event| event.name.clone())
        .collect();
    assert_eq!(
        names,
        vec!["hit".to_string()],
        "the marker landed on the row"
    );
}
