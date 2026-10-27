use crate::util::{self, OperatorResultExt as _};

use bevy::prelude::*;
use jackdaw::boot_ops::run_op_clause;
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

fn launched(app: &App) -> RunConfig {
    jackdaw::pie::run_with_open_scene(app.world(), RunConfig::default())
}

#[test]
fn play_names_the_open_scene_by_its_asset_path() {
    let (app, _tmp) = project_with_scene("levels/meadow.bsn");

    assert_eq!(
        launched(&app)
            .env
            .get("JACKDAW_PIE_SCENE")
            .map(String::as_str),
        Some("levels/meadow.bsn")
    );
    assert_eq!(
        launched(&app)
            .env
            .get("JACKDAW_PIE_REPLACES")
            .map(String::as_str),
        Some("scene.bsn"),
        "in place of the game's main scene"
    );
}

#[test]
fn a_scene_whose_name_holds_a_hash_is_named_as_written() {
    let (app, _tmp) = project_with_scene("boss#2.bsn");

    assert_eq!(
        launched(&app)
            .env
            .get("JACKDAW_PIE_SCENE")
            .map(String::as_str),
        Some("boss#2.bsn")
    );
}

#[test]
fn with_the_main_scene_in_front_nothing_is_named() {
    let (app, _tmp) = project_with_scene("scene.bsn");

    let run = launched(&app);
    assert_eq!(run.env.get("JACKDAW_PIE_SCENE"), None);
    assert_eq!(run.env.get("JACKDAW_PIE_REPLACES"), None);
}

#[test]
fn an_untitled_scene_in_front_names_nothing_so_the_game_loads_its_own() {
    let (mut app, _tmp) = project_with_scene("meadow.bsn");
    run_op_clause(app.world_mut(), "scene.new")
        .expect("a new scene opens")
        .assert_finished();
    app.update();

    assert_eq!(launched(&app).env.get("JACKDAW_PIE_SCENE"), None);
}

#[test]
fn a_scene_the_run_configuration_names_is_kept() {
    let (app, _tmp) = project_with_scene("meadow.bsn");
    let mut run = RunConfig::default();
    run.env
        .insert("JACKDAW_PIE_SCENE".to_string(), "boss.bsn".to_string());

    let run = jackdaw::pie::run_with_open_scene(app.world(), run);

    assert_eq!(
        run.env.get("JACKDAW_PIE_SCENE").map(String::as_str),
        Some("boss.bsn")
    );
}
