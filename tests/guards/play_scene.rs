//! What the editor answers when a running game asks for a scene, spawned by the
//! runtime's own loader where the answer is a document.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::boot_ops::{run_op_clause, run_op_clause_as_user};
use jackdaw::pie::InstanceKey;
use jackdaw::play_scene::{LaunchScene, PlayScene, launch_scene, scene_answer, serve_scene};
use jackdaw::play_sync::{collect_changes_for, collect_material_changes, collect_node_changes};
use jackdaw::scenes::{Scenes, TabKind, TabRefusal};
use jackdaw_api::prelude::*;
use jackdaw_pie_protocol::manifest::RunConfig;
use jackdaw_pie_protocol::{ControlEvent, SceneAnswer};
use jackdaw_runtime::node_sync::{
    apply_node_components, install_node_index, load_asset_files, load_scene_assets,
};
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};
use jackdaw_scene_types::{Brush, SceneNodeId};

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
        SceneAnswer::Document {
            text, dir, stem, ..
        } => (text, dir, stem),
        SceneAnswer::UseFile { path, .. } => panic!("expected a document, got the file {path}"),
    }
}

fn use_file(path: &str) -> SceneAnswer {
    SceneAnswer::UseFile {
        path: path.to_string(),
        sidecars: None,
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
fn an_inactive_tab_answers_with_the_text_it_answered_with_in_front() {
    let (mut app, _tmp) = editor();
    open(&mut app, "assets/levels/meadow.bsn");
    let meadow = entity_named(&mut app, "Meadow");
    move_to(&mut app, meadow, 3.0);
    let (front, ..) = document(scene_answer(app.world_mut(), None, "levels/meadow.bsn"));

    open(&mut app, "assets/zone.bsn");

    let (behind, ..) = document(scene_answer(app.world_mut(), None, "levels/meadow.bsn"));
    assert_eq!(behind, front);
}

#[test]
fn a_scene_not_open_or_refused_uses_its_file() {
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
    assert!(
        matches!(
            scene_answer(app.world_mut(), None, "zone.bsn"),
            SceneAnswer::Document { .. }
        ),
        "a prefab tab answers for a root spawning that prefab"
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

const KEY_CONFIG: &str = "default";

fn instance() -> InstanceKey {
    InstanceKey {
        config: KEY_CONFIG.to_string(),
        instance: 1,
    }
}

/// A game taking materials and brushes, spawning `text` from the asset folder
/// `assets` as if it were in `dir` under it.
fn game_with_assets(assets: &Path, text: String, dir: &str) -> App {
    let mut game = App::new();
    game.add_plugins(MinimalPlugins);
    game.add_plugins(bevy::transform::TransformPlugin);
    game.add_plugins(bevy::asset::AssetPlugin {
        file_path: assets.to_string_lossy().into_owned(),
        ..default()
    });
    game.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    game.init_asset::<StandardMaterial>();
    game.register_asset_reflect::<StandardMaterial>();
    game.init_asset::<Image>();
    game.register_type::<Brush>();
    game.register_type::<Painted>();
    install_node_index(&mut game);
    game.add_plugins(JackdawPlugin);
    let scene = game
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(text, PathBuf::from(dir)));
    game.world_mut().spawn(JackdawSceneRoot(scene));
    game.update();
    game
}

fn take_in_game(game: &mut App, events: Vec<(InstanceKey, ControlEvent)>) {
    for (_, event) in events {
        match event {
            ControlEvent::NodeComponents { node, set, removed } => {
                apply_node_components(game.world_mut(), node, &set, &removed);
            }
            ControlEvent::SceneAssets { text } => load_scene_assets(game.world_mut(), &text),
            ControlEvent::AssetFiles { files } => load_asset_files(game.world_mut(), &files),
            other => panic!("not an edit: {other:?}"),
        }
    }
    game.update();
}

fn asset_messages(events: &[(InstanceKey, ControlEvent)]) -> usize {
    events
        .iter()
        .filter(|(_, event)| matches!(event, ControlEvent::SceneAssets { .. }))
        .count()
}

/// A new cube in the document, with a new material of `roughness` on its first
/// face.
fn cube_with_material(app: &mut App, roughness: f32) -> Entity {
    let before: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .collect();
    run_op_clause_as_user(app.world_mut(), "entity.add.cube")
        .expect("the cube dispatched")
        .assert_finished();
    app.update();
    let cube = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .find(|entity| !before.contains(entity))
        .expect("a cube");
    assign_material(app, cube, roughness);
    cube
}

fn assign_material(app: &mut App, cube: Entity, roughness: f32) -> Handle<StandardMaterial> {
    let material = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            perceptual_roughness: roughness,
            ..default()
        });
    let held = material.clone();
    edit_brush(app, cube, |brush| brush.faces[0].material = held);
    material
}

fn edit_brush(app: &mut App, cube: Entity, edit: impl FnOnce(&mut Brush)) {
    let world = app.world_mut();
    let mut brush = world.get::<Brush>(cube).expect("a brush").clone();
    edit(&mut brush);
    world.entity_mut(cube).insert(brush.clone());
    jackdaw::brush::sync_brush_to_ast(world, cube, &brush);
    app.update();
}

/// The only brush in `world`, as the game holds it.
fn game_brush(game: &mut App) -> Brush {
    game.world_mut()
        .query::<&Brush>()
        .iter(game.world())
        .next()
        .expect("the game holds the brush")
        .clone()
}

fn game_roughness(game: &mut App) -> f32 {
    let brush = game_brush(game);
    game.world()
        .resource::<Assets<StandardMaterial>>()
        .get(&brush.faces[0].material)
        .expect("the handle resolves to an asset")
        .perceptual_roughness
}

fn first_cube(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .next()
        .expect("a cube")
}

fn save(app: &mut App, tmp: &tempfile::TempDir) {
    app.world_mut()
        .operator("scene.save")
        .param("path", zone_file(tmp).to_string_lossy().into_owned())
        .call()
        .expect("scene.save dispatches")
        .assert_finished();
    app.update();
}

fn served_material_name(text: &str) -> String {
    let start = text
        .find("material: \"#")
        .expect("the face names a material")
        + 11;
    let rest = &text[start..];
    rest[..rest.find('"').expect("a closing quote")].to_string()
}

fn served_text(app: &mut App, key: &InstanceKey) -> String {
    let SceneAnswer::Document { text, .. } =
        serve_scene(app.world_mut(), key, None, "zone.bsn").answer
    else {
        panic!("a document");
    };
    text
}

/// A material assigned and saved before Play: the live document holds no root for
/// it, the text gives it a session name the file does not hold, and embeds it.
#[test]
fn a_saved_runtime_material_is_embedded_under_a_session_name() {
    let (mut app, tmp) = editor();
    cube_with_material(&mut app, 0.25);
    save(&mut app, &tmp);
    let saved = std::fs::read_to_string(zone_file(&tmp)).expect("the saved scene");

    let (text, ..) = document(scene_answer(app.world_mut(), None, "zone.bsn"));
    let name = served_material_name(&text);
    assert!(name.starts_with("#Play"), "{name}");
    assert!(!saved.contains(&name), "{name} is in the file:\n{saved}");
    assert!(
        text.contains(&format!("{name}\n")),
        "embedded as a root:\n{text}"
    );
    let mut game = game_with_assets(&tmp.path().join("assets"), text, "");
    assert_eq!(game_roughness(&mut game), 0.25);
}

/// A saved scene with a material, closed and opened again from its file so the
/// document holds a root for it and its face names that root. The brush entity
/// comes with it.
fn reopened_scene_with_a_material(app: &mut App, tmp: &tempfile::TempDir) -> (Entity, String) {
    cube_with_material(app, 0.25);
    save(app, tmp);
    let saved = std::fs::read_to_string(zone_file(tmp)).expect("the saved scene");
    let name = served_material_name(&saved);
    run_op_clause(app.world_mut(), "scene.new")
        .expect("a new scene opens")
        .assert_finished();
    app.update();
    let scenes = app.world().resource::<Scenes>();
    let zone = scenes
        .tabs
        .iter()
        .position(|tab| tab.path.as_deref() == Some(zone_file(tmp).as_path()))
        .expect("the scene's tab");
    jackdaw::scenes::operators::scene_close_system_unprompted(app.world_mut(), zone);
    app.update();
    open(app, "assets/zone.bsn");
    let cube = first_cube(app);
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let (node, brush_path) = (
        ast.ast_for(cube).expect("the cube is in the document"),
        <Brush as TypePath>::type_path(),
    );
    let patch = ast
        .find_patch_by_type_path(node, brush_path)
        .and_then(|patch| ast.get_patch(patch))
        .expect("the document names the brush");
    assert!(
        format!("{patch:?}").contains(&name),
        "the face names {name}: {patch:?}"
    );
    (cube, name)
}

/// Bring the scene back the way a Stop does when the game did something only a
/// respawn undoes.
fn respawn_as_stop_does(app: &mut App) {
    app.world_mut()
        .resource_mut::<jackdaw::pie_projection::PieProjection>()
        .needs_respawn = true;
    jackdaw::pie_projection::revert_preview(app.world_mut());
    app.update();
}

/// A document's own root and each reference to it are renamed together, for an
/// ordinary entity and one Live view fills with game values, whose patch the
/// emit does not derive again.
#[test]
fn a_document_s_own_asset_root_is_renamed_with_every_reference() {
    let (mut app, tmp) = editor();
    let (cube, file_name) = reopened_scene_with_a_material(&mut app, &tmp);
    for overlaid in [false, true] {
        if overlaid {
            app.world_mut()
                .resource_mut::<jackdaw::pie_projection::PieProjection>()
                .overlaid
                .insert(cube);
        }
        let (text, ..) = document(scene_answer(app.world_mut(), None, "zone.bsn"));
        assert!(
            !text.contains(&file_name),
            "overlaid {overlaid}: {file_name} is still in\n{text}"
        );
        let served = served_material_name(&text);
        assert!(served.starts_with("#Play"), "{served}");
        let mut game = game_with_assets(&tmp.path().join("assets"), text, "");
        assert_eq!(game_roughness(&mut game), 0.25, "overlaid {overlaid}");
    }
}

/// A tab behind the one in front holds a snapshot with no entities: its roots and
/// references still go by session names, and a store holding the same spelling for
/// the front tab's asset does not answer for it.
#[test]
fn an_inactive_tab_s_asset_root_is_renamed_with_every_reference() {
    let (mut app, tmp) = editor();
    let (_, file_name) = reopened_scene_with_a_material(&mut app, &tmp);
    open(&mut app, "assets/levels/meadow.bsn");
    cube_with_material(&mut app, 0.75);

    let (text, ..) = document(scene_answer(app.world_mut(), None, "zone.bsn"));
    assert!(!text.contains(&file_name), "{text}");
    let mut game = game_with_assets(&tmp.path().join("assets"), text, "");
    assert_eq!(game_roughness(&mut game), 0.25);
}

/// Another scene spawned from its file holds the name the served scene's file
/// holds. It takes nothing from the served scene, and an edit keeps the material.
#[test]
fn a_scene_loaded_from_its_file_does_not_take_an_edit_s_material() {
    let (mut app, tmp) = editor();
    let (cube, file_name) = reopened_scene_with_a_material(&mut app, &tmp);
    let key = instance();
    collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    let text = served_text(&mut app, &key);
    let mut game = game_with_assets(&tmp.path().join("assets"), text, "");

    let level2 = format!(
        "bevy_ecs::hierarchy::Children [\n    {file_name}\n    \
         bevy_pbr::pbr_material::StandardMaterial {{ perceptual_roughness: 0.9 }}\n    --\n    \
         #Other\n    bevy_transform::components::transform::Transform\n]\n"
    );
    let scene = game
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(level2, PathBuf::new()));
    game.world_mut().spawn(JackdawSceneRoot(scene));
    game.update();

    edit_brush(&mut app, cube, |brush| {
        brush.faces[0].uv_offset = Vec2::new(0.5, 0.25);
    });
    take_in_game(
        &mut game,
        collect_changes_for(app.world_mut(), std::slice::from_ref(&key)),
    );
    assert_eq!(
        game_brush(&mut game).faces[0].uv_offset,
        Vec2::new(0.5, 0.25)
    );
    assert_eq!(game_roughness(&mut game), 0.25, "main's own material");
}

/// A root entity and an embedded asset may share a name. Only the asset renames.
#[test]
fn a_root_entity_named_like_an_embedded_asset_stays() {
    let (mut app, tmp) = editor();
    std::fs::write(
        tmp.path().join("assets/levels/meadow.bsn"),
        "bevy_ecs::hierarchy::Children [\n    #Stone\n    \
         bevy_pbr::pbr_material::StandardMaterial { perceptual_roughness: 0.3 }\n    --\n    \
         #Stone\n    bevy_transform::components::transform::Transform\n]\n",
    )
    .expect("a scene");
    open(&mut app, "assets/levels/meadow.bsn");

    let (text, dir, _) = document(scene_answer(app.world_mut(), None, "levels/meadow.bsn"));
    assert!(text.contains("#Play"), "{text}");
    let mut game = game_with_assets(&tmp.path().join("assets"), text, &dir);
    assert_eq!(game_x(&mut game, "Stone"), 0.0, "the entity spawned");
}

/// Brushes sharing a material: its definition changes, one of them is edited, and
/// the one asset the game holds changes for both.
#[test]
fn a_changed_definition_reaches_every_brush_sharing_the_material() {
    let (mut app, tmp) = editor();
    let first = cube_with_material(&mut app, 0.25);
    let material = app.world().get::<Brush>(first).expect("a brush").faces[0]
        .material
        .clone();
    run_op_clause_as_user(app.world_mut(), "entity.add.cube")
        .expect("the cube dispatched")
        .assert_finished();
    app.update();
    let second = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .find(|&entity| entity != first)
        .expect("the second cube");
    let shared = material.clone();
    edit_brush(&mut app, second, |brush| brush.faces[0].material = shared);

    let key = instance();
    collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    let text = served_text(&mut app, &key);
    let mut game = game_with_assets(&tmp.path().join("assets"), text, "");

    set_roughness(&mut app, &material, 0.8);
    let sends = collect_material_changes(app.world_mut(), std::slice::from_ref(&key));
    assert_eq!(asset_messages(&sends), 1, "{sends:?}");
    take_in_game(&mut game, sends);
    assert_eq!(game_roughnesses(&mut game), vec![0.8, 0.8]);

    assert!(
        collect_material_changes(app.world_mut(), std::slice::from_ref(&key)).is_empty(),
        "an idle frame sends nothing"
    );
    set_roughness(&mut app, &material, 0.8);
    assert!(
        collect_material_changes(app.world_mut(), std::slice::from_ref(&key)).is_empty(),
        "the game holds this definition"
    );
    edit_brush(&mut app, first, |brush| {
        brush.faces[0].uv_offset = Vec2::new(0.5, 0.25);
    });
    let sends = collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    assert_eq!(asset_messages(&sends), 0, "it went on its own: {sends:?}");
}

fn set_roughness(app: &mut App, material: &Handle<StandardMaterial>, roughness: f32) {
    app.world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .get_mut(material)
        .expect("the material")
        .perceptual_roughness = roughness;
    app.update();
}

fn game_roughnesses(game: &mut App) -> Vec<f32> {
    let mut roughness: Vec<f32> = game
        .world_mut()
        .query::<&Brush>()
        .iter(game.world())
        .map(|brush| {
            game.world()
                .resource::<Assets<StandardMaterial>>()
                .get(&brush.faces[0].material)
                .expect("the handle resolves to an asset")
                .perceptual_roughness
        })
        .collect();
    roughness.sort_by(f32::total_cmp);
    roughness
}

fn second_cube(app: &mut App, first: Entity) -> Entity {
    run_op_clause_as_user(app.world_mut(), "entity.add.cube")
        .expect("the cube dispatched")
        .assert_finished();
    app.update();
    app.world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .find(|&entity| entity != first)
        .expect("the second cube")
}

/// Two materials that read alike go by one name. When one changes, the game gets
/// it with no edit, and the other keeps its value.
#[test]
fn a_changed_material_parts_from_another_that_read_the_same() {
    let (mut app, tmp) = editor();
    let first = cube_with_material(&mut app, 0.25);
    let second = second_cube(&mut app, first);
    let material = assign_material(&mut app, second, 0.25);

    let key = instance();
    collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    let text = served_text(&mut app, &key);
    let mut game = game_with_assets(&tmp.path().join("assets"), text, "");
    assert_eq!(game_roughnesses(&mut game), vec![0.25, 0.25]);

    set_roughness(&mut app, &material, 0.8);
    let mut sends = collect_material_changes(app.world_mut(), std::slice::from_ref(&key));
    sends.extend(collect_changes_for(
        app.world_mut(),
        std::slice::from_ref(&key),
    ));
    assert_eq!(asset_messages(&sends), 1, "{sends:?}");
    take_in_game(&mut game, sends);
    assert_eq!(game_roughnesses(&mut game), vec![0.25, 0.8]);
}

/// A tab switch reparses the document, so a shared material comes back under a new
/// id. Something like an undo step can keep the old one alive. The name is still
/// the one asset's, so a change reaches both brushes.
#[test]
fn a_shared_material_stays_one_asset_after_a_tab_switch() {
    let (mut app, tmp) = editor();
    let first = cube_with_material(&mut app, 0.25);
    let old_material = app.world().get::<Brush>(first).expect("a brush").faces[0]
        .material
        .clone();
    let second = second_cube(&mut app, first);
    let shared = old_material.clone();
    edit_brush(&mut app, second, |brush| brush.faces[0].material = shared);

    let key = instance();
    collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    let text = served_text(&mut app, &key);
    let mut game = game_with_assets(&tmp.path().join("assets"), text, "");

    open(&mut app, "assets/levels/meadow.bsn");
    open(&mut app, "assets/zone.bsn");
    collect_changes_for(app.world_mut(), std::slice::from_ref(&key));

    let cube = first_cube(&mut app);
    let material = app.world().get::<Brush>(cube).expect("a brush").faces[0]
        .material
        .clone();
    assert_ne!(material, old_material, "the reparse made a new asset");
    set_roughness(&mut app, &material, 0.8);
    edit_brush(&mut app, cube, |brush| {
        brush.faces[0].uv_offset = Vec2::new(0.5, 0.25);
    });
    take_in_game(
        &mut game,
        collect_changes_for(app.world_mut(), std::slice::from_ref(&key)),
    );
    assert_eq!(game_roughnesses(&mut game), vec![0.8, 0.8]);
}

/// Left out of automatic registration, so only the game below registers it.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, no_auto_register)]
#[type_path = "game"]
struct Painted {
    level: u32,
    swatch: Handle<StandardMaterial>,
}

const PAINTED: &str = "game::Painted";

/// The project reports `game::Painted` by schema alone: the editor has no
/// registration for it, and the game does. One entity holds it, naming `#Stone`.
fn scene_with_a_schema_only_component(app: &mut App, tmp: &tempfile::TempDir) -> Entity {
    let schema: jackdaw_schema::ProjectSchema = serde_json::from_value(serde_json::json!({
        "components": [{
            "type_path": PAINTED,
            "short_name": "Painted",
            "module_path": "game",
            "category": "",
            "description": "",
            "editor_description": "",
            "hidden": false,
            "preview": "",
            "default_constructible": true,
            "fields": [
                { "name": "level", "type_path": "u32" },
                {
                    "name": "swatch",
                    "type_path": <Handle<StandardMaterial> as TypePath>::type_path(),
                },
            ],
            "kind": "Struct",
            "default": { "level": 0, "swatch": "" },
            "variants": [],
            "entity_fields": [],
            "fills_gaps": true,
            "asset": false
        }],
        "resources": [],
        "events": [],
        "functions": [],
        "assets": []
    }))
    .expect("the schema reads");
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    assert!(
        registry.read().get_with_type_path(PAINTED).is_none(),
        "the editor has no registration for it"
    );
    let native = jackdaw::project_types::native_type_paths(&registry.read());
    app.world_mut()
        .resource_mut::<jackdaw::project_types::ProjectTypes>()
        .update(&schema, &native);
    std::fs::write(
        tmp.path().join("assets/stone.bsn"),
        "bevy_ecs::hierarchy::Children [\n    #Stone\n    \
         bevy_pbr::pbr_material::StandardMaterial { perceptual_roughness: 0.3 }\n    --\n    \
         #Rock\n    bevy_transform::components::transform::Transform\n    \
         game::Painted { level: 1, swatch: \"#Stone\" }\n]\n",
    )
    .expect("a scene");
    open(app, "assets/stone.bsn");
    entity_named(app, "Rock")
}

fn set_painted(app: &mut App, entity: Entity, field: &str, value: &str) {
    app.world_mut()
        .operator("component.set")
        .param("entity", entity)
        .param("type_path", PAINTED.to_string())
        .param("field", field.to_string())
        .param("value", value.to_string())
        .call()
        .expect("component.set dispatches")
        .assert_finished();
    app.update();
}

fn game_painted(game: &mut App) -> (u32, f32) {
    let (level, swatch) = {
        let painted = game
            .world_mut()
            .query::<&Painted>()
            .single(game.world())
            .expect("the game holds the component");
        (painted.level, painted.swatch.clone())
    };
    let roughness = game
        .world()
        .resource::<Assets<StandardMaterial>>()
        .get(&swatch)
        .expect("the handle resolves to an asset")
        .perceptual_roughness;
    (level, roughness)
}

/// A field of a type only the schema describes names assets as served text does:
/// an edit keeps the session name, and a file newly assigned goes ahead of it.
#[test]
fn an_edit_of_a_schema_only_component_names_what_the_game_holds() {
    let (mut app, tmp) = editor();
    let rock = scene_with_a_schema_only_component(&mut app, &tmp);
    let key = instance();
    collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    let SceneAnswer::Document { text, .. } =
        serve_scene(app.world_mut(), &key, None, "stone.bsn").answer
    else {
        panic!("a document");
    };
    assert!(!text.contains("swatch: \"#Stone\""), "{text}");
    let mut game = game_with_assets(&tmp.path().join("assets"), text, "");
    assert_eq!(game_painted(&mut game), (1, 0.3));

    set_painted(&mut app, rock, "level", "5");
    let sends = collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    assert_eq!(asset_messages(&sends), 0, "the game has it: {sends:?}");
    take_in_game(&mut game, sends);
    assert_eq!(game_painted(&mut game), (5, 0.3), "the edit landed");

    let stone = app
        .world()
        .resource::<jackdaw_bsn::BsnSceneAssets>()
        .0
        .get("#Stone")
        .expect("the document's asset")
        .clone()
        .typed::<StandardMaterial>();
    set_roughness(&mut app, &stone, 0.9);
    take_in_game(
        &mut game,
        collect_material_changes(app.world_mut(), std::slice::from_ref(&key)),
    );
    assert_eq!(game_painted(&mut game), (5, 0.9), "written in place");

    create_material_file(&mut app, "name=granite roughness=0.6");
    set_painted(&mut app, rock, "swatch", "materials/granite.bsn");
    let sends = collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    assert!(
        matches!(
            sends.as_slice(),
            [
                (_, ControlEvent::AssetFiles { .. }),
                (_, ControlEvent::NodeComponents { .. })
            ]
        ),
        "the definition goes first: {sends:?}"
    );
    take_in_game(&mut game, sends);
    assert_eq!(game_painted(&mut game), (5, 0.6));
}

#[derive(Reflect, Default, Clone)]
#[type_path = "game"]
enum Slot {
    #[default]
    Empty,
    Held(Handle<StandardMaterial>),
    Named {
        swatch: Handle<StandardMaterial>,
    },
}

#[derive(Reflect, Default, Clone)]
#[type_path = "game"]
struct Inner {
    swatch: Handle<StandardMaterial>,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
#[type_path = "game"]
struct Nested {
    first: Slot,
    second: Slot,
    inner: Inner,
}

const NESTED_REFERENCES: usize = 3;

/// An entity whose handles the document names `#Stone` at every depth, then
/// cleared on the entity.
fn cleared_nested_handles(app: &mut App, tmp: &tempfile::TempDir) -> Entity {
    app.register_type::<Nested>();
    std::fs::write(
        tmp.path().join("assets/stone.bsn"),
        "bevy_ecs::hierarchy::Children [\n    #Stone\n    \
         bevy_pbr::pbr_material::StandardMaterial { perceptual_roughness: 0.3 }\n    --\n    \
         #Rock\n    bevy_transform::components::transform::Transform\n    \
         game::Nested { first: game::Slot::Held(\"#Stone\"), \
         second: game::Slot::Named { swatch: \"#Stone\" }, \
         inner: game::Inner { swatch: \"#Stone\" } }\n]\n",
    )
    .expect("a scene");
    open(app, "assets/stone.bsn");
    let rock = entity_named(app, "Rock");
    let world = app.world_mut();
    let held = world.get::<Nested>(rock).expect("the component");
    assert!(matches!(&held.first, Slot::Held(_)), "it loaded");
    *world.get_mut::<Nested>(rock).expect("the component") = Nested::default();
    rock
}

fn saved_references(app: &mut App, tmp: &tempfile::TempDir) -> (usize, String) {
    let text = jackdaw::scene_io::save::emit_bsn_scene_for_file(
        app.world_mut(),
        &tmp.path().join("assets"),
    )
    .expect("nothing is refused");
    (text.matches("\"#Stone\"").count(), text)
}

/// A normal save writes what the entity holds: a handle cleared at any depth stays
/// cleared. An entity Live view overlays keeps the document's references, since a
/// stream carries no handle.
#[test]
fn a_cleared_nested_handle_stays_cleared_unless_the_entity_is_overlaid() {
    let (mut app, tmp) = editor();
    let rock = cleared_nested_handles(&mut app, &tmp);
    let (count, text) = saved_references(&mut app, &tmp);
    assert_eq!(count, 0, "{text}");

    app.world_mut()
        .resource_mut::<jackdaw::pie_projection::PieProjection>()
        .overlaid
        .insert(rock);
    let (count, text) = saved_references(&mut app, &tmp);
    assert_eq!(count, NESTED_REFERENCES, "{text}");
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
#[type_path = "game"]
struct Notes {
    path: PathBuf,
    label: std::borrow::Cow<'static, str>,
    tags: bevy::platform::collections::HashMap<String, String>,
    swatch: Handle<StandardMaterial>,
}

/// Only what takes an asset path is renamed, and a value of a type the editor does
/// not know when it exactly names a root: text equal to a root's name stays.
#[test]
fn text_equal_to_a_root_name_is_not_renamed() {
    let (mut app, tmp) = editor();
    app.register_type::<Notes>();
    std::fs::write(
        tmp.path().join("assets/stone.bsn"),
        "bevy_ecs::hierarchy::Children [\n    #Stone\n    \
         bevy_pbr::pbr_material::StandardMaterial { perceptual_roughness: 0.3 }\n    --\n    \
         #Rock\n    bevy_transform::components::transform::Transform\n    \
         game::Notes { path: \"#Stone\", label: \"#Stone\", \
         tags: map[(\"kind\", \"#Stone\")], swatch: \"#Stone\" }\n    \
         game::Unknown { swatch: \"#Stone\", label: \"#Other\" }\n]\n",
    )
    .expect("a scene");
    open(&mut app, "assets/stone.bsn");
    let (text, ..) = document(scene_answer(app.world_mut(), None, "stone.bsn"));
    assert!(text.contains("path: \"#Stone\""), "a PathBuf:\n{text}");
    assert!(text.contains("label: \"#Stone\""), "a Cow:\n{text}");
    assert!(
        text.contains("\"kind\", \"#Stone\""),
        "a map value:\n{text}"
    );
    assert_eq!(
        text.matches("swatch: \"#Play").count(),
        2,
        "a handle, and a field of an unknown type:\n{text}"
    );
    assert!(text.contains("label: \"#Other\""), "names no root:\n{text}");
}

fn create_material_file(app: &mut App, clause: &str) -> Handle<StandardMaterial> {
    run_op_clause(app.world_mut(), &format!("material.create {clause}"))
        .expect("the material dispatched")
        .assert_finished();
    app.update();
    let world = app.world();
    jackdaw::material_assets::material_of_reference(
        world.get_resource::<jackdaw::asset_index::AssetIndex>(),
        world.resource::<jackdaw::material_assets::MaterialRegistry>(),
        "materials/granite.bsn",
    )
    .expect("the material file is indexed")
}

/// A material file made after the game read its asset folder, with no copy of it
/// in the game: the definition goes ahead of the edit naming its path, so the face
/// holds an asset rather than a handle no loader fills.
#[test]
fn a_material_file_created_during_play_shows_up_in_the_game() {
    let (mut app, tmp) = editor();
    let cube = cube_with_material(&mut app, 0.25);
    let key = instance();
    collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    let text = served_text(&mut app, &key);
    let mut game = game_with_assets(&tmp.path().join("assets"), text, "");

    let file = create_material_file(&mut app, "name=granite roughness=0.6");
    edit_brush(&mut app, cube, |brush| brush.faces[0].material = file);
    let sends = collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    assert!(
        matches!(
            sends.as_slice(),
            [
                (_, ControlEvent::AssetFiles { .. }),
                (_, ControlEvent::NodeComponents { .. })
            ]
        ),
        "the definition goes first: {sends:?}"
    );
    take_in_game(&mut game, sends);
    assert_eq!(game_roughness(&mut game), 0.6);

    edit_brush(&mut app, cube, |brush| {
        brush.faces[0].uv_offset = Vec2::new(0.5, 0.25);
    });
    let sends = collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    assert!(sends.len() == 1, "the game holds it: {sends:?}");
}

/// A document naming a file the game's walk never saw brings its definition.
#[test]
fn a_served_document_brings_the_material_files_it_names() {
    let (mut app, tmp) = editor();
    let cube = cube_with_material(&mut app, 0.25);
    let mut game = game_with_assets(&tmp.path().join("assets"), ZONE.to_string(), "");

    let file = create_material_file(&mut app, "name=granite roughness=0.6");
    edit_brush(&mut app, cube, |brush| brush.faces[0].material = file);
    let key = instance();
    let served = serve_scene(app.world_mut(), &key, None, "zone.bsn");
    let Some(ControlEvent::AssetFiles { files }) = served.files else {
        panic!("the file goes ahead of the answer: {:?}", served.files);
    };
    load_asset_files(game.world_mut(), &files);
    let SceneAnswer::Document { text, .. } = served.answer else {
        panic!("a document");
    };
    let scene = game
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(text, PathBuf::new()));
    game.world_mut().spawn(JackdawSceneRoot(scene));
    game.update();
    assert_eq!(game_roughness(&mut game), 0.6);

    let again = serve_scene(app.world_mut(), &key, None, "zone.bsn");
    assert!(again.files.is_none(), "sent once");
}

/// A value set on a material file reaches every mesh using it, with no component
/// edited, and so does its undo.
#[test]
fn a_set_on_a_material_file_reaches_the_game_and_undo_reverts_it() {
    let (mut app, tmp) = editor();
    let cube = cube_with_material(&mut app, 0.25);
    let file = create_material_file(&mut app, "name=granite roughness=0.6");
    edit_brush(&mut app, cube, |brush| brush.faces[0].material = file);
    let key = instance();
    collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    let text = served_text(&mut app, &key);
    let mut game = game_with_assets(&tmp.path().join("assets"), text, "");
    assert_eq!(game_roughness(&mut game), 0.6);
    let held = game_brush(&mut game).faces[0].material.clone();
    collect_material_changes(app.world_mut(), std::slice::from_ref(&key));

    run_op_clause(
        app.world_mut(),
        "material.set material=materials/granite.bsn field=roughness value=0.9",
    )
    .expect("the set dispatched")
    .assert_finished();
    app.update();
    take_in_game(
        &mut game,
        collect_material_changes(app.world_mut(), std::slice::from_ref(&key)),
    );
    assert_eq!(game_roughness(&mut game), 0.9);
    assert_eq!(game_brush(&mut game).faces[0].material, held, "in place");

    app.world_mut().resource_scope(
        |world, mut history: Mut<jackdaw::commands::CommandHistory>| {
            history.undo(world);
        },
    );
    app.update();
    take_in_game(
        &mut game,
        collect_material_changes(app.world_mut(), std::slice::from_ref(&key)),
    );
    assert_eq!(game_roughness(&mut game), 0.6);
}

/// Saving during Play renumbers a file's names. An edit and a respawn keep
/// spelling the one name the game holds.
#[test]
fn a_save_during_play_does_not_split_the_served_name_from_the_edit() {
    let (mut app, tmp) = editor();
    let cube = cube_with_material(&mut app, 0.25);
    let key = instance();
    collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    let text = served_text(&mut app, &key);
    let name = served_material_name(&text);
    assert!(name.starts_with("#Play"), "{name}");

    save(&mut app, &tmp);
    let (after, ..) = document(scene_answer(app.world_mut(), None, "zone.bsn"));
    assert_eq!(served_material_name(&after), name);

    edit_brush(&mut app, cube, |brush| {
        brush.faces[0].uv_offset = Vec2::new(0.5, 0.25);
    });
    let sends = collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    assert_eq!(asset_messages(&sends), 0, "the game has it: {sends:?}");
    let mut game = game_with_assets(&tmp.path().join("assets"), text, "");
    take_in_game(&mut game, sends);
    assert_eq!(
        game_brush(&mut game).faces[0].uv_offset,
        Vec2::new(0.5, 0.25)
    );
    assert_eq!(game_roughness(&mut game), 0.25);
}

/// A view toggle re-emits and reparses the document, so each asset comes back
/// with a new id and its handle fields with a new per-document name. The running
/// game only knows the first, and still takes the edit, without a new copy.
#[test]
fn an_edit_after_the_document_respawns_reaches_the_game() {
    let (mut app, tmp) = editor();
    cube_with_material(&mut app, 0.25);
    let key = instance();
    collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    let text = served_text(&mut app, &key);
    let name = served_material_name(&text);
    let mut game = game_with_assets(&tmp.path().join("assets"), text, "");
    assert_eq!(game_roughness(&mut game), 0.25);
    let spawned = game_brush(&mut game).faces[0].material.clone();

    for offset in [0.5, 0.75] {
        let before = first_cube(&mut app);
        respawn_as_stop_does(&mut app);
        let cube = first_cube(&mut app);
        assert_ne!(cube, before, "the document respawned");
        edit_brush(&mut app, cube, |brush| {
            brush.faces[0].uv_offset = Vec2::new(offset, 0.0);
        });
        let sends = collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
        assert_eq!(asset_messages(&sends), 0, "no copy per reparse: {sends:?}");
        take_in_game(&mut game, sends);
        let brush = game_brush(&mut game);
        assert_eq!(brush.faces[0].uv_offset, Vec2::new(offset, 0.0));
        assert_eq!(brush.faces[0].material, spawned, "the same material");
    }
    let (again, ..) = document(scene_answer(app.world_mut(), None, "zone.bsn"));
    assert_eq!(served_material_name(&again), name);
}

/// A material new to the game arrives ahead of the edit naming it.
#[test]
fn a_material_assigned_during_play_shows_up_in_the_game() {
    let (mut app, tmp) = editor();
    let cube = cube_with_material(&mut app, 0.25);
    let key = instance();
    collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    let text = served_text(&mut app, &key);
    let mut game = game_with_assets(&tmp.path().join("assets"), text, "");

    assign_material(&mut app, cube, 0.75);
    let sends = collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    assert!(
        matches!(
            sends.as_slice(),
            [
                (_, ControlEvent::SceneAssets { .. }),
                (_, ControlEvent::NodeComponents { .. })
            ]
        ),
        "the definition goes first: {sends:?}"
    );
    take_in_game(&mut game, sends);
    assert_eq!(game_roughness(&mut game), 0.75);

    edit_brush(&mut app, cube, |brush| {
        brush.faces[0].uv_offset = Vec2::new(0.5, 0.25);
    });
    let sends = collect_changes_for(app.world_mut(), std::slice::from_ref(&key));
    assert_eq!(asset_messages(&sends), 0, "sent once: {sends:?}");

    let other = InstanceKey {
        config: KEY_CONFIG.to_string(),
        instance: 2,
    };
    edit_brush(&mut app, cube, |brush| {
        brush.faces[0].uv_offset = Vec2::new(0.75, 0.25);
    });
    let sends = collect_changes_for(app.world_mut(), &[key, other.clone()]);
    let to_other: Vec<_> = sends.iter().filter(|(to, _)| *to == other).collect();
    assert!(
        matches!(to_other[0].1, ControlEvent::SceneAssets { .. }),
        "each game is tracked on its own: {sends:?}"
    );
    assert_eq!(asset_messages(&sends), 1);
}

/// An edit made long before a later scene spawns comes from the document that
/// scene is served, whatever time passed in the game.
#[test]
fn an_edit_long_before_a_later_scene_is_in_the_document_it_is_served() {
    let (mut app, tmp) = editor();
    let zone = entity_named(&mut app, "Zone");
    collect_node_changes(app.world_mut());
    let mut game = game_with_assets(&tmp.path().join("assets"), MEADOW.to_string(), "levels");
    game.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs(1),
    ));

    move_to(&mut app, zone, 8.0);
    for event in collect_node_changes(app.world_mut()) {
        let ControlEvent::NodeComponents { node, set, removed } = event else {
            panic!("a node edit");
        };
        apply_node_components(game.world_mut(), node, &set, &removed);
    }
    for _ in 0..32 {
        game.update();
    }

    let (text, ..) = document(scene_answer(app.world_mut(), None, "zone.bsn"));
    let scene = game
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(text, PathBuf::new()));
    game.world_mut().spawn(JackdawSceneRoot(scene));
    game.update();
    assert_eq!(game_x(&mut game, "Zone"), 8.0);
}

/// A scene in a folder with a prefab instance whose source is spelled beside the
/// scene: the served text still reaches the prefab, so its content spawns.
#[test]
fn a_served_scene_in_a_folder_spawns_its_prefab() {
    let (mut app, tmp) = editor();
    std::fs::create_dir_all(tmp.path().join("assets/levels/parts")).expect("a folder");
    std::fs::write(
        tmp.path().join("assets/levels/parts/rock.bsn"),
        "#Rock\nbevy_transform::components::transform::Transform\n\
         bevy_ecs::hierarchy::Children [\n    #Pebble\n    \
         bevy_transform::components::transform::Transform\n]\n",
    )
    .expect("a prefab");
    std::fs::write(
        tmp.path().join("assets/levels/meadow.bsn"),
        "#Meadow\nbevy_transform::components::transform::Transform\n\
         bevy_ecs::hierarchy::Children [\n    \
         #Boulder\n    jackdaw::prefab::components::IsA { source: \"parts/rock.bsn\", deleted: [] }\n\
         ]\n",
    )
    .expect("a scene");
    open(&mut app, "assets/levels/meadow.bsn");

    let (text, dir, _) = document(scene_answer(app.world_mut(), None, "levels/meadow.bsn"));
    assert_eq!(dir, "levels");
    assert!(text.contains("rock.bsn"), "{text}");
    let mut game = game_with_assets(&tmp.path().join("assets"), text, &dir);
    let pebble = game
        .world_mut()
        .query::<&Name>()
        .iter(game.world())
        .any(|name| name.as_str() == "Pebble");
    assert!(pebble, "the prefab's content spawned");
}

/// A saved cube reopened, its node id, and a stream of the game's own value for
/// its `Transform` and `Visibility`.
fn streamed_cube(
    app: &mut App,
    tmp: &tempfile::TempDir,
) -> (Entity, jackdaw_pie_protocol::RemoteEntity) {
    let (cube, _) = reopened_scene_with_a_material(app, tmp);
    let node = app.world().get::<SceneNodeId>(cube).expect("a node id").0;
    let entity = jackdaw_pie_protocol::RemoteEntity {
        entity: 77,
        scene_node_id: Some(node),
        components: [
            (
                "bevy_transform::components::transform::Transform".to_string(),
                serde_json::json!({
                    "translation": [9.0, 9.0, 9.0],
                    "rotation": [0.0, 1.0, 0.0, 0.0],
                    "scale": [3.0, 3.0, 3.0],
                }),
            ),
            (
                "bevy_camera::visibility::Visibility".to_string(),
                serde_json::json!("Hidden"),
            ),
        ]
        .into_iter()
        .collect(),
    };
    (cube, entity)
}

fn project(app: &mut App, entity: jackdaw_pie_protocol::RemoteEntity) {
    jackdaw::pie_projection::project_event(
        app.world_mut(),
        jackdaw_pie_protocol::StateEvent::EntitySpawned { entity },
    );
    app.update();
}

/// Stop puts back everything the game changed, including what the document does
/// not name, and leaves a scene the game only changed the values of unspawned.
#[test]
fn stop_restores_what_the_game_changed_in_place() {
    let (mut app, tmp) = editor();
    let (cube, streamed) = streamed_cube(&mut app, &tmp);
    let transform = *app.world().get::<Transform>(cube).expect("a transform");
    let visibility = *app.world().get::<Visibility>(cube).expect("a visibility");
    let material = app.world().get::<Brush>(cube).expect("a brush").faces[0]
        .material
        .clone();

    project(&mut app, streamed);
    let shown = *app.world().get::<Transform>(cube).expect("a transform");
    assert_eq!(shown.scale, Vec3::splat(3.0), "the stream reached the cube");
    assert_eq!(
        app.world().get::<Visibility>(cube),
        Some(&Visibility::Hidden)
    );

    jackdaw::pie_projection::revert_preview(app.world_mut());
    app.update();

    assert!(app.world().get_entity(cube).is_ok(), "no respawn");
    assert_eq!(app.world().get::<Transform>(cube), Some(&transform));
    assert_eq!(app.world().get::<Visibility>(cube), Some(&visibility));
    let brush = app.world().get::<Brush>(cube).expect("a brush");
    assert_eq!(brush.faces[0].material, material);
}

/// A brush field and a transform's x were edited after the game first changed
/// them: Stop returns to the document for those, not to what the entity held when
/// Play started. The transform's other fields are not asserted: the operator
/// writes whole values, a separate known issue.
#[test]
fn stop_keeps_a_brush_edit_and_a_moved_x_made_during_play() {
    let (mut app, tmp) = editor();
    let (cube, streamed) = streamed_cube(&mut app, &tmp);
    let brush_value = streamed_brush_value(&mut app, cube, |brush| {
        brush.faces[0].uv_offset = Vec2::new(9.0, 9.0);
    });
    let mut streamed = streamed;
    streamed
        .components
        .insert(<Brush as TypePath>::type_path().to_string(), brush_value);
    project(&mut app, streamed);

    edit_brush(&mut app, cube, |brush| {
        brush.faces[0].uv_offset = Vec2::new(0.5, 0.0);
    });
    app.world_mut()
        .operator("entity.set_transform")
        .param("entity", cube)
        .param("x", 2.0)
        .call()
        .expect("entity.set_transform dispatches")
        .assert_finished();
    app.update();

    jackdaw::pie_projection::revert_preview(app.world_mut());
    app.update();

    let cube = first_cube(&mut app);
    let brush = app.world().get::<Brush>(cube).expect("a brush");
    assert_eq!(brush.faces[0].uv_offset, Vec2::new(0.5, 0.0));
    let transform = app.world().get::<Transform>(cube).expect("a transform");
    assert_eq!(transform.translation.x, 2.0, "the edit stays");
    let hidden = app.world().get::<Visibility>(cube);
    assert_ne!(
        hidden,
        Some(&Visibility::Hidden),
        "what the edit did not write goes back"
    );
}

/// A field the user took out of the document after the game changed the component:
/// what the entity held at Play still names it, and the document no longer does.
#[test]
fn stop_drops_a_field_reset_out_of_the_document_during_play() {
    const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
    let (mut app, tmp) = editor();
    let (cube, streamed) = streamed_cube(&mut app, &tmp);
    move_to(&mut app, cube, 4.0);
    project(&mut app, streamed);

    let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.ast_for(cube).expect("the cube is in the document");
    let patch = ast
        .find_patch_by_type_path(node, TRANSFORM)
        .expect("the document names the transform");
    ast.set_patch(patch, jackdaw_bsn::BsnPatch::Type(TRANSFORM.to_string()));

    jackdaw::pie_projection::revert_preview(app.world_mut());
    app.update();

    let cube = first_cube(&mut app);
    let transform = app.world().get::<Transform>(cube).expect("a transform");
    assert_eq!(transform.translation.x, 0.0, "what the document says");
}

/// A stream's value for the cube's brush, with `change` made to it. The game's
/// handles do not travel, so the value holds defaults for them.
fn streamed_brush_value(
    app: &mut App,
    cube: Entity,
    change: impl FnOnce(&mut Brush),
) -> serde_json::Value {
    let mut brush = app.world().get::<Brush>(cube).expect("a brush").clone();
    change(&mut brush);
    let world = app.world_mut();
    let probe = world.spawn(brush).id();
    let registry = world.resource::<AppTypeRegistry>().clone();
    let snapshot = jackdaw_pie_protocol::build_snapshot(world, &registry.read(), &[probe]);
    world.entity_mut(probe).despawn();
    snapshot[0].components[<Brush as TypePath>::type_path()].clone()
}

/// A saved cube with an authored field in its brush, and a stream changing that
/// field to the game's own.
fn cube_with_streamed_brush(
    app: &mut App,
    tmp: &tempfile::TempDir,
) -> (Entity, jackdaw_pie_protocol::RemoteEntity, String) {
    let (cube, mut streamed) = streamed_cube(app, tmp);
    edit_brush(app, cube, |brush| {
        brush.faces[0].uv_offset = Vec2::new(0.25, 0.75);
    });
    save(app, tmp);
    let authored = std::fs::read_to_string(zone_file(tmp)).expect("the saved scene");
    let value = streamed_brush_value(app, cube, |brush| {
        brush.faces[0].uv_offset = Vec2::new(0.5, 0.125);
    });
    streamed
        .components
        .insert(<Brush as TypePath>::type_path().to_string(), value);
    (cube, streamed, authored)
}

/// The game's value of a handle-bearing component never reaches the document or
/// the file, whichever way Stop brings the scene back.
#[track_caller]
fn assert_holds_the_authored_brush(app: &mut App, tmp: &tempfile::TempDir, authored: &str) {
    let cube = first_cube(app);
    let brush = app.world().get::<Brush>(cube).expect("a brush");
    assert_eq!(brush.faces[0].uv_offset, Vec2::new(0.25, 0.75));
    let (served, ..) = document(scene_answer(app.world_mut(), None, "zone.bsn"));
    assert!(!served.contains("0.125"), "the document: {served}");
    save(app, tmp);
    let saved = std::fs::read_to_string(zone_file(tmp)).expect("the saved scene");
    assert!(!saved.contains("0.125"), "the file: {saved}");
    assert_eq!(saved, authored, "the file is as it was");
}

#[test]
fn a_stop_that_respawns_writes_no_game_value_to_the_document() {
    let (mut app, tmp) = editor();
    let (cube, mut streamed, authored) = cube_with_streamed_brush(&mut app, &tmp);
    streamed.components.insert(
        "bevy_light::point_light::PointLight".to_string(),
        serde_json::json!({}),
    );
    project(&mut app, streamed);
    let shown = app.world().get::<Brush>(cube).expect("a brush");
    assert_eq!(shown.faces[0].uv_offset, Vec2::new(0.5, 0.125));

    jackdaw::pie_projection::revert_preview(app.world_mut());
    app.update();

    assert!(app.world().get_entity(cube).is_err(), "a respawn");
    assert_holds_the_authored_brush(&mut app, &tmp, &authored);
}

#[test]
fn a_stop_that_restores_in_place_writes_no_game_value_to_the_document() {
    let (mut app, tmp) = editor();
    let (cube, streamed, authored) = cube_with_streamed_brush(&mut app, &tmp);
    project(&mut app, streamed);
    let shown = app.world().get::<Brush>(cube).expect("a brush");
    assert_eq!(shown.faces[0].uv_offset, Vec2::new(0.5, 0.125));

    jackdaw::pie_projection::revert_preview(app.world_mut());
    app.update();

    assert!(app.world().get_entity(cube).is_ok(), "no respawn");
    assert_holds_the_authored_brush(&mut app, &tmp, &authored);
}

/// A component the game added brought whatever it requires, which only a respawn
/// takes back off.
#[test]
fn stop_respawns_when_the_game_added_a_component() {
    let (mut app, tmp) = editor();
    let (cube, mut streamed) = streamed_cube(&mut app, &tmp);
    streamed.components.clear();
    streamed.components.insert(
        "bevy_light::point_light::PointLight".to_string(),
        serde_json::json!({}),
    );
    let before = app.world().entity(cube).archetype().components().len();
    project(&mut app, streamed);
    assert!(app.world().entity(cube).archetype().components().len() > before);

    jackdaw::pie_projection::revert_preview(app.world_mut());
    app.update();

    assert!(app.world().get_entity(cube).is_err(), "a respawn");
    let cube = first_cube(&mut app);
    assert!(app.world().get::<PointLight>(cube).is_none());
}

/// The game parents a scene's roots under its own container, so this happens on
/// every Play. The root comes back without a respawn.
#[test]
fn stop_makes_a_root_the_game_parented_a_root_again() {
    let (mut app, tmp) = editor();
    let (cube, mut streamed) = streamed_cube(&mut app, &tmp);
    project(&mut app, container());
    streamed.components.insert(
        "bevy_ecs::hierarchy::ChildOf".to_string(),
        serde_json::json!(5),
    );
    project(&mut app, streamed);
    assert!(
        app.world().get::<ChildOf>(cube).is_some(),
        "the game moved it"
    );

    jackdaw::pie_projection::revert_preview(app.world_mut());
    app.update();

    assert!(app.world().get_entity(cube).is_ok(), "no respawn");
    assert!(app.world().get::<ChildOf>(cube).is_none());
}

/// A rename made during Play, on an entity the game also renamed, survives Stop.
#[test]
fn stop_keeps_a_rename_made_during_play() {
    let (mut app, tmp) = editor();
    let (cube, mut streamed) = streamed_cube(&mut app, &tmp);
    streamed.components.insert(
        <Name as TypePath>::type_path().to_string(),
        serde_json::json!("GameName"),
    );
    project(&mut app, streamed);
    assert_eq!(
        app.world().get::<Name>(cube).map(Name::as_str),
        Some("GameName")
    );

    app.world_mut()
        .operator("entity.rename")
        .param("entity", cube)
        .param("name", "UserName".to_string())
        .call()
        .expect("entity.rename dispatches")
        .assert_finished();
    app.update();

    jackdaw::pie_projection::revert_preview(app.world_mut());
    app.update();

    let cube = first_cube(&mut app);
    assert_eq!(
        app.world().get::<Name>(cube).map(Name::as_str),
        Some("UserName")
    );
}

/// A root the user dragged under another entity during Play stays there on Stop,
/// though the game had parented it under its container too.
#[test]
fn stop_keeps_a_root_the_user_reparented_during_play() {
    let (mut app, tmp) = editor();
    let (cube, mut streamed) = streamed_cube(&mut app, &tmp);
    streamed.components.clear();
    project(&mut app, container());
    streamed.components.insert(
        "bevy_ecs::hierarchy::ChildOf".to_string(),
        serde_json::json!(5),
    );
    project(&mut app, streamed);

    run_op_clause(app.world_mut(), "entity.add.group name=Folder")
        .expect("a folder is added")
        .assert_finished();
    app.update();
    let folder = entity_named(&mut app, "Folder");
    app.world_mut()
        .operator("entity.reparent")
        .param("child", cube)
        .param("parent", folder)
        .call()
        .expect("entity.reparent dispatches")
        .assert_finished();
    app.update();

    jackdaw::pie_projection::revert_preview(app.world_mut());
    app.update();

    let cube = first_cube(&mut app);
    let folder = entity_named(&mut app, "Folder");
    let parent = app.world().get::<ChildOf>(cube).map(|held| held.0);
    assert_eq!(parent, Some(folder), "the cube stays in the folder");
}

/// Putting a child back under its parent could change its place among its
/// siblings, so a child the game moved brings the scene back by a respawn.
#[test]
fn stop_respawns_when_the_game_moved_a_child() {
    let (mut app, tmp) = editor();
    let (cube, mut streamed) = streamed_cube(&mut app, &tmp);
    let transform = *app.world().get::<Transform>(cube).expect("a transform");
    let parent = app.world_mut().spawn_empty().id();
    app.world_mut().entity_mut(cube).insert(ChildOf(parent));
    project(&mut app, container());
    streamed.components.insert(
        "bevy_ecs::hierarchy::ChildOf".to_string(),
        serde_json::json!(5),
    );
    project(&mut app, streamed);

    jackdaw::pie_projection::revert_preview(app.world_mut());
    app.update();

    assert!(app.world().get_entity(cube).is_err(), "a respawn");
    let cube = first_cube(&mut app);
    assert_eq!(app.world().get::<Transform>(cube), Some(&transform));
    assert!(app.world().get::<ChildOf>(cube).is_none());
}

/// An entity of the game's own that the editor shows as an ephemeral.
fn container() -> jackdaw_pie_protocol::RemoteEntity {
    jackdaw_pie_protocol::RemoteEntity {
        entity: 5,
        scene_node_id: None,
        components: Default::default(),
    }
}

/// An edit made live in the panels is held in the log and the game, not the
/// document, so Stop brings the scene back by a respawn.
#[test]
fn stop_respawns_after_a_live_edit() {
    let (mut app, tmp) = editor();
    let (cube, streamed) = streamed_cube(&mut app, &tmp);
    project(&mut app, streamed);
    app.world_mut()
        .resource_mut::<jackdaw::live_edits::LiveEditLog>()
        .entries
        .push((
            jackdaw::live_edits::LiveEditKey {
                bits: 77,
                type_path: "bevy_transform::components::transform::Transform".to_string(),
                field_path: "scale".to_string(),
            },
            jackdaw::live_edits::LiveEditEntry {
                node_id: None,
                baseline: None,
                live_value: serde_json::json!([3.0, 3.0, 3.0]),
                label: "cube / Transform.scale".to_string(),
            },
        ));

    jackdaw::pie_projection::revert_preview(app.world_mut());
    app.update();

    assert!(app.world().get_entity(cube).is_err(), "a respawn");
    let cube = first_cube(&mut app);
    assert_eq!(
        app.world()
            .get::<Transform>(cube)
            .expect("a transform")
            .scale,
        Vec3::ONE
    );
}

/// A scene with an asset root of every kind: `#Stone` loaded from the file into the
/// stores, `#Waiting` in the document with no store holding it, and a material made
/// at runtime that the emit generates a root for.
fn scene_with_every_kind_of_asset(app: &mut App, tmp: &tempfile::TempDir) {
    std::fs::write(
        zone_file(tmp),
        "bevy_ecs::hierarchy::Children [\n    #Stone\n    \
         bevy_pbr::pbr_material::StandardMaterial { perceptual_roughness: 0.3 }\n    --\n    \
         #Rock\n    bevy_transform::components::transform::Transform { translation: \
         glam::Vec3 { x: 1.5, y: 0.1, z: -2.0 } }\n    bevy_ecs::hierarchy::Children [\n        \
         #Pebble\n        bevy_transform::components::transform::Transform\n    ]\n]\n",
    )
    .expect("a scene");
    run_op_clause(app.world_mut(), "scene.new")
        .expect("a new scene opens")
        .assert_finished();
    app.update();
    let zone = app
        .world()
        .resource::<Scenes>()
        .tabs
        .iter()
        .position(|tab| tab.path.as_deref() == Some(zone_file(tmp).as_path()))
        .expect("the scene's tab");
    jackdaw::scenes::operators::scene_close_system_unprompted(app.world_mut(), zone);
    app.update();
    open(app, "assets/zone.bsn");
    cube_with_material(app, 0.25);
    let world = app.world_mut();
    let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
    let waiting = ast.create_entity_node(vec![
        jackdaw_bsn::BsnPatch::Name("Waiting".to_string()),
        jackdaw_bsn::BsnPatch::Struct(jackdaw_bsn::BsnStructData {
            type_path: "bevy_pbr::pbr_material::StandardMaterial".to_string(),
            fields: jackdaw_bsn::BsnStructFields(vec![jackdaw_bsn::BsnField {
                name: "perceptual_roughness".into(),
                value: jackdaw_bsn::BsnValue::Float(0.75),
            }]),
        }),
    ]);
    ast.add_to_roots(waiting);
}

/// `text` with each node id, which is minted at random, written as `N`.
fn without_node_ids(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("SceneNodeId(") {
        let (head, tail) = rest.split_at(at + "SceneNodeId(".len());
        out.push_str(head);
        out.push('N');
        rest = tail.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    out.push_str(rest);
    out
}

/// What a game gets, byte for byte, for the scene in front and for the same scene
/// behind another.
#[test]
fn served_text_matches_the_recorded_document() {
    let (mut app, tmp) = editor();
    scene_with_every_kind_of_asset(&mut app, &tmp);
    let (front, ..) = document(scene_answer(app.world_mut(), None, "zone.bsn"));
    open(&mut app, "assets/levels/meadow.bsn");
    let (behind, ..) = document(scene_answer(app.world_mut(), None, "zone.bsn"));
    let served = format!("{front}=====\n{behind}");
    let served = without_node_ids(&served);
    if std::env::var_os("JACKDAW_PRINT_SERVED").is_some() {
        println!("<<<{served}>>>");
    }
    assert_eq!(served, include_str!("play_scene_served.txt"));
}

/// Serving reads the document's text back once, whatever assets it embeds.
#[test]
fn serving_a_scene_reads_its_text_back_once() {
    let (mut app, tmp) = editor();
    scene_with_every_kind_of_asset(&mut app, &tmp);
    let before = jackdaw_bsn::parses_on_this_thread();
    serve_scene(app.world_mut(), &instance(), None, "zone.bsn");
    assert_eq!(jackdaw_bsn::parses_on_this_thread() - before, 1);
    let before = jackdaw_bsn::parses_on_this_thread();
    serve_scene(app.world_mut(), &instance(), None, "zone.bsn");
    assert_eq!(
        jackdaw_bsn::parses_on_this_thread() - before,
        1,
        "and so does a respawn"
    );
}

fn add_unreadable_root(app: &mut App) {
    let mut ast = app.world_mut().resource_mut::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::Struct(
        jackdaw_bsn::BsnStructData {
            type_path: String::new(),
            fields: jackdaw_bsn::BsnStructFields(vec![jackdaw_bsn::BsnField {
                name: "0".into(),
                value: jackdaw_bsn::BsnValue::Int(1),
            }]),
        },
    )]);
    ast.add_to_roots(node);
}

/// Text that would not read back never goes to a game. It loads the file, and the
/// user is told why the edits are missing.
#[test]
fn a_document_that_does_not_read_back_is_refused_with_a_message() {
    let (mut app, _tmp) = editor();
    add_unreadable_root(&mut app);
    let answer = scene_answer(app.world_mut(), None, "zone.bsn");
    assert_eq!(answer, use_file("zone.bsn"));
    let notice = app.world().resource::<jackdaw::status_bar::StatusNotice>();
    assert!(notice.is_active(), "the user is told");
    assert_eq!(
        notice.text(),
        "zone.bsn: unsaved edits not sent, the game reads the saved file"
    );
}

/// A refused document leaves nothing counted as given: the game never got its text,
/// so an edit of a material it embedded is not sent.
#[test]
fn a_refused_document_counts_nothing_as_given() {
    let (mut app, _tmp) = editor();
    let cube = cube_with_material(&mut app, 0.25);
    add_unreadable_root(&mut app);
    let key = instance();
    let served = serve_scene(app.world_mut(), &key, None, "zone.bsn");
    assert!(matches!(served.answer, SceneAnswer::UseFile { .. }));
    collect_material_changes(app.world_mut(), std::slice::from_ref(&key));

    let material = app.world().get::<Brush>(cube).expect("a brush").faces[0]
        .material
        .clone();
    app.world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .get_mut(&material)
        .expect("the material")
        .perceptual_roughness = 0.9;
    app.update();
    let events = collect_material_changes(app.world_mut(), std::slice::from_ref(&key));
    assert_eq!(asset_messages(&events), 0, "{events:?}");
}

/// A file counted as given at the refused answer still goes to the game: another
/// answer on its way may be relying on it.
#[test]
fn a_refused_document_still_sends_the_files_it_names() {
    let (mut app, _tmp) = editor();
    let cube = cube_with_material(&mut app, 0.25);
    let file = create_material_file(&mut app, "name=granite roughness=0.6");
    edit_brush(&mut app, cube, |brush| brush.faces[0].material = file);
    add_unreadable_root(&mut app);
    let served = serve_scene(app.world_mut(), &instance(), None, "zone.bsn");
    assert!(matches!(served.answer, SceneAnswer::UseFile { .. }));
    let Some(ControlEvent::AssetFiles { files }) = served.files else {
        panic!("the file still goes: {:?}", served.files);
    };
    assert_eq!(files[0].0, "materials/granite.bsn");
}

/// A name the game got with an earlier answer is still held after a refusal, so an
/// edit of the material keeps reaching the root that spawned from that answer.
#[test]
fn a_refused_document_keeps_a_name_the_game_held_earlier() {
    let (mut app, _tmp) = editor();
    let cube = cube_with_material(&mut app, 0.25);
    let key = instance();
    serve_scene(app.world_mut(), &key, None, "zone.bsn");
    collect_material_changes(app.world_mut(), std::slice::from_ref(&key));
    add_unreadable_root(&mut app);
    let served = serve_scene(app.world_mut(), &key, None, "zone.bsn");
    assert!(matches!(served.answer, SceneAnswer::UseFile { .. }));
    collect_material_changes(app.world_mut(), std::slice::from_ref(&key));

    let material = app.world().get::<Brush>(cube).expect("a brush").faces[0]
        .material
        .clone();
    app.world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .get_mut(&material)
        .expect("the material")
        .perceptual_roughness = 0.9;
    app.update();
    let events = collect_material_changes(app.world_mut(), std::slice::from_ref(&key));
    assert_eq!(asset_messages(&events), 1, "{events:?}");
}
