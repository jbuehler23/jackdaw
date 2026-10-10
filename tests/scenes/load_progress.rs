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
    util::settle_scene_load(app);
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

fn footer(app: &mut App) -> (String, String) {
    use jackdaw_feathers::status_bar::{StatusBarLeft, StatusBarRight};
    let left = app
        .world_mut()
        .query_filtered::<&Text, With<StatusBarLeft>>()
        .single(app.world())
        .expect("the footer's left slot")
        .0
        .clone();
    let right = app
        .world_mut()
        .query_filtered::<&Text, With<StatusBarRight>>()
        .single(app.world())
        .expect("the footer's right slot")
        .0
        .clone();
    (left, right)
}

#[test]
fn the_footer_words_a_running_task_as_its_overlay_does() {
    const OWNER: &str = "bake";
    let mut app = util::editor_test_app();
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    app.update();
    let (idle_left, _) = footer(&mut app);

    begin_progress(app.world_mut(), OWNER, "Baking lighting", true);
    progress_stage(app.world_mut(), OWNER, "Rendering probes", Some(10));
    progress_count(app.world_mut(), OWNER, 3, Some(10));
    app.update();
    assert_eq!(
        footer(&mut app),
        (
            "Baking lighting".to_string(),
            "Rendering probes 3 / 10".to_string()
        ),
        "the footer names the task and counts it the way the card does"
    );

    finish_progress(app.world_mut(), OWNER);
    app.update();
    app.update();
    assert_eq!(
        footer(&mut app).0,
        idle_left,
        "the left slot goes back to its own wording"
    );
}

#[test]
fn an_open_dialog_keeps_the_pointer_and_the_card_moves_beside_it() {
    use jackdaw_feathers::dialog::OpenDialogEvent;
    const OWNER: &str = "bake";
    let mut app = util::editor_test_app();
    begin_progress(app.world_mut(), OWNER, "Baking lighting", true);
    progress_stage(app.world_mut(), OWNER, "Rendering probes", Some(10));
    app.update();

    app.world_mut()
        .commands()
        .trigger(OpenDialogEvent::new("Legacy Scene Format", "Convert"));
    app.world_mut().flush();
    app.update();
    app.update();

    let (z, backdrop, pickable) = app
        .world_mut()
        .query_filtered::<(&GlobalZIndex, &BackgroundColor, Option<&Pickable>), With<ProgressOverlay>>()
        .single(app.world())
        .map(|(z, color, pickable)| (z.0, color.0, pickable.copied()))
        .expect("the overlay stays up");
    assert!(z > 200, "the card draws above the dialog's backdrop: z {z}");
    assert_eq!(
        backdrop.alpha(),
        0.0,
        "the overlay leaves the dimming to the dialog"
    );
    assert_eq!(
        pickable,
        Some(Pickable::IGNORE),
        "the pointer reaches the dialog"
    );
    assert!(
        texts(&mut app)
            .iter()
            .any(|text| text == "Rendering probes"),
        "the stage is still shown"
    );
}

const GROUP_WITH_A_LEVEL_THAT_NEVER_DRAWS: &str = r#"bevy_ecs::hierarchy::Children [
    #Wall
    bevy_transform::components::transform::Transform
    jackdaw_scene_types::types::LodGroup {
        levels: [
            jackdaw_scene_types::types::LodLevel { screen_height: 0.25 },
            jackdaw_scene_types::types::LodLevel { screen_height: 0.25 },
        ],
    }
    bevy_ecs::hierarchy::Children [
        #LOD0
        bevy_transform::components::transform::Transform
        jackdaw_scene_types::types::GltfSource {
            path: "models/dungeon.glb",
            scene_index: 0,
        }
        ,
        #LOD1
        bevy_transform::components::transform::Transform
        jackdaw_scene_types::types::GltfSource {
            path: "models/dungeon.glb",
            scene_index: 0,
        }
    ]
]
"#;

/// A LOD level that never draws is never given a model, so the load cannot
/// wait for one: it would hold the overlay up until the stall limit.
#[test]
fn opening_a_scene_does_not_wait_for_a_lod_level_that_never_draws() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scene = dir.path().join("wall.bsn");
    std::fs::write(&scene, GROUP_WITH_A_LEVEL_THAT_NEVER_DRAWS).expect("write the scene");
    let mut app = util::editor_test_app();

    let started = Instant::now();
    open(&mut app, &scene);
    settle_load(&mut app);

    assert!(!loading(&app), "the load never finished");
    assert!(
        started.elapsed() < Duration::from_secs(30),
        "the load waited {:?} for a level that never draws",
        started.elapsed()
    );
}

const GROUP_WHOSE_COARSEST_MODEL_IS_MISSING: &str = r#"bevy_ecs::hierarchy::Children [
    #Wall
    bevy_transform::components::transform::Transform
    jackdaw_scene_types::types::LodGroup {
        levels: [
            jackdaw_scene_types::types::LodLevel { screen_height: 0.5 },
            jackdaw_scene_types::types::LodLevel { screen_height: 0.1 },
        ],
        size: 2.0,
    }
    bevy_ecs::hierarchy::Children [
        #LOD0
        bevy_transform::components::transform::Transform
        jackdaw_scene_types::types::GltfSource {
            path: "models/dungeon.glb",
            scene_index: 0,
        }
        ,
        #LOD1
        bevy_transform::components::transform::Transform
        jackdaw_scene_types::types::GltfSource {
            path: "models/missing.glb",
            scene_index: 0,
        }
    ]
]
"#;

/// The level a group stands in with while it draws nothing is the coarsest
/// one whose model loads, so a missing file neither leaves the group empty nor
/// holds the overlay up until the stall limit.
#[test]
fn a_lod_group_whose_coarsest_model_is_missing_draws_its_other_level_and_the_load_finishes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scene = dir.path().join("wall.bsn");
    std::fs::write(&scene, GROUP_WHOSE_COARSEST_MODEL_IS_MISSING).expect("write the scene");
    let mut app = viewed_editor();

    open(&mut app, &scene);
    settle_load(&mut app);

    let progress = *app.world().resource::<jackdaw_runtime::LiveLevelProgress>();
    assert_eq!(progress.groups, 1);
    assert_eq!(progress.drawn, 1, "the group draws its loaded level");
    let mut parts = app
        .world_mut()
        .query_filtered::<&jackdaw_runtime::LodPartLevel, With<jackdaw_runtime::LodPart>>();
    assert!(
        parts.iter(app.world()).any(|level| level.0 == 0),
        "the most detailed level is placed"
    );
}

/// A wanted level whose model never loads is not waited for.
#[test]
fn a_lod_group_whose_models_are_all_missing_does_not_hold_the_load_up() {
    let dir = tempfile::tempdir().expect("tempdir");
    let scene = dir.path().join("wall.bsn");
    let text = GROUP_WHOSE_COARSEST_MODEL_IS_MISSING.replace("dungeon.glb", "gone.glb");
    std::fs::write(&scene, text).expect("write the scene");
    let mut app = viewed_editor();

    open(&mut app, &scene);
    settle_load(&mut app);

    let progress = *app.world().resource::<jackdaw_runtime::LiveLevelProgress>();
    assert_eq!(progress.groups, 1);
    assert!(progress.is_drawn() && progress.is_refined(), "{progress:?}");
}

/// An editor with a viewport camera, so LOD groups want levels.
fn viewed_editor() -> App {
    let mut app = util::editor_test_app();
    app.world_mut().spawn((
        Camera3d::default(),
        jackdaw::viewport::MainViewportCamera,
        Transform::from_xyz(0.0, 0.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    app
}
