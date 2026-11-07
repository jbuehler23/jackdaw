use crate::util::{self, OperatorResultExt as _};

use bevy::prelude::*;
use jackdaw::boot_ops::{run_op_clause, run_op_clause_as_user};
use jackdaw::play_scene::{LaunchScene, PlayScene, launch_scene};
use jackdaw_pie_protocol::manifest::RunConfig;

const MEADOW_SCENE: &str = "#Meadow\nbevy_transform::components::transform::Transform\n";

fn project_with_scene(relative: &str) -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let scene = tmp.path().join("assets").join(relative);
    std::fs::create_dir_all(scene.parent().expect("a folder")).expect("assets folder");
    std::fs::write(&scene, MEADOW_SCENE).expect("scene written");

    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    run_op_clause(
        app.world_mut(),
        &format!("scene.open path=assets/{relative}"),
    )
    .expect("the scene opens")
    .assert_finished();
    app.update();
    (app, tmp)
}

fn launched(app: &App) -> Option<LaunchScene> {
    launch_scene(app.world(), &RunConfig::default())
}

fn in_place_of_main(scene: &str) -> Option<LaunchScene> {
    Some(LaunchScene {
        replaces: "scene.bsn".to_string(),
        scene: PlayScene::Path(scene.to_string()),
    })
}

#[test]
fn play_names_the_open_scene_by_its_asset_path() {
    let (app, _tmp) = project_with_scene("levels/meadow.bsn");
    assert_eq!(launched(&app), in_place_of_main("levels/meadow.bsn"));
}

#[test]
fn a_scene_whose_name_holds_a_hash_is_named_as_written() {
    let (app, _tmp) = project_with_scene("boss#2.bsn");
    assert_eq!(launched(&app), in_place_of_main("boss#2.bsn"));
}

#[test]
fn with_the_main_scene_in_front_nothing_is_named() {
    let (app, _tmp) = project_with_scene("scene.bsn");
    assert_eq!(launched(&app), None);
}

#[test]
fn an_untitled_scene_in_front_names_nothing_so_the_game_loads_its_own() {
    let (mut app, _tmp) = project_with_scene("meadow.bsn");
    run_op_clause(app.world_mut(), "scene.new")
        .expect("a new scene opens")
        .assert_finished();
    app.update();

    assert_eq!(launched(&app), None);
}

#[test]
fn a_scene_the_run_configuration_names_is_kept() {
    let (app, _tmp) = project_with_scene("meadow.bsn");
    let mut run = RunConfig::default();
    run.env
        .insert("JACKDAW_PIE_SCENE".to_string(), "boss.bsn".to_string());

    assert_eq!(
        launch_scene(app.world(), &run),
        in_place_of_main("boss.bsn")
    );
}

#[test]
fn project_settings_name_scenes_over_the_run_configuration() {
    let (mut app, _tmp) = project_with_scene("meadow.bsn");
    {
        let mut settings = app
            .world_mut()
            .resource_mut::<jackdaw::play_settings::PlaySettings>();
        settings.env.insert(
            "JACKDAW_PIE_SCENE".to_string(),
            "levels\\boss.bsn".to_string(),
        );
        settings
            .env
            .insert("JACKDAW_PIE_REPLACES".to_string(), "level.bsn".to_string());
    }
    let mut run = RunConfig::default();
    run.env
        .insert("JACKDAW_PIE_SCENE".to_string(), "other.bsn".to_string());

    assert_eq!(
        launch_scene(app.world(), &run),
        Some(LaunchScene {
            replaces: "level.bsn".to_string(),
            scene: PlayScene::Path("levels/boss.bsn".to_string()),
        })
    );
}

#[test]
fn an_empty_scene_name_names_nothing() {
    let (app, _tmp) = project_with_scene("meadow.bsn");
    let mut run = RunConfig::default();
    run.env
        .insert("JACKDAW_PIE_SCENE".to_string(), String::new());

    assert_eq!(
        launch_scene(app.world(), &run),
        in_place_of_main("meadow.bsn")
    );
}

/// A scene in the old form plays from its document, under the name it has.
#[test]
fn a_scene_in_the_old_form_is_served_unsaved() {
    let (mut app, tmp) = project_with_scene("zone.bsn");
    let legacy = tmp.path().join("assets/zone.jsn");
    std::fs::write(&legacy, "{}").expect("the old file");
    std::fs::remove_file(tmp.path().join("assets/zone.bsn")).expect("the new name is free");
    {
        let mut scenes = app.world_mut().resource_mut::<jackdaw::scenes::Scenes>();
        let active = scenes.active;
        scenes.tabs[active].path = Some(legacy.clone());
    }
    run_op_clause_as_user(app.world_mut(), "entity.add.group name=Props")
        .expect("the group dispatched")
        .assert_finished();
    app.update();

    let launch = launch_scene(app.world(), &RunConfig::default()).expect("a scene to play");
    assert_eq!(launch.scene, PlayScene::Path("zone.jsn".to_string()));
    let answer = jackdaw::play_scene::scene_answer(app.world_mut(), Some(&launch), "scene.bsn");
    let jackdaw_pie_protocol::SceneAnswer::Document { text, .. } = answer else {
        panic!("not the document: {answer:?}");
    };
    assert!(text.contains("Props"), "{text}");
    assert_eq!(
        std::fs::read_to_string(&legacy).expect("the old file"),
        "{}"
    );
}
