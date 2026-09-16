//! A document reference handed to the asset server reads whichever of the two
//! forms is on disk, so a `.bsn` path written before an export still loads
//! once the tree has been rewritten as `.bsb`.

use std::path::Path;

use bevy::asset::io::{AssetReaderError, AssetSourceBuilders, AssetSourceId};
use bevy::asset::{AssetApp, AssetLoadError, AssetMode, AssetPlugin, LoadState};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_animation_runtime::{
    AnimationGraphAsset, AnimationGraphDef, AnimationGraphLoader, register_animation_graph_types,
};
use jackdaw_runtime::{JackdawAssetSourcePlugin, JackdawPlugin, JackdawScene, JackdawSceneRoot};

const ANCHOR: &str = "bevy_ecs::hierarchy::Children [
    #Anchor
    bevy_transform::components::transform::Transform
]
";

const SPARE: &str = "bevy_ecs::hierarchy::Children [
    #Spare
    bevy_transform::components::transform::Transform
]
";

fn graph_text() -> String {
    format!("{} {{ entry: \"idle\" }}\n", AnimationGraphDef::type_path())
}

fn source_plugin(dir: &Path) -> JackdawAssetSourcePlugin {
    JackdawAssetSourcePlugin {
        file_path: dir.to_string_lossy().into_owned(),
        ..Default::default()
    }
}

fn scene_app(dir: &Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(source_plugin(dir));
    app.add_plugins(AssetPlugin {
        file_path: dir.to_string_lossy().into_owned(),
        ..Default::default()
    });
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(JackdawPlugin);
    app
}

fn spawn_scene(app: &mut App, path: &str) -> Handle<JackdawScene> {
    let handle: Handle<JackdawScene> = app.world().resource::<AssetServer>().load(path.to_string());
    app.world_mut().spawn(JackdawSceneRoot(handle.clone()));
    handle
}

fn pump(app: &mut App, mut done: impl FnMut(&mut App) -> bool) -> bool {
    for _ in 0..200 {
        app.update();
        if done(app) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

fn named(app: &mut App, wanted: &str) -> bool {
    let mut names = app.world_mut().query::<&Name>();
    names.iter(app.world()).any(|name| name.as_str() == wanted)
}

#[test]
fn a_scene_asked_for_as_text_spawns_from_its_binary_twin() {
    let dir = tempfile::tempdir().expect("tempdir");
    jackdaw_bsn::write_document_text(&dir.path().join("scene.bsb"), ANCHOR).unwrap();

    let mut app = scene_app(dir.path());
    spawn_scene(&mut app, "scene.bsn");

    assert!(
        pump(&mut app, |app| named(app, "Anchor")),
        "the binary twin answered a reference written as text"
    );
}

#[test]
fn a_graph_asked_for_as_text_loads_from_its_binary_twin() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("animation")).unwrap();
    jackdaw_bsn::write_document_text(
        &dir.path().join("animation/walk.animgraph.bsb"),
        &graph_text(),
    )
    .unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(source_plugin(dir.path()));
    app.add_plugins(AssetPlugin {
        file_path: dir.path().to_string_lossy().into_owned(),
        ..Default::default()
    });
    register_animation_graph_types(&mut app);
    app.init_asset::<AnimationGraphAsset>()
        .init_asset_loader::<AnimationGraphLoader>();

    let handle: Handle<AnimationGraphAsset> = app
        .world()
        .resource::<AssetServer>()
        .load("animation/walk.animgraph.bsn");

    let loaded = pump(&mut app, |app| {
        app.world()
            .resource::<Assets<AnimationGraphAsset>>()
            .get(&handle)
            .is_some()
    });
    assert!(
        loaded,
        "a graph reference written as text reached the binary twin"
    );
    let graphs = app.world().resource::<Assets<AnimationGraphAsset>>();
    assert_eq!(graphs.get(&handle).expect("the graph").def.entry, "idle");
}

#[test]
fn a_document_in_neither_form_fails_as_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");

    let mut app = scene_app(dir.path());
    let handle = spawn_scene(&mut app, "nowhere.bsn");

    let failed = pump(&mut app, |app| {
        app.world()
            .resource::<AssetServer>()
            .load_state(&handle)
            .is_failed()
    });
    assert!(failed, "the load gave up rather than hanging");

    let LoadState::Failed(err) = app.world().resource::<AssetServer>().load_state(&handle) else {
        panic!("the load reported a failure");
    };
    let AssetLoadError::AssetReaderError(AssetReaderError::NotFound(missing)) = &*err else {
        panic!("the failure is the ordinary not-found one, got {err}");
    };
    assert!(
        missing.ends_with("nowhere.bsn"),
        "the missing path is named as it was written, got {}",
        missing.display()
    );
}

#[test]
fn a_document_on_disk_in_both_forms_loads_the_text_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    jackdaw_bsn::write_document_text(&dir.path().join("scene.bsn"), ANCHOR).unwrap();
    jackdaw_bsn::write_document_text(&dir.path().join("scene.bsb"), SPARE).unwrap();

    let mut app = scene_app(dir.path());
    spawn_scene(&mut app, "scene.bsn");

    assert!(
        pump(&mut app, |app| named(app, "Anchor")),
        "the text file is the one that loaded"
    );
    assert!(
        !named(&mut app, "Spare"),
        "the binary twin was left where it sat"
    );
}

fn processed_reader_registered(plugin: JackdawAssetSourcePlugin) -> bool {
    let mut app = App::new();
    app.add_plugins(plugin);
    let mut sources = app.world_mut().resource_mut::<AssetSourceBuilders>();
    let default = sources
        .get_mut(AssetSourceId::Default)
        .expect("the default source");
    default.processed_reader.is_some()
}

#[test]
fn an_unprocessed_game_gets_no_processed_half() {
    assert!(
        !processed_reader_registered(JackdawAssetSourcePlugin::default()),
        "a game that does not process assets keeps the source AssetPlugin would have built"
    );
}

#[test]
fn a_processed_game_keeps_its_processed_half() {
    assert!(
        processed_reader_registered(JackdawAssetSourcePlugin {
            mode: AssetMode::Processed,
            ..Default::default()
        }),
        "processed reads resolve twins too"
    );
}
