//! What opening a scene reports while it loads, and the overlay that shows it.

use std::path::Path;
use std::time::{Duration, Instant};

use bevy::prelude::*;
use jackdaw::progress::{
    EditorProgress, ProgressEnded, ProgressOverlay, ProgressStageBegan, begin_progress,
    finish_progress, progress_count, progress_stage,
};
use jackdaw::scenes::load_progress::{
    LOADING_MODELS, PLACING_MODELS, READING, SCENE_LOAD, SPAWNING,
};
use jackdaw_api::prelude::*;

use crate::util;
use crate::util::OperatorResultExt as _;

const TWO_MODELS: &str = r#"bevy_ecs::hierarchy::Children [
    #Dungeon
    bevy_transform::components::transform::Transform
    jackdaw_scene_types::types::GltfSource {
        path: "models/dungeon.glb",
        scene_index: 0,
    }
    ,
    #Lantern
    bevy_transform::components::transform::Transform
    jackdaw_scene_types::types::GltfSource {
        path: "jan/jan.gltf",
        scene_index: 0,
    }
]
"#;

/// The removed facade UI vocabulary, which every open refuses.
const RETIRED: &str = "#Overlay\njackdaw_ui::UiCanvas\n";

#[derive(Resource, Default)]
struct Reported {
    stages: Vec<String>,
    ended: Vec<Option<String>>,
}

fn record_progress(app: &mut App) {
    app.init_resource::<Reported>();
    app.add_observer(
        |began: On<ProgressStageBegan>, mut reported: ResMut<Reported>| {
            if began.owner == SCENE_LOAD {
                reported.stages.push(began.stage.clone());
            }
        },
    );
    app.add_observer(|ended: On<ProgressEnded>, mut reported: ResMut<Reported>| {
        if ended.owner == SCENE_LOAD {
            reported.ended.push(ended.error.clone());
        }
    });
}

fn open(app: &mut App, scene: &Path) {
    app.world_mut()
        .operator("scene.open")
        .param("path", scene.display().to_string())
        .call()
        .expect("scene.open dispatches")
        .assert_finished();
    app.update();
}

fn loading(app: &App) -> bool {
    app.world()
        .resource::<EditorProgress>()
        .is_running(SCENE_LOAD)
}

fn settle_load(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while loading(app) && Instant::now() < deadline {
        app.update();
        std::thread::sleep(Duration::from_millis(5));
    }
    app.update();
}

fn overlays(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<(), With<ProgressOverlay>>()
        .iter(app.world())
        .count()
}

fn texts(app: &mut App) -> Vec<String> {
    app.world_mut()
        .query::<&Text>()
        .iter(app.world())
        .map(|text| text.0.clone())
        .collect()
}

#[test]
fn opening_a_scene_reports_each_stage_in_order_and_then_finishes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scene = dir.path().join("models.bsn");
    std::fs::write(&scene, TWO_MODELS).expect("write the scene");
    let mut app = util::editor_test_app();
    record_progress(&mut app);

    open(&mut app, &scene);
    settle_load(&mut app);

    let reported = app.world().resource::<Reported>();
    assert_eq!(
        reported.stages,
        [READING, SPAWNING, LOADING_MODELS, PLACING_MODELS],
        "the stages come in the order the load runs them"
    );
    assert_eq!(
        reported.ended,
        [None],
        "the load ends once, without an error"
    );
    assert!(!loading(&app), "nothing is left reporting");
    assert_eq!(
        overlays(&mut app),
        0,
        "the overlay is gone once the load is over"
    );
}

#[test]
fn a_scene_that_fails_to_open_ends_its_load_with_the_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scene = dir.path().join("retired.bsn");
    std::fs::write(&scene, RETIRED).expect("write the scene");
    let mut app = util::editor_test_app();
    record_progress(&mut app);

    open(&mut app, &scene);
    settle_load(&mut app);

    let reported = app.world().resource::<Reported>();
    assert_eq!(
        reported.stages,
        [READING],
        "the load stops at the step that failed"
    );
    let [Some(error)] = reported.ended.as_slice() else {
        panic!("the load ends once, with an error: {:?}", reported.ended);
    };
    assert!(
        error.contains("retired.bsn"),
        "the error names the file: {error}"
    );
    let notice = app.world().resource::<jackdaw::status_bar::StatusNotice>();
    assert!(
        notice.is_active() && notice.text() == error,
        "the user is told why: {:?}",
        notice.text()
    );
    assert!(!loading(&app));
    assert_eq!(overlays(&mut app), 0);
}

#[test]
fn the_overlay_shows_a_running_task_and_closes_when_it_finishes() {
    const OWNER: &str = "bake";
    let mut app = util::editor_test_app();

    begin_progress(app.world_mut(), OWNER, "Baking lighting", true);
    progress_stage(app.world_mut(), OWNER, "Rendering probes", Some(10));
    progress_count(app.world_mut(), OWNER, 3, Some(10));
    app.update();

    assert_eq!(overlays(&mut app), 1, "a running task puts up the overlay");
    let backdrop = app
        .world_mut()
        .query_filtered::<&BackgroundColor, With<ProgressOverlay>>()
        .single(app.world())
        .expect("the overlay has a backdrop")
        .0;
    assert!(
        backdrop.alpha() > 0.0,
        "the backdrop dims the editor: {backdrop:?}"
    );
    let shown = texts(&mut app);
    for wanted in ["Baking lighting", "Rendering probes", "3 / 10"] {
        assert!(
            shown.iter().any(|text| text == wanted),
            "the overlay shows {wanted:?}: {shown:?}"
        );
    }

    finish_progress(app.world_mut(), OWNER);
    app.update();
    assert_eq!(overlays(&mut app), 0, "the overlay closes with the task");
}
