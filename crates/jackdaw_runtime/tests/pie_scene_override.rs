use std::path::{Path, PathBuf};
use std::time::Duration;

use bevy::asset::AssetPath;
use bevy::prelude::*;
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot, PieSceneOverride};

fn named_scene(name: &str) -> String {
    format!("#{name}\nbevy_transform::components::transform::Transform\n")
}

fn app_over(dir: &Path, play_scene: Option<PieSceneOverride>) -> App {
    let files = [
        ("scene.bsn", "Default"),
        ("hud.bsn", "Hud"),
        ("levels/meadow.bsn", "Meadow"),
        ("levels/boss#2.bsn", "Boss"),
    ];
    for (file, name) in files {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("folder");
        std::fs::write(path, named_scene(name)).expect("scene written");
    }

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin {
        file_path: dir.to_string_lossy().into_owned(),
        ..Default::default()
    });
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    if let Some(play_scene) = play_scene {
        app.insert_resource(play_scene);
    }
    app.add_plugins(JackdawPlugin);
    app
}

fn meadow_over_main() -> Option<PieSceneOverride> {
    Some(PieSceneOverride::new("levels/meadow.bsn", "scene.bsn"))
}

fn spawned_names(app: &mut App, root: Entity) -> Vec<String> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        app.update();
        let names: Vec<String> = app
            .world_mut()
            .query::<(&Name, &ChildOf)>()
            .iter(app.world())
            .filter(|(_, parent)| parent.parent() == root)
            .map(|(name, _)| name.to_string())
            .collect();
        if !names.is_empty() || std::time::Instant::now() > deadline {
            return names;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn spawn_root(app: &mut App, path: &'static str) -> Entity {
    let handle: Handle<JackdawScene> = app.world().resource::<AssetServer>().load(path);
    app.world_mut().spawn(JackdawSceneRoot(handle)).id()
}

fn asked_path(app: &App, root: Entity) -> Option<AssetPath<'static>> {
    app.world()
        .get::<JackdawSceneRoot>(root)
        .and_then(|root| root.0.path().cloned())
}

#[test]
fn under_play_the_main_scene_root_loads_the_open_scene() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_over(dir.path(), meadow_over_main());

    let root = spawn_root(&mut app, "scene.bsn");

    assert_eq!(spawned_names(&mut app, root), vec!["Meadow".to_string()]);
}

#[test]
fn a_hud_spawned_first_is_left_alone_and_the_level_spawns_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_over(dir.path(), meadow_over_main());

    let hud = spawn_root(&mut app, "hud.bsn");
    let level = spawn_root(&mut app, "scene.bsn");

    assert_eq!(spawned_names(&mut app, hud), vec!["Hud".to_string()]);
    assert_eq!(spawned_names(&mut app, level), vec!["Meadow".to_string()]);
    let meadows = app
        .world_mut()
        .query::<&Name>()
        .iter(app.world())
        .filter(|name| name.as_str() == "Meadow")
        .count();
    assert_eq!(meadows, 1);
}

#[test]
fn an_in_memory_root_is_left_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_over(dir.path(), meadow_over_main());
    let handle = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(named_scene("Built"), PathBuf::new()));

    let root = app.world_mut().spawn(JackdawSceneRoot(handle)).id();

    assert_eq!(spawned_names(&mut app, root), vec!["Built".to_string()]);
}

#[test]
fn a_root_inserted_again_to_restart_is_redirected_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_over(dir.path(), meadow_over_main());
    let root = spawn_root(&mut app, "scene.bsn");
    spawned_names(&mut app, root);

    let handle: Handle<JackdawScene> = app.world().resource::<AssetServer>().load("scene.bsn");
    app.world_mut()
        .entity_mut(root)
        .insert(JackdawSceneRoot(handle));

    assert_eq!(
        asked_path(&app, root),
        Some(AssetPath::from("levels/meadow.bsn"))
    );
}

#[test]
fn an_open_scene_whose_name_holds_a_hash_loads_as_a_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_over(
        dir.path(),
        Some(PieSceneOverride::new("levels/boss#2.bsn", "scene.bsn")),
    );

    let root = spawn_root(&mut app, "scene.bsn");

    assert_eq!(spawned_names(&mut app, root), vec!["Boss".to_string()]);
}

#[test]
fn without_an_override_the_game_loads_its_own_scene() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut app = app_over(dir.path(), None);

    let root = spawn_root(&mut app, "scene.bsn");

    assert_eq!(spawned_names(&mut app, root), vec!["Default".to_string()]);
}
