//! A game hears when a scene root's scene has spawned, and when it was refused.

use std::sync::{Arc, Mutex};

use bevy::asset::AssetPlugin;
use bevy::prelude::*;
use jackdaw_runtime::{
    JackdawPlugin, JackdawScene, JackdawSceneReady, JackdawSceneRoot, JackdawSceneSpawnFailed,
    JackdawSceneSpawned, SceneRefused,
};

const SCENE: &str = "#World\nbevy_transform::components::transform::Transform\n";

const RETIRED: &str = "#Overlay\njackdaw_ui::UiCanvas\n";

/// Each spawn the observer saw: the root it fired on and whether the scene's
/// `World` entity was there to find.
type Seen = Arc<Mutex<Vec<(Entity, bool)>>>;

fn watch_spawns(app: &mut App) -> Seen {
    let seen = Seen::default();
    let record = seen.clone();
    app.add_observer(
        move |spawned: On<JackdawSceneSpawned>, names: Query<&Name>| {
            let present = names.iter().any(|name| name.as_str() == "World");
            if let Ok(mut seen) = record.lock() {
                seen.push((spawned.entity, present));
            }
        },
    );
    seen
}

fn watch_failures(app: &mut App) -> Arc<Mutex<Vec<(Entity, String)>>> {
    let failed: Arc<Mutex<Vec<(Entity, String)>>> = Arc::default();
    let record = failed.clone();
    app.add_observer(move |refused: On<JackdawSceneSpawnFailed>| {
        if let Ok(mut failed) = record.lock() {
            failed.push((refused.entity, refused.error.clone()));
        }
    });
    failed
}

#[test]
fn a_scene_file_fires_spawned_once_with_its_entities_present() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("level.bsn"), SCENE).expect("write the scene");
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin {
        file_path: dir.path().to_string_lossy().into_owned(),
        ..default()
    });
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(JackdawPlugin);
    let seen = watch_spawns(&mut app);

    let handle: Handle<JackdawScene> = app.world().resource::<AssetServer>().load("level.bsn");
    let root = app.world_mut().spawn(JackdawSceneRoot(handle)).id();
    for _ in 0..2000 {
        app.update();
        if app.world().get::<JackdawSceneReady>(root).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    for _ in 0..5 {
        app.update();
    }

    assert!(app.world().get::<JackdawSceneReady>(root).is_some());
    assert_eq!(*seen.lock().expect("the record"), vec![(root, true)]);
}

#[test]
fn a_reloaded_scene_fires_spawned_again() {
    let mut app = runtime_app();
    let seen = watch_spawns(&mut app);
    let handle = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(SCENE.into(), ".".into()));
    let root = app.world_mut().spawn(JackdawSceneRoot(handle.clone())).id();
    app.update();
    app.update();
    assert_eq!(seen.lock().expect("the record").len(), 1);

    if let Some(mut scene) = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .get_mut(&handle)
    {
        *scene = JackdawScene::new(SCENE.into(), ".".into());
    }
    app.update();
    app.update();

    assert!(app.world().get::<JackdawSceneReady>(root).is_some());
    assert_eq!(
        *seen.lock().expect("the record"),
        vec![(root, true), (root, true)]
    );
}

#[test]
fn a_refused_scene_fires_failed_and_not_spawned() {
    let mut app = runtime_app();
    let seen = watch_spawns(&mut app);
    let failed = watch_failures(&mut app);
    let handle = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(RETIRED.into(), ".".into()));
    let root = app.world_mut().spawn(JackdawSceneRoot(handle)).id();
    app.update();
    app.update();

    assert!(app.world().get::<SceneRefused>(root).is_some());
    assert!(app.world().get::<JackdawSceneReady>(root).is_none());
    assert!(seen.lock().expect("the record").is_empty());
    let failed = failed.lock().expect("the record");
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].0, root);
    assert!(
        failed[0].1.contains("UiCanvas"),
        "the error names what was refused: {}",
        failed[0].1
    );
}

fn runtime_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(JackdawPlugin);
    app
}
