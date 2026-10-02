#![cfg(feature = "render")]
//! A model whose import settings list levels of detail draws them live
//! wherever it is placed, with no LOD group of its own.

use std::path::Path;
use std::time::Duration;

use bevy::asset::AssetApp;
use bevy::camera::primitives::Aabb;
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;
use jackdaw_runtime::{JackdawPlugin, LiveLevels, LodPlugin, LodSwitches, lod_distance};
use jackdaw_scene_types::model_import::{
    LevelShow, LodImportSource, ModelLevels, ModelLod, ModelLodIndex, ModelLodLevel, ModelSettings,
    model_meta, read_model_meta,
};
use jackdaw_scene_types::model_parts::{FlatModel, ModelPart, ModelParts, nodes_key};
use jackdaw_scene_types::{GltfSource, LodFade, LodOverride};

const TREE_LEVELS: [&str; 3] = [
    "trees/tree.gltf",
    "trees/tree_LOD1.gltf",
    "trees/tree_LOD2.gltf",
];

fn tree_lod(size: f32) -> ModelLod {
    ModelLod {
        version: ModelLod::VERSION,
        source: LodImportSource::SiblingFiles,
        size,
        fade: LodFade::Snap,
        levels: vec![
            ModelLodLevel {
                show: LevelShow::Model,
                screen_height: 0.5,
            },
            ModelLodLevel {
                show: LevelShow::File("tree_LOD1.gltf".into()),
                screen_height: 0.25,
            },
            ModelLodLevel {
                show: LevelShow::File("tree_LOD2.gltf".into()),
                screen_height: 0.1,
            },
        ],
    }
}

fn flat_model() -> FlatModel {
    FlatModel {
        parts: vec![ModelPart {
            mesh: Handle::default(),
            material: Handle::default(),
            material_name: None,
            local: Transform::IDENTITY,
        }],
        bounds: Aabb::from_min_max(Vec3::new(-1.0, 0.0, -1.0), Vec3::new(1.0, 2.0, 1.0)),
        needs_instance: false,
    }
}

/// An app holding the tree's levels already flattened and its settings
/// already read, with a camera `distance` away from the origin.
fn tree_app(distance: f32, loaded: bool) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, LodPlugin));
    if loaded {
        let mut models = ModelParts::default();
        for path in TREE_LEVELS {
            models.insert(path, flat_model());
        }
        app.insert_resource(models);
    }
    app.world_mut()
        .resource_mut::<ModelLodIndex>()
        .set(TREE_LEVELS[0], Some(tree_lod(2.0)));
    app.world_mut().spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection {
            fov: 60f32.to_radians(),
            ..default()
        }),
        Transform::from_xyz(0.0, 0.0, distance),
    ));
    app
}

fn place_tree(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            GltfSource {
                path: TREE_LEVELS[0].to_string(),
                scene_index: 0,
            },
            Transform::default(),
        ))
        .id()
}

fn settle(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

fn switch_at(app: &App, tree: Entity, level: usize) -> f32 {
    app.world()
        .get::<LodSwitches>(tree)
        .expect("the tree has switches")
        .ranges[level]
        .end_margin
        .start
}

#[test]
fn a_model_with_lod_settings_draws_live_levels_without_a_lod_group() {
    let mut app = tree_app(5.0, true);
    let tree = place_tree(&mut app);
    settle(&mut app, 8);

    let live = app.world().get::<LiveLevels>(tree).expect("live levels");
    assert_eq!(live.model_path(1), Some(TREE_LEVELS[1]));
    assert_eq!(live.model_path(2), Some(TREE_LEVELS[2]));
    assert_eq!(live.ready(), 0b010, "the camera stands where LOD1 shows");
    assert!(!live.parts(1).is_empty());
    assert!(app.world().get::<WorldAssetRoot>(tree).is_none());
}

#[test]
fn switches_come_from_the_settings_size_before_any_level_loads() {
    let mut app = tree_app(5.0, false);
    let tree = place_tree(&mut app);
    settle(&mut app, 2);

    let half_fov_tan = 30f32.to_radians().tan();
    assert!((switch_at(&app, tree, 0) - lod_distance(2.0, 0.5, half_fov_tan)).abs() < 0.05);
    assert!((switch_at(&app, tree, 1) - lod_distance(2.0, 0.25, half_fov_tan)).abs() < 0.05);
}

#[test]
fn an_override_moves_the_switches_of_its_placement_only() {
    let mut app = tree_app(5.0, true);
    let plain = place_tree(&mut app);
    let overridden = place_tree(&mut app);
    app.world_mut().entity_mut(overridden).insert(LodOverride {
        screen_heights: Some(vec![0.8, 0.4, 0.2]),
        fade: None,
    });
    settle(&mut app, 4);

    let half_fov_tan = 30f32.to_radians().tan();
    assert!((switch_at(&app, plain, 0) - lod_distance(2.0, 0.5, half_fov_tan)).abs() < 0.05);
    assert!((switch_at(&app, overridden, 0) - lod_distance(2.0, 0.8, half_fov_tan)).abs() < 0.05);

    app.world_mut()
        .entity_mut(overridden)
        .remove::<LodOverride>();
    settle(&mut app, 2);
    assert!((switch_at(&app, overridden, 0) - lod_distance(2.0, 0.5, half_fov_tan)).abs() < 0.05);
}

#[test]
fn a_model_whose_settings_go_loses_its_live_levels() {
    let mut app = tree_app(5.0, true);
    let tree = place_tree(&mut app);
    settle(&mut app, 4);
    assert!(app.world().get::<ModelLevels>(tree).is_some());

    app.world_mut()
        .resource_mut::<ModelLodIndex>()
        .set(TREE_LEVELS[0], None);
    settle(&mut app, 4);

    assert!(app.world().get::<ModelLevels>(tree).is_none());
    assert!(app.world().get::<LiveLevels>(tree).is_none());
}

#[test]
fn model_settings_round_trip_through_their_meta() {
    let mut lod = tree_lod(6.4);
    lod.fade = LodFade::CrossFade { width: 0.1 };
    lod.levels[1].show = LevelShow::Nodes(vec!["Tree_LOD1".into(), "Rocks".into()]);

    let bytes = model_meta(ModelSettings::drawn(Some(lod.clone())));
    let read = read_model_meta(&bytes).expect("the meta names the model loader");

    assert_eq!(read.lod, Some(lod));
    assert_eq!(
        read.gltf.load_materials,
        jackdaw_scene_types::render_assets::DRAWN_TEXTURE_USAGE
    );
}

#[test]
fn a_meta_naming_another_loader_has_no_model_settings() {
    let meta = bevy::asset::meta::AssetMeta::<bevy::gltf::GltfLoader, ()>::new(
        bevy::asset::meta::AssetAction::Load {
            loader: "bevy_gltf::loader::GltfLoader".into(),
            settings: bevy::gltf::GltfLoaderSettings::default(),
        },
    );
    let bytes = bevy::asset::meta::AssetMetaDyn::serialize(&meta);

    assert!(read_model_meta(&bytes).is_none());
}

/// A glTF of one triangle drawn by three named nodes side by side.
fn named_nodes_gltf() -> (String, Vec<u8>) {
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
  "scenes": [{{"nodes": [0, 1, 3]}}],
  "nodes": [
    {{"name": "Tree_LOD0", "mesh": 0, "translation": [0, 0, 0]}},
    {{"name": "Tree_LOD1", "children": [2]}},
    {{"name": "Crown", "mesh": 0, "translation": [5, 0, 0]}},
    {{"name": "Rocks", "mesh": 0, "translation": [9, 0, 0]}}
  ],
  "meshes": [{{"primitives": [{{"attributes": {{"POSITION": 0}}}}]}}],
  "buffers": [{{"byteLength": {length}, "uri": "tree.bin"}}],
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
    app.add_plugins(bevy::gltf::GltfPlugin::default());
    app.add_plugins(JackdawPlugin);
    app.finish();
    app.cleanup();
    app
}

fn write_tree(dir: &Path, lod: Option<ModelLod>) {
    let (gltf, buffer) = named_nodes_gltf();
    std::fs::write(dir.join("tree.gltf"), gltf).expect("gltf");
    std::fs::write(dir.join("tree.bin"), buffer).expect("buffer");
    if let Some(lod) = lod {
        std::fs::write(
            dir.join("tree.gltf.meta"),
            model_meta(ModelSettings::drawn(Some(lod))),
        )
        .expect("meta");
    }
}

/// Run `app` until `done` holds, or fail after a few seconds.
fn run_until(app: &mut App, what: &str, mut done: impl FnMut(&mut App) -> bool) {
    for _ in 0..1000 {
        app.update();
        if done(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("{what} never happened");
}

fn node_lod(levels: &[&[&str]]) -> ModelLod {
    ModelLod {
        version: ModelLod::VERSION,
        source: LodImportSource::NodeSuffixes,
        size: 2.0,
        fade: LodFade::Snap,
        levels: levels
            .iter()
            .enumerate()
            .map(|(index, nodes)| ModelLodLevel {
                show: LevelShow::Nodes(nodes.iter().map(ToString::to_string).collect()),
                screen_height: 0.5 / (index + 1) as f32,
            })
            .collect(),
    }
}

fn place_model(app: &mut App, path: &str) -> Entity {
    app.world_mut()
        .spawn((
            GltfSource {
                path: path.to_string(),
                scene_index: 0,
            },
            Transform::default(),
        ))
        .id()
}

#[test]
fn a_model_without_a_meta_is_placed_as_one_instance() {
    let project = tempfile::tempdir().expect("tempdir");
    write_tree(project.path(), None);
    let mut app = game(project.path());
    let tree = place_model(&mut app, "tree.gltf");

    run_until(&mut app, "the instance", |app| {
        app.world().get::<WorldAssetRoot>(tree).is_some()
    });

    assert!(app.world().get::<ModelLevels>(tree).is_none());
}

#[test]
fn a_model_with_a_lod_meta_is_never_placed_as_an_instance() {
    let project = tempfile::tempdir().expect("tempdir");
    write_tree(
        project.path(),
        Some(node_lod(&[
            &["Tree_LOD0", "Rocks"],
            &["Tree_LOD1", "Rocks"],
        ])),
    );
    let mut app = game(project.path());
    let tree = place_model(&mut app, "tree.gltf");

    run_until(&mut app, "the live levels", |app| {
        app.world().get::<LiveLevels>(tree).is_some()
    });
    settle(&mut app, 20);

    assert!(app.world().get::<WorldAssetRoot>(tree).is_none());
    assert_eq!(
        app.world()
            .get::<ModelLevels>(tree)
            .map(|levels| levels.models.len()),
        Some(2)
    );
}

#[test]
fn a_nodes_level_flattens_only_its_nodes() {
    let project = tempfile::tempdir().expect("tempdir");
    write_tree(project.path(), None);
    let mut app = game(project.path());
    let key = nodes_key("tree.gltf", &["Tree_LOD1".to_string(), "Rocks".to_string()]);
    app.world_mut().resource_mut::<ModelParts>().request(&key);

    run_until(&mut app, "the flattened level", |app| {
        app.world().resource::<ModelParts>().get(&key).is_some()
    });

    let model = app
        .world()
        .resource::<ModelParts>()
        .get(&key)
        .cloned()
        .unwrap();
    let mut placed: Vec<f32> = model
        .parts
        .iter()
        .map(|part| part.local.translation.x)
        .collect();
    placed.sort_by(f32::total_cmp);
    assert_eq!(placed, [5.0, 9.0], "the crown under LOD1, and the rocks");
}

#[test]
fn an_edited_meta_updates_the_models_placed_from_it() {
    let project = tempfile::tempdir().expect("tempdir");
    write_tree(
        project.path(),
        Some(node_lod(&[&["Tree_LOD0"], &["Tree_LOD1"]])),
    );
    let mut app = game(project.path());
    let tree = place_model(&mut app, "tree.gltf");
    run_until(&mut app, "two levels", |app| {
        app.world()
            .get::<ModelLevels>(tree)
            .is_some_and(|levels| levels.models.len() == 2)
    });

    write_tree(
        project.path(),
        Some(node_lod(&[&["Tree_LOD0"], &["Tree_LOD1"], &["Rocks"]])),
    );
    app.world_mut()
        .resource_mut::<ModelLodIndex>()
        .reread("tree.gltf");

    run_until(&mut app, "three levels", |app| {
        app.world()
            .get::<ModelLevels>(tree)
            .is_some_and(|levels| levels.models.len() == 3)
    });
}

#[test]
fn a_model_with_a_meta_loads_the_same_scene_as_one_without() {
    let project = tempfile::tempdir().expect("tempdir");
    write_tree(project.path(), Some(node_lod(&[&["Tree_LOD0"]])));
    let (gltf, _) = named_nodes_gltf();
    std::fs::write(project.path().join("plain.gltf"), gltf).expect("gltf");
    let mut app = game(project.path());
    app.world_mut()
        .resource_mut::<ModelLodIndex>()
        .request("tree.gltf");
    run_until(&mut app, "the meta read", |app| {
        app.world()
            .resource::<ModelLodIndex>()
            .is_known("tree.gltf")
    });
    let server = app.world().resource::<AssetServer>().clone();
    let imported: Handle<bevy::gltf::Gltf> = jackdaw_scene_types::render_assets::load_model(
        &server,
        Some(app.world().resource::<ModelLodIndex>()),
        "tree.gltf",
    );
    let plain: Handle<bevy::gltf::Gltf> = jackdaw_scene_types::render_assets::load_model(
        &server,
        Some(app.world().resource::<ModelLodIndex>()),
        "plain.gltf",
    );

    run_until(&mut app, "both models", |app| {
        let gltfs = app.world().resource::<Assets<bevy::gltf::Gltf>>();
        gltfs.contains(&imported) && gltfs.contains(&plain)
    });
    let gltfs = app.world().resource::<Assets<bevy::gltf::Gltf>>();
    let names = |handle: &Handle<bevy::gltf::Gltf>| {
        let mut names: Vec<String> = gltfs
            .get(handle)
            .unwrap()
            .named_nodes
            .keys()
            .map(ToString::to_string)
            .collect();
        names.sort();
        names
    };
    assert_eq!(names(&imported), names(&plain));
    assert_eq!(gltfs.get(&imported).unwrap().scenes.len(), 1);
}
