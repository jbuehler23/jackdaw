//! An authored `GltfSource` path names a file under the assets root, not one
//! beside the scene that wrote it, and a source game code inserts later is read
//! the same way.

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;
use jackdaw_runtime::{JackdawCatalogPath, JackdawPlugin, JackdawScene, JackdawSceneRoot};
use jackdaw_scene_types::GltfSource;

#[test]
fn a_prefab_in_a_subdirectory_loads_its_gltf_from_the_assets_root() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = runtime_app(dir.path());

    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(
            "#Fox\n\
             jackdaw_scene_types::types::GltfSource { path: \"characters/fox.gltf\", scene_index: 0 }\n"
                .to_owned(),
            PathBuf::from("prefabs"),
        ));
    app.world_mut().spawn(JackdawSceneRoot(scene));

    app.update();
    app.update();

    assert_eq!(
        loaded_gltf_path(app.world_mut()).as_deref(),
        Some("characters/fox.gltf#Scene0"),
        "the scene's own directory must not be prepended to the authored path"
    );
}

#[test]
fn a_gltf_source_inserted_at_runtime_spawns_its_model() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = runtime_app(dir.path());

    app.world_mut().spawn(GltfSource {
        path: "characters/fox.gltf".to_owned(),
        scene_index: 2,
    });

    app.update();

    assert_eq!(
        loaded_gltf_path(app.world_mut()).as_deref(),
        Some("characters/fox.gltf#Scene2"),
        "a source game code inserted gets the same model an authored one gets"
    );
}

/// The glTF the one `GltfSource` entity ended up pointing the world-asset
/// spawner at.
fn loaded_gltf_path(world: &mut World) -> Option<String> {
    let mut sources = world.query::<(&GltfSource, &WorldAssetRoot)>();
    let (_, root) = sources.iter(world).next()?;
    Some(root.0.path()?.to_string())
}

fn runtime_app(assets_root: &std::path::Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.insert_resource(JackdawCatalogPath(assets_root.join("catalog.bsn")));
    app.add_plugins(JackdawPlugin);
    app
}
