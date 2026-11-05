//! What the editor answers when a running game asks for a scene, spawned by the
//! runtime's own loader where the answer is a document.

use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw::boot_ops::{run_op_clause, run_op_clause_as_user};
use jackdaw::play_scene::{LaunchScene, PlayScene, launch_scene, scene_answer};
use jackdaw::play_sync::collect_node_changes;
use jackdaw::scenes::{Scenes, TabKind, TabRefusal};
use jackdaw_api::prelude::*;
use jackdaw_pie_protocol::manifest::RunConfig;
use jackdaw_pie_protocol::{ControlEvent, SceneAnswer};
use jackdaw_runtime::node_sync::{apply_node_components, install_node_index};
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};
use jackdaw_scene_types::SceneNodeId;

use crate::util::{self, OperatorResultExt as _};

const ZONE: &str = "#Zone\nbevy_transform::components::transform::Transform\n";
const MEADOW: &str = "#Meadow\nbevy_transform::components::transform::Transform\n";

/// An editor on a project with `zone.bsn` and `levels/meadow.bsn` on disk, the
/// first open in front. It stays out of the editor state, where the sync system
/// would forget its baseline with no game running.
fn editor() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join("assets/levels")).expect("folders");
    std::fs::write(tmp.path().join("assets/zone.bsn"), ZONE).expect("a scene");
    std::fs::write(tmp.path().join("assets/levels/meadow.bsn"), MEADOW).expect("a scene");
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    open(&mut app, "assets/zone.bsn");
    (app, tmp)
}

fn open(app: &mut App, path: &str) {
    run_op_clause(app.world_mut(), &format!("scene.open path={path}"))
        .expect("the scene opens")
        .assert_finished();
    app.update();
}

fn zone_file(tmp: &tempfile::TempDir) -> PathBuf {
    tmp.path().join("assets/zone.bsn")
}

fn entity_named(app: &mut App, name: &str) -> Entity {
    app.world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, held)| held.as_str() == name)
        .map(|(entity, _)| entity)
        .expect("the entity is spawned")
}

fn move_to(app: &mut App, entity: Entity, x: f64) {
    app.world_mut()
        .operator("entity.set_transform")
        .param("entity", entity)
        .param("x", x)
        .call()
        .expect("entity.set_transform dispatches")
        .assert_finished();
    app.update();
}

fn document(answer: SceneAnswer) -> (String, String, String) {
    match answer {
        SceneAnswer::Document { text, dir, stem } => (text, dir, stem),
        SceneAnswer::UseFile { path } => panic!("expected a document, got the file {path}"),
    }
}

fn use_file(path: &str) -> SceneAnswer {
    SceneAnswer::UseFile {
        path: path.to_string(),
    }
}

/// A headless game with the node index, as the link installs it, holding one
/// root spawned from `text`.
fn game_spawning(text: String) -> App {
    let mut game = App::new();
    game.add_plugins(MinimalPlugins);
    game.add_plugins(bevy::transform::TransformPlugin);
    game.add_plugins(bevy::asset::AssetPlugin::default());
    game.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    install_node_index(&mut game);
    game.add_plugins(JackdawPlugin);
    let scene = game
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(text, PathBuf::new()));
    game.world_mut().spawn(JackdawSceneRoot(scene));
    game.update();
    game
}

fn game_x(game: &mut App, name: &str) -> f32 {
    game.world_mut()
        .query::<(&Name, &Transform)>()
        .iter(game.world())
        .find(|(held, _)| held.as_str() == name)
        .map(|(_, transform)| transform.translation.x)
        .expect("the entity is in the game")
}

/// A file with no node ids, unedited: the game gets the ids from the document,
/// so an edit reaches its entity, and no file is written.
#[test]
fn an_unedited_file_without_ids_syncs_and_is_not_written() {
    let (mut app, tmp) = editor();
    let zone = entity_named(&mut app, "Zone");
    assert!(collect_node_changes(app.world_mut()).is_empty());

    let (text, dir, stem) = document(scene_answer(app.world_mut(), None, "zone.bsn"));
    assert_eq!((dir.as_str(), stem.as_str()), ("", "zone"));
    assert!(text.contains("SceneNodeId"), "{text}");
    let mut game = game_spawning(text);
    assert_eq!(game_x(&mut game, "Zone"), 0.0);

    move_to(&mut app, zone, 4.0);
    for event in collect_node_changes(app.world_mut()) {
        let ControlEvent::NodeComponents { node, set, removed } = event else {
            panic!("a node edit");
        };
        apply_node_components(game.world_mut(), node, &set, &removed);
    }
    assert_eq!(
        game_x(&mut game, "Zone"),
        4.0,
        "the edit reached the entity"
    );
    let ids: Vec<u64> = game
        .world_mut()
        .query::<&SceneNodeId>()
        .iter(game.world())
        .map(|id| id.0)
        .collect();
    assert_eq!(ids.len(), 1);
    assert_eq!(
        std::fs::read_to_string(zone_file(&tmp)).expect("the file"),
        ZONE,
        "no file written"
    );
}

/// The unsaved edit comes back on a respawn, and the tab still reads unsaved.
#[test]
fn an_unsaved_edit_is_served_and_stays_unsaved() {
    let (mut app, tmp) = editor();
    let zone = entity_named(&mut app, "Zone");
    run_op_clause_as_user(app.world_mut(), "entity.add.group name=Props parent=Zone")
        .expect("the group dispatched")
        .assert_finished();
    app.update();
    move_to(&mut app, zone, 6.0);

    for _ in 0..2 {
        let (text, ..) = document(scene_answer(app.world_mut(), None, "zone.bsn"));
        assert_eq!(game_x(&mut game_spawning(text), "Zone"), 6.0);
    }
    assert!(jackdaw::scenes::operators::active_tab_modified(app.world()));
    assert_eq!(
        std::fs::read_to_string(zone_file(&tmp)).expect("the file"),
        ZONE,
        "file bytes unchanged"
    );
}

#[test]
fn an_inactive_tab_answers_with_its_document_and_its_folder() {
    let (mut app, _tmp) = editor();
    open(&mut app, "assets/levels/meadow.bsn");
    let meadow = entity_named(&mut app, "Meadow");
    move_to(&mut app, meadow, 3.0);
    open(&mut app, "assets/zone.bsn");

    let (text, dir, stem) = document(scene_answer(app.world_mut(), None, "levels/meadow.bsn"));
    assert_eq!((dir.as_str(), stem.as_str()), ("levels", "meadow"));
    assert_eq!(game_x(&mut game_spawning(text), "Meadow"), 3.0);

    let (front, ..) = document(scene_answer(app.world_mut(), None, "zone.bsn"));
    assert_eq!(game_x(&mut game_spawning(front), "Zone"), 0.0);
}

#[test]
fn a_scene_not_open_refused_or_a_prefab_uses_its_file() {
    let (mut app, _tmp) = editor();
    assert_eq!(
        scene_answer(app.world_mut(), None, "levels/meadow.bsn"),
        use_file("levels/meadow.bsn"),
        "not open"
    );

    let set_tab = |app: &mut App, change: &dyn Fn(&mut jackdaw::scenes::SceneTab)| {
        let mut scenes = app.world_mut().resource_mut::<Scenes>();
        let active = scenes.active;
        change(&mut scenes.tabs[active]);
    };
    set_tab(&mut app, &|tab| {
        tab.refusal = Some(TabRefusal::Rejected("no".to_string()));
    });
    assert_eq!(
        scene_answer(app.world_mut(), None, "zone.bsn"),
        use_file("zone.bsn"),
        "refused"
    );
    set_tab(&mut app, &|tab| {
        tab.refusal = None;
        tab.kind = TabKind::Prefab;
    });
    assert_eq!(
        scene_answer(app.world_mut(), None, "zone.bsn"),
        use_file("zone.bsn"),
        "a prefab tab is not a scene"
    );
}

/// An edited scene with no file plays in place of the game's usual scene, in
/// front or not. A root asking for anything else gets its own scene.
#[test]
fn a_modified_untitled_scene_replaces_only_the_usual_scene() {
    let (mut app, _tmp) = editor();
    run_op_clause(app.world_mut(), "scene.new")
        .expect("a new scene opens")
        .assert_finished();
    app.update();
    assert_eq!(
        launch_scene(app.world(), &RunConfig::default()),
        None,
        "nothing to play until it is edited"
    );
    run_op_clause_as_user(app.world_mut(), "entity.add.group name=Crates")
        .expect("the group dispatched")
        .assert_finished();
    app.update();

    let launch = launch_scene(app.world(), &RunConfig::default()).expect("a launch scene");
    let PlayScene::Untitled(name) = &launch.scene else {
        panic!("the untitled scene: {launch:?}");
    };
    assert_eq!(launch.replaces, "scene.bsn");
    let (text, dir, stem) = document(scene_answer(app.world_mut(), Some(&launch), "scene.bsn"));
    assert_eq!((dir.as_str(), stem.as_str()), ("", name.as_str()));
    assert_eq!(game_x(&mut game_spawning(text), "Crates"), 0.0);

    assert_eq!(
        scene_answer(app.world_mut(), Some(&launch), "levels/meadow.bsn"),
        use_file("levels/meadow.bsn"),
        "another root keeps its own scene"
    );
    let (zone, ..) = document(scene_answer(app.world_mut(), Some(&launch), "zone.bsn"));
    assert!(zone.contains("Zone"), "{zone}");

    open(&mut app, "assets/zone.bsn");
    let (behind, ..) = document(scene_answer(app.world_mut(), Some(&launch), "scene.bsn"));
    assert!(
        behind.contains("Crates"),
        "still served from behind: {behind}"
    );
}

#[test]
fn a_save_as_during_the_build_falls_back_to_the_file_and_warns() {
    let (mut app, tmp) = editor();
    run_op_clause(app.world_mut(), "scene.new")
        .expect("a new scene opens")
        .assert_finished();
    app.update();
    run_op_clause_as_user(app.world_mut(), "entity.add.group name=Crates")
        .expect("the group dispatched")
        .assert_finished();
    app.update();
    let launch = launch_scene(app.world(), &RunConfig::default()).expect("a launch scene");

    jackdaw::scene_io::save::retarget_active_scene(
        app.world_mut(),
        &tmp.path().join("assets/saved.bsn").to_string_lossy(),
    )
    .expect("the tab takes the file");

    assert_eq!(
        scene_answer(app.world_mut(), Some(&launch), "scene.bsn"),
        use_file("scene.bsn")
    );
    let notice = app
        .world()
        .resource::<jackdaw::status_bar::StatusNotice>()
        .text()
        .to_string();
    assert!(notice.contains("saved or closed"), "{notice}");
}

/// A game told to play a scene gets it, and the scene in front does not take
/// the place of one a run configuration names.
#[test]
fn a_launch_scene_answers_for_the_scene_it_replaces() {
    let (mut app, _tmp) = editor();
    assert_eq!(
        launch_scene(app.world(), &RunConfig::default()),
        Some(LaunchScene {
            replaces: "scene.bsn".to_string(),
            scene: PlayScene::Path("zone.bsn".to_string()),
        })
    );

    let mut named = RunConfig::default();
    named.env.insert(
        "JACKDAW_PIE_SCENE".to_string(),
        "levels/meadow.bsn".to_string(),
    );
    assert_eq!(launch_scene(app.world(), &named), None);
    let redirected = jackdaw::pie::run_with_open_scene(app.world(), named);
    assert_eq!(
        redirected.env["JACKDAW_PIE_SCENE"], "levels/meadow.bsn",
        "the run configuration's scene stands"
    );
    assert_eq!(
        scene_answer(app.world_mut(), None, "levels/meadow.bsn"),
        use_file("levels/meadow.bsn"),
        "the game asks for the scene it was told, not the one in front"
    );

    let launch = LaunchScene {
        replaces: "scene.bsn".to_string(),
        scene: PlayScene::Path("levels/meadow.bsn".to_string()),
    };
    assert_eq!(
        scene_answer(app.world_mut(), Some(&launch), "scene.bsn"),
        use_file("levels/meadow.bsn"),
        "a file it replaces with, when that scene is not open"
    );
    open(&mut app, "assets/levels/meadow.bsn");
    open(&mut app, "assets/zone.bsn");
    let (text, dir, stem) = document(scene_answer(app.world_mut(), Some(&launch), "scene.bsn"));
    assert_eq!((dir.as_str(), stem.as_str()), ("levels", "meadow"));
    assert!(text.contains("Meadow"), "{text}");
}

/// A save would refuse this path. Play warns and the game still gets the text.
#[test]
fn an_absolute_asset_path_warns_and_answers_with_the_document() {
    let (mut app, tmp) = editor();
    app.register_type::<NamesAnAsset>();
    std::fs::write(tmp.path().join("assets/rock.bsn"), absolute_field_scene()).expect("a scene");
    open(&mut app, "assets/rock.bsn");

    let (served, ..) = document(scene_answer(app.world_mut(), None, "rock.bsn"));
    assert!(served.contains("/elsewhere/rock.item"), "{served}");
    let notice = app
        .world()
        .resource::<jackdaw::status_bar::StatusNotice>()
        .text()
        .to_string();
    assert!(notice.contains("absolute path"), "{notice}");
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
#[type_path = "game"]
struct NamesAnAsset {
    #[reflect(@jackdaw_scene_types::AssetRef("game::Item"))]
    item: String,
}

fn absolute_field_scene() -> String {
    "#Rock\nbevy_transform::components::transform::Transform\n\
     game::NamesAnAsset { item: \"/elsewhere/rock.item\" }\n"
        .to_string()
}
