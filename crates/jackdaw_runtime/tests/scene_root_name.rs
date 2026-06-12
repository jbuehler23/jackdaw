//! The container entity that scene content spawns under (`JackdawSceneRoot`)
//! gets a readable `Name` from the scene's source file stem, so the editor's
//! Live tree shows the scene name instead of a bare entity id. An author-set
//! name on the root is preserved.

use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_jsn::format::{JsnAssets, JsnHeader, JsnMetadata, JsnScene};
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};

fn empty_scene() -> JsnScene {
    JsnScene {
        jsn: JsnHeader::default(),
        metadata: JsnMetadata::default(),
        editor: None,
        assets: JsnAssets::default(),
        scene: Vec::new(),
    }
}

fn headless_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::scene::ScenePlugin);
    app.add_plugins(JackdawPlugin);
    app
}

#[test]
fn scene_root_gains_a_name_from_the_scene_path() {
    let mut app = headless_app();

    let scene_handle =
        app.world_mut()
            .resource_mut::<Assets<JackdawScene>>()
            .add(JackdawScene::with_stem(
                empty_scene(),
                PathBuf::new(),
                Some("starter".to_string()),
            ));

    let root = app.world_mut().spawn(JackdawSceneRoot(scene_handle)).id();

    app.update();
    app.update();

    let name = app.world().get::<Name>(root);
    assert_eq!(
        name.map(Name::as_str),
        Some("starter"),
        "scene root should be named from the source file stem"
    );
}

#[test]
fn scene_root_keeps_an_author_supplied_name() {
    let mut app = headless_app();

    let scene_handle =
        app.world_mut()
            .resource_mut::<Assets<JackdawScene>>()
            .add(JackdawScene::with_stem(
                empty_scene(),
                PathBuf::new(),
                Some("starter".to_string()),
            ));

    let root = app
        .world_mut()
        .spawn((JackdawSceneRoot(scene_handle), Name::new("Hand Picked")))
        .id();

    app.update();
    app.update();

    let name = app.world().get::<Name>(root);
    assert_eq!(
        name.map(Name::as_str),
        Some("Hand Picked"),
        "an author-supplied root name must not be overwritten"
    );
}
