//! A LOD group's levels show over the distances their screen heights stand
//! for at the camera's field of view.

#![cfg(feature = "render")]

use std::path::PathBuf;

use bevy::camera::primitives::Aabb;
use bevy::camera::visibility::VisibilityRange;
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;
use jackdaw_runtime::{
    JackdawCatalogPath, JackdawPlugin, JackdawScene, JackdawSceneRoot, LodPlugin, level_shows,
};
use jackdaw_scene_types::{GltfSource, LodGroup, LodLevel};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, TransformPlugin, LodPlugin));
    app
}

fn camera(app: &mut App, fov_degrees: f32) -> Entity {
    app.world_mut()
        .spawn((
            Camera3d::default(),
            Projection::Perspective(PerspectiveProjection {
                fov: fov_degrees.to_radians(),
                ..default()
            }),
        ))
        .id()
}

fn group(size: f32) -> LodGroup {
    heights(&[0.5, 0.25, 0.1], size)
}

fn heights(screen_heights: &[f32], size: f32) -> LodGroup {
    LodGroup {
        levels: screen_heights
            .iter()
            .map(|&screen_height| LodLevel { screen_height })
            .collect(),
        size,
        fade: 0.0,
    }
}

/// A group with one mesh under each of its levels, returned in level order.
fn spawn_group(app: &mut App, group: LodGroup) -> (Entity, Vec<Entity>) {
    let count = group.levels.len();
    let root = app.world_mut().spawn((group, Transform::default())).id();
    let meshes = (0..count)
        .map(|_| {
            let level = app
                .world_mut()
                .spawn((Transform::default(), ChildOf(root)))
                .id();
            app.world_mut()
                .spawn((Mesh3d::default(), ChildOf(level)))
                .id()
        })
        .collect();
    (root, meshes)
}

fn range(app: &App, mesh: Entity) -> VisibilityRange {
    app.world()
        .get::<VisibilityRange>(mesh)
        .expect("the level's mesh has a range")
        .clone()
}

fn distance(size: f32, screen_height: f32, fov_degrees: f32) -> f32 {
    size / (2.0 * screen_height * (fov_degrees.to_radians() / 2.0).tan())
}

#[test]
fn each_level_shows_between_the_distances_its_screen_heights_stand_for() {
    let mut app = app();
    camera(&mut app, 60.0);
    let (_, meshes) = spawn_group(&mut app, group(2.0));
    app.update();
    app.update();

    let first = range(&app, meshes[0]);
    let second = range(&app, meshes[1]);
    let last = range(&app, meshes[2]);
    assert_eq!(first.start_margin, 0.0..0.0);
    assert!((first.end_margin.start - distance(2.0, 0.5, 60.0)).abs() < 1e-4);
    assert_eq!(second.start_margin, first.end_margin);
    assert_eq!(last.start_margin, second.end_margin);
    assert!((last.end_margin.start - distance(2.0, 0.1, 60.0)).abs() < 1e-3);
}

#[test]
fn a_narrower_field_of_view_pushes_every_switch_further_out() {
    let mut app = app();
    let camera = camera(&mut app, 60.0);
    let (_, meshes) = spawn_group(&mut app, group(2.0));
    app.update();
    app.update();
    let wide = range(&app, meshes[0]).end_margin.start;

    app.world_mut()
        .entity_mut(camera)
        .insert(Projection::Perspective(PerspectiveProjection {
            fov: 30f32.to_radians(),
            ..default()
        }));
    app.update();

    let narrow = range(&app, meshes[0]).end_margin.start;
    assert!((narrow - distance(2.0, 0.5, 30.0)).abs() < 1e-3);
    assert!(narrow > wide);
}

#[test]
fn a_group_with_no_size_measures_its_first_level() {
    let mut app = app();
    camera(&mut app, 60.0);
    let (_, meshes) = spawn_group(&mut app, group(0.0));
    app.world_mut()
        .entity_mut(meshes[0])
        .insert(Aabb::from_min_max(
            Vec3::new(-1.0, 0.0, -0.5),
            Vec3::new(1.0, 1.0, 0.5),
        ));
    app.update();
    app.update();

    assert!((range(&app, meshes[0]).end_margin.start - distance(2.0, 0.5, 60.0)).abs() < 1e-3);
}

#[test]
fn a_mesh_that_arrives_after_its_group_gets_its_level_range() {
    let mut app = app();
    camera(&mut app, 60.0);
    let (root, meshes) = spawn_group(&mut app, group(2.0));
    app.update();
    app.update();

    let level = app.world().get::<ChildOf>(meshes[1]).unwrap().parent();
    let late = app
        .world_mut()
        .spawn((Mesh3d::default(), ChildOf(level)))
        .id();
    app.update();
    app.update();

    assert!(app.world().get::<LodGroup>(root).is_some());
    assert!(range(&app, late) == range(&app, meshes[1]));
}

#[test]
fn fading_widens_each_switch_into_a_shared_margin() {
    let mut app = app();
    camera(&mut app, 60.0);
    let (_, meshes) = spawn_group(
        &mut app,
        LodGroup {
            fade: 0.2,
            ..group(2.0)
        },
    );
    app.update();
    app.update();

    let first = range(&app, meshes[0]);
    let second = range(&app, meshes[1]);
    let switch = distance(2.0, 0.5, 60.0);
    assert!((first.end_margin.start - switch * 0.9).abs() < 1e-3);
    assert!((first.end_margin.end - switch * 1.1).abs() < 1e-3);
    assert_eq!(second.start_margin, first.end_margin);
}

#[test]
fn a_level_as_high_as_the_one_before_it_never_shows_and_the_next_takes_over_where_that_one_ends() {
    let group = heights(&[0.5, 0.25, 0.25, 0.1], 2.0);
    assert!(level_shows(&group, 0));
    assert!(level_shows(&group, 1));
    assert!(!level_shows(&group, 2));
    assert!(level_shows(&group, 3));

    let mut app = app();
    camera(&mut app, 60.0);
    let (_, meshes) = spawn_group(&mut app, group);
    app.update();
    app.update();

    let second = range(&app, meshes[1]);
    let last = range(&app, meshes[3]);
    assert!((second.end_margin.start - distance(2.0, 0.25, 60.0)).abs() < 1e-3);
    assert_eq!(last.start_margin, second.end_margin);
    assert!((last.end_margin.start - distance(2.0, 0.1, 60.0)).abs() < 1e-3);
}

const GROUP_WITH_A_LEVEL_THAT_NEVER_SHOWS: &str = "#Tree\n\
     jackdaw_scene_types::types::LodGroup { levels: [\
     jackdaw_scene_types::types::LodLevel { screen_height: 0.5 },\
     jackdaw_scene_types::types::LodLevel { screen_height: 0.25 },\
     jackdaw_scene_types::types::LodLevel { screen_height: 0.25 },\
     ] }\n\
     bevy_ecs::hierarchy::Children [\n\
     #LOD0\n\
     jackdaw_scene_types::types::GltfSource { path: \"tree.gltf\", scene_index: 0 }\n\
     ,\n\
     #LOD1\n\
     jackdaw_scene_types::types::GltfSource { path: \"tree_LOD1.gltf\", scene_index: 0 }\n\
     ,\n\
     #LOD2\n\
     jackdaw_scene_types::types::GltfSource { path: \"tree_LOD2.gltf\", scene_index: 0 }\n\
     ]\n";

#[test]
fn a_level_that_never_shows_is_never_given_a_model_when_its_scene_loads() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = runtime_app(dir.path());
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(
            GROUP_WITH_A_LEVEL_THAT_NEVER_SHOWS.to_owned(),
            PathBuf::new(),
        ));
    app.world_mut().spawn(JackdawSceneRoot(scene));
    app.update();
    app.update();

    assert_eq!(
        placed_models(app.world_mut()),
        ["tree.gltf", "tree_LOD1.gltf"]
    );
}

#[test]
fn a_level_that_comes_to_show_is_given_its_model() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = runtime_app(dir.path());
    let root = app.world_mut().spawn(heights(&[0.5, 0.25, 0.25], 0.0)).id();
    for path in ["tree.gltf", "tree_LOD1.gltf", "tree_LOD2.gltf"] {
        app.world_mut().spawn((
            GltfSource {
                path: path.to_owned(),
                scene_index: 0,
            },
            ChildOf(root),
        ));
    }
    app.update();
    assert_eq!(
        placed_models(app.world_mut()),
        ["tree.gltf", "tree_LOD1.gltf"]
    );

    app.world_mut()
        .entity_mut(root)
        .insert(heights(&[0.5, 0.25, 0.1], 0.0));
    app.update();

    assert_eq!(
        placed_models(app.world_mut()),
        ["tree.gltf", "tree_LOD1.gltf", "tree_LOD2.gltf"]
    );
}

/// The glTF files given to the world-asset spawner, in path order.
fn placed_models(world: &mut World) -> Vec<String> {
    let mut placed = world.query::<(&GltfSource, &WorldAssetRoot)>();
    let mut paths: Vec<String> = placed
        .iter(world)
        .map(|(source, _)| source.path.clone())
        .collect();
    paths.sort();
    paths
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
