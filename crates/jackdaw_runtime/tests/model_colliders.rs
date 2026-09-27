#![cfg(feature = "physics")]
//! A placed model saved with a collider collides in a game: its meshes get
//! colliders of the saved shape once the model has loaded.

use std::path::Path;

use avian3d::prelude::*;
use bevy::asset::AssetApp;
use bevy::prelude::*;
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};

/// A glTF holding one tetrahedron mesh under one node, and its buffer. The
/// mesh carries normals, as an exported model's does, so the loader keeps its
/// indices.
fn tetrahedron_gltf() -> (String, Vec<u8>) {
    let positions: [[f32; 3]; 4] = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
    ];
    let indices: [u16; 12] = [0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3];
    let mut buffer: Vec<u8> = positions
        .iter()
        .flatten()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    let normal_offset = buffer.len();
    buffer.extend(
        positions
            .iter()
            .map(|position| Vec3::from(*position) - Vec3::splat(0.25))
            .flat_map(|outward| outward.normalize().to_array())
            .flat_map(f32::to_le_bytes),
    );
    let index_offset = buffer.len();
    buffer.extend(indices.iter().flat_map(|index| index.to_le_bytes()));
    let gltf = format!(
        r#"{{
  "asset": {{"version": "2.0"}},
  "scene": 0,
  "scenes": [{{"nodes": [0]}}],
  "nodes": [{{"name": "Rock", "mesh": 0}}],
  "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0, "NORMAL": 2}}, "indices": 1}}]}}],
  "buffers": [{{"byteLength": {length}, "uri": "rock.bin"}}],
  "bufferViews": [
    {{"buffer": 0, "byteOffset": 0, "byteLength": {normal_offset}}},
    {{"buffer": 0, "byteOffset": {index_offset}, "byteLength": 24}},
    {{"buffer": 0, "byteOffset": {normal_offset}, "byteLength": 48}}
  ],
  "accessors": [
    {{"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 1]}},
    {{"bufferView": 1, "componentType": 5123, "count": 12, "type": "SCALAR"}},
    {{"bufferView": 2, "componentType": 5126, "count": 4, "type": "VEC3"}}
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
    app.add_plugins(PhysicsPlugins::default());
    app.finish();
    app.cleanup();
    app
}

/// Load a scene placing the rock with `shape`, and return the colliders it
/// ends up with, and on which entities.
fn load_rock(shape: &str) -> (App, Vec<(Entity, Collider)>) {
    let project = tempfile::tempdir().expect("tempdir");
    let assets = project.path().to_path_buf();
    let (gltf, buffer) = tetrahedron_gltf();
    std::fs::write(assets.join("rock.gltf"), gltf).expect("gltf");
    std::fs::write(assets.join("rock.bin"), buffer).expect("buffer");
    let mut app = game(&assets);
    let scene = format!(
        "#Rock\n\
         jackdaw_scene_types::types::GltfSource {{ path: \"rock.gltf\", scene_index: 0 }}\n\
         jackdaw_avian_integration::AvianCollider({shape})\n"
    );
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(scene, std::path::PathBuf::new()));
    app.world_mut().spawn(JackdawSceneRoot(scene));

    let mut built = Vec::new();
    for _ in 0..500 {
        app.update();
        let mut query = app.world_mut().query::<(Entity, &Collider)>();
        built = query
            .iter(app.world())
            .map(|(entity, collider)| (entity, collider.clone()))
            .collect();
        if !built.is_empty() {
            for _ in 0..5 {
                app.update();
            }
            let mut query = app.world_mut().query::<(Entity, &Collider)>();
            built = query
                .iter(app.world())
                .map(|(entity, collider)| (entity, collider.clone()))
                .collect();
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    drop(project);
    (app, built)
}

const CONSTRUCTOR: &str = "avian3d::collision::collider::constructor::ColliderConstructor";

#[test]
fn a_model_saved_with_a_trimesh_collides_on_its_mesh() {
    let (app, built) = load_rock(&format!("{CONSTRUCTOR}::TrimeshFromMesh"));
    assert_eq!(built.len(), 1, "one collider for the rock's one mesh");
    assert!(app.world().get::<Mesh3d>(built[0].0).is_some());
    assert!(built[0].1.shape().as_trimesh().is_some());
}

#[test]
fn a_model_saved_with_a_convex_hull_collides_on_its_mesh() {
    let (app, built) = load_rock(&format!("{CONSTRUCTOR}::ConvexHullFromMesh"));
    assert_eq!(built.len(), 1);
    assert!(app.world().get::<Mesh3d>(built[0].0).is_some());
    assert!(built[0].1.shape().as_convex_polyhedron().is_some());
}

#[test]
fn a_model_saved_with_a_primitive_collides_as_that_primitive() {
    let (app, built) = load_rock(&format!(
        "{CONSTRUCTOR}::Cuboid {{ x_length: 1.0, y_length: 2.0, z_length: 1.0 }}"
    ));
    assert_eq!(built.len(), 1);
    assert!(app.world().get::<Mesh3d>(built[0].0).is_none());
    assert!(built[0].1.shape().as_cuboid().is_some());
}
