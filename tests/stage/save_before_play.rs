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
