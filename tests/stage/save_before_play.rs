//! Play saves the modified scenes before it starts the game, so the game
//! loads what the editor shows, unless the project turned that off.

use crate::util::{self, OperatorResultExt as _};

use bevy::prelude::*;
use jackdaw::boot_ops::{run_op_clause, run_op_clause_as_user};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};

const ZONE_SCENE: &str = "#Zone\nbevy_transform::components::transform::Transform\n";

/// An editor on a project holding one saved scene, open in front, and one
/// unsaved edit to it.
fn edited_scene() -> (App, tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("assets")).expect("assets folder");
    let scene = tmp.path().join("assets/zone.bsn");
    std::fs::write(&scene, ZONE_SCENE).expect("scene written");

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
    jackdaw::run_config::read_run_configs(app.world_mut());
    run_op_clause(app.world_mut(), "scene.open path=assets/zone.bsn")
        .expect("the scene opens")
        .assert_finished();
    app.update();
    run_op_clause_as_user(app.world_mut(), "entity.add.group name=Props parent=Zone")
        .expect("the group dispatched")
        .assert_finished();
    app.update();
    (app, tmp, scene)
}

fn play(app: &mut App) {
    app.world_mut()
        .operator("pie.play")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .call()
        .expect("dispatch")
        .assert_finished();
    app.update();
    app.update();
}

#[test]
fn play_saves_the_edited_scene_before_the_game_starts() {
    let (mut app, _tmp, scene) = edited_scene();
    assert!(
        !std::fs::read_to_string(&scene)
            .expect("scene read")
            .contains("Props"),
        "the edit is not on disk yet"
    );

    play(&mut app);

    assert!(
        std::fs::read_to_string(&scene)
            .expect("scene read")
            .contains("Props"),
        "the file the game loads holds the edit"
    );
    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    assert!(!scenes.tabs[scenes.active].dirty, "and the tab reads saved");
}

#[test]
fn with_save_before_play_off_play_leaves_the_file_alone() {
    let (mut app, _tmp, scene) = edited_scene();
    run_op_clause(app.world_mut(), "project.toggle_save_before_play")
        .expect("the toggle dispatches")
        .assert_finished();
    app.update();

    play(&mut app);

    assert_eq!(
        std::fs::read_to_string(&scene).expect("scene read"),
        ZONE_SCENE,
        "nothing was written"
    );
}

#[test]
fn an_edited_scene_never_saved_is_not_played_and_says_why() {
    let (mut app, _tmp, scene) = edited_scene();
    run_op_clause(app.world_mut(), "scene.new")
        .expect("a new scene opens")
        .assert_finished();
    app.update();
    run_op_clause_as_user(app.world_mut(), "entity.add.group name=Crates")
        .expect("the group dispatched")
        .assert_finished();
    app.update();

    play(&mut app);

    assert_eq!(
        app.world()
            .resource::<jackdaw::status_bar::StatusNotice>()
            .text(),
        "Save the scene before playing"
    );
    assert_eq!(
        std::fs::read_to_string(&scene).expect("scene read"),
        ZONE_SCENE,
        "no other scene was written in its place"
    );
}

#[test]
fn a_new_scene_at_a_relative_path_saves_under_the_assets_folder() {
    let (mut app, tmp, _scene) = edited_scene();
    run_op_clause(app.world_mut(), "scene.new path=scenes/fresh.bsn")
        .expect("a new scene opens")
        .assert_finished();
    app.update();
    let scenes = app.world().resource::<jackdaw::scenes::Scenes>();
    let held = scenes.tabs[scenes.active]
        .path
        .clone()
        .expect("the tab has a path");
    assert!(
        held.is_absolute(),
        "the tab holds an absolute path: {}",
        held.display()
    );

    run_op_clause(app.world_mut(), "scene.save")
        .expect("the save dispatches")
        .assert_finished();
    app.update();
    assert!(
        tmp.path().join("assets/scenes/fresh.bsn").is_file(),
        "the scene lands under the project's assets"
    );
}

#[test]
fn play_saves_a_new_scene_made_at_a_relative_path() {
    let (mut app, tmp, _scene) = edited_scene();
    run_op_clause(app.world_mut(), "scene.new path=scenes/played.bsn")
        .expect("a new scene opens")
        .assert_finished();
    app.update();
    run_op_clause_as_user(app.world_mut(), "entity.add.group name=Crates")
        .expect("the group dispatched")
        .assert_finished();
    app.update();

    play(&mut app);

    let notice = app
        .world()
        .resource::<jackdaw::status_bar::StatusNotice>()
        .text()
        .to_string();
    assert!(
        !notice.contains("Not playing"),
        "save-before-play went through: {notice}"
    );
    let saved = std::fs::read_to_string(tmp.path().join("assets/scenes/played.bsn"))
        .expect("the scene was saved under assets");
    assert!(saved.contains("Crates"), "with the edit:\n{saved}");
}

#[test]
fn a_path_under_assets_is_not_doubled_and_reopens_by_the_same_path() {
    let (mut app, tmp, _scene) = edited_scene();
    run_op_clause(app.world_mut(), "scene.new path=assets/levels/a.bsn")
        .expect("a new scene opens")
        .assert_finished();
    app.update();
    run_op_clause(app.world_mut(), "scene.save")
        .expect("the save dispatches")
        .assert_finished();
    app.update();
    let saved = tmp.path().join("assets/levels/a.bsn");
    assert!(saved.is_file(), "the scene lands at {}", saved.display());
    assert!(
        !tmp.path().join("assets/assets").exists(),
        "no doubled assets folder"
    );

    let tabs = |app: &App| app.world().resource::<jackdaw::scenes::Scenes>().tabs.len();
    let open = tabs(&app);
    run_op_clause(app.world_mut(), "scene.open path=assets/levels/a.bsn")
        .expect("the open dispatches")
        .assert_finished();
    app.update();
    assert_eq!(tabs(&app), open, "the open tab answers to the same path");
}

#[test]
fn a_scene_path_that_leaves_the_project_is_refused() {
    let (mut app, _tmp, _scene) = edited_scene();
    let tabs = |app: &App| app.world().resource::<jackdaw::scenes::Scenes>().tabs.len();
    let open = tabs(&app);
    let result =
        run_op_clause(app.world_mut(), "scene.new path=../../x.bsn").expect("scene.new dispatches");
    assert_eq!(result, OperatorResult::Cancelled);
    assert_eq!(tabs(&app), open, "no tab was made");

    let result = run_op_clause(app.world_mut(), "scene.save path=../../x.bsn")
        .expect("scene.save dispatches");
    assert_eq!(result, OperatorResult::Cancelled);
}
