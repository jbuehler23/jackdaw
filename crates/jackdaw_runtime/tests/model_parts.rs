#![cfg(feature = "render")]
//! A model flattened into its parts puts each part where spawning the model
//! would have put the same mesh.

use std::path::Path;

use bevy::asset::AssetApp;
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;
use jackdaw_runtime::JackdawPlugin;
use jackdaw_scene_types::model_parts::ModelParts;

/// A glTF with one triangle mesh drawn by two nodes, the second a child of
/// the first, each with a transform of its own.
fn nested_gltf() -> (String, Vec<u8>) {
    let positions: [[f32; 3]; 3] = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
    let buffer: Vec<u8> = positions
        .iter()
        .flatten()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let gltf = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "scene": 0,
  "scenes": [{{"nodes": [0]}}],
  "nodes": [
    {{"name": "Base", "mesh": 0, "translation": [1, 0, 0], "rotation": [0, 0.7071068, 0, 0.7071068], "scale": [2, 2, 2], "children": [1]}},
    {{"name": "Top", "mesh": 0, "translation": [0, 3, 0]}}
  ],
  "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0}}}}]}}],
  "buffers": [{{"byteLength": {length}, "uri": "nested.bin"}}],
  "bufferViews": [{{"buffer": 0, "byteOffset": 0, "byteLength": {length}}}],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]}}
  ]
}}"#,
        length = buffer.len(),
    );
    (gltf, buffer)
}

fn game(assets: &Path) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin {
        file_path: assets.to_string_lossy().into_owned(),
        ..default()
    });
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.init_asset::<Mesh>();
    app.add_plugins(JackdawPlugin);
    app.add_plugins(bevy::gltf::GltfPlugin::default());
    app.finish();
    app.cleanup();
    app
}

#[test]
fn a_flattened_model_places_each_part_where_a_spawned_one_does() {
    let project = tempfile::tempdir().expect("tempdir");
    let (gltf, buffer) = nested_gltf();
    std::fs::write(project.path().join("nested.gltf"), gltf).expect("gltf");
    std::fs::write(project.path().join("nested.bin"), buffer).expect("buffer");
    let mut app = game(project.path());

    let placed = Transform::from_xyz(4.0, 0.0, -2.0);
    let server = app.world().resource::<AssetServer>().clone();
    app.world_mut()
        .resource_mut::<ModelParts>()
        .request("nested.gltf");
    let scene = server
        .load_builder()
        .with_settings(jackdaw_scene_types::render_assets::model_settings)
        .load("nested.gltf#Scene0");
    app.world_mut().spawn((placed, WorldAssetRoot(scene)));

    let mut spawned = Vec::new();
    for _ in 0..500 {
        app.update();
        spawned = app
            .world_mut()
            .query_filtered::<&GlobalTransform, With<Mesh3d>>()
            .iter(app.world())
            .map(GlobalTransform::translation)
            .collect::<Vec<_>>();
        let ready = app
            .world()
            .resource::<ModelParts>()
            .get("nested.gltf")
            .is_some();
        if ready && spawned.len() == 2 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let model = app
        .world()
        .resource::<ModelParts>()
        .get("nested.gltf")
        .cloned()
        .expect("the model flattened");
    let mut flattened: Vec<Vec3> = model
        .parts
        .iter()
        .map(|part| (placed * part.local).translation)
        .collect();

    let order = |points: &mut Vec<Vec3>| {
        points.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));
    };
    order(&mut spawned);
    order(&mut flattened);
    assert_eq!(spawned.len(), 2, "both nodes spawned a mesh");
    assert_eq!(flattened.len(), 2, "both nodes flattened to a part");
    for (spawned, flattened) in spawned.iter().zip(&flattened) {
        assert!(
            spawned.distance(*flattened) < 1e-4,
            "a part flattened to {flattened} where the spawned mesh stands at {spawned}"
        );
    }
}
