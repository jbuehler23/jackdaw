use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_runtime::{FallbackCamera2d, JackdawPlugin, JackdawScene, JackdawSceneRoot};

const SCENE_2D_WITHOUT_CAMERA: &str = "
#Scene2d
jackdaw_scene_types::Scene2dRoot
bevy_transform::components::transform::Transform
bevy_ecs::hierarchy::Children [
    #Crate
    bevy_transform::components::transform::Transform {
        translation: glam::Vec3 { x: 40.0, y: 8.0, z: 0.0 },
    }
]
";

const SCENE_2D_WITH_CAMERA: &str = "
#Scene2d
jackdaw_scene_types::Scene2dRoot
bevy_transform::components::transform::Transform
bevy_ecs::hierarchy::Children [
    #\"Main Camera\"
    bevy_camera::components::Camera2d
    bevy_transform::components::transform::Transform {
        translation: glam::Vec3 { x: 5.0, y: 0.0, z: 0.0 },
    }
]
";

const SCENE_3D_WITHOUT_CAMERA: &str = "
#Root
bevy_transform::components::transform::Transform
bevy_ecs::hierarchy::Children [
    #Crate
    bevy_transform::components::transform::Transform
]
";

fn headless_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(JackdawPlugin);
    app
}

fn spawn_scene(app: &mut App, text: &str) -> Entity {
    let handle =
        app.world_mut()
            .resource_mut::<Assets<JackdawScene>>()
            .add(JackdawScene::with_stem(
                text.to_string(),
                PathBuf::new(),
                Some("level".to_string()),
            ));
    let root = app.world_mut().spawn(JackdawSceneRoot(handle)).id();
    app.update();
    app.update();
    root
}

fn cameras_2d(app: &mut App) -> Vec<(Entity, Transform)> {
    app.world_mut()
        .query_filtered::<(Entity, &Transform), With<Camera2d>>()
        .iter(app.world())
        .map(|(entity, transform)| (entity, *transform))
        .collect()
}

#[test]
fn a_2d_scene_with_no_camera_gets_one_at_the_world_origin() {
    let mut app = headless_app();
    spawn_scene(&mut app, SCENE_2D_WITHOUT_CAMERA);

    let cameras = cameras_2d(&mut app);
    assert_eq!(cameras.len(), 1, "exactly one camera: {cameras:?}");
    let (camera, transform) = cameras[0];
    assert_eq!(transform, Transform::IDENTITY, "at the origin, scale 1");
    assert!(app.world().get::<FallbackCamera2d>(camera).is_some());
}

#[test]
fn a_game_camera_spawned_in_code_means_no_fallback() {
    let mut app = headless_app();
    let game_camera = app.world_mut().spawn(Camera2d).id();

    spawn_scene(&mut app, SCENE_2D_WITHOUT_CAMERA);

    let cameras: Vec<Entity> = cameras_2d(&mut app).into_iter().map(|(e, _)| e).collect();
    assert_eq!(cameras, vec![game_camera]);
}

#[test]
fn two_camera_less_2d_scenes_share_one_fallback() {
    let mut app = headless_app();
    spawn_scene(&mut app, SCENE_2D_WITHOUT_CAMERA);
    spawn_scene(&mut app, SCENE_2D_WITHOUT_CAMERA);

    assert_eq!(cameras_2d(&mut app).len(), 1);
}

#[test]
fn a_game_camera_added_later_replaces_the_fallback() {
    let mut app = headless_app();
    spawn_scene(&mut app, SCENE_2D_WITHOUT_CAMERA);
    assert_eq!(cameras_2d(&mut app).len(), 1);

    let game_camera = app.world_mut().spawn(Camera2d).id();
    app.update();

    let cameras: Vec<Entity> = cameras_2d(&mut app).into_iter().map(|(e, _)| e).collect();
    assert_eq!(cameras, vec![game_camera]);
}

#[test]
fn a_2d_scene_with_its_own_camera_gets_none_added() {
    let mut app = headless_app();
    spawn_scene(&mut app, SCENE_2D_WITH_CAMERA);

    let cameras = cameras_2d(&mut app);
    assert_eq!(cameras.len(), 1, "only the scene's camera: {cameras:?}");
    assert_eq!(
        cameras[0].1.translation,
        Vec3::new(5.0, 0.0, 0.0),
        "and it is the authored one"
    );
}

#[test]
fn a_3d_scene_with_no_camera_gets_none() {
    let mut app = headless_app();
    spawn_scene(&mut app, SCENE_3D_WITHOUT_CAMERA);

    let cameras = app
        .world_mut()
        .query_filtered::<(), With<Camera>>()
        .iter(app.world())
        .count();
    assert_eq!(cameras, 0);
}

#[test]
fn the_fallback_camera_goes_with_its_scene() {
    let mut app = headless_app();
    let root = spawn_scene(&mut app, SCENE_2D_WITHOUT_CAMERA);
    assert_eq!(cameras_2d(&mut app).len(), 1);

    app.world_mut().entity_mut(root).despawn();
    app.update();
    app.update();

    assert!(
        cameras_2d(&mut app).is_empty(),
        "the camera left with the scene"
    );
}

const SCENE_2D_WITH_TWO_CAMERAS: &str = "
#Scene2d
jackdaw_scene_types::Scene2dRoot
bevy_transform::components::transform::Transform
bevy_ecs::hierarchy::Children [
    #First
    bevy_camera::components::Camera2d
    bevy_transform::components::transform::Transform
    --
    #Second
    bevy_camera::components::Camera2d
    bevy_transform::components::transform::Transform
    --
    #Overlay
    bevy_camera::components::Camera2d
    bevy_camera::camera::Camera { order: 1 }
    bevy_transform::components::transform::Transform
]
";

#[test]
fn two_authored_cameras_at_one_window_order_leave_the_first_drawing() {
    let mut app = headless_app();
    spawn_scene(&mut app, SCENE_2D_WITH_TWO_CAMERAS);
    let mut active: Vec<(String, bool)> = app
        .world_mut()
        .query::<(&Name, &Camera)>()
        .iter(app.world())
        .map(|(name, camera)| (name.to_string(), camera.is_active))
        .collect();
    active.sort();
    assert_eq!(
        active,
        [
            ("First".to_string(), true),
            ("Overlay".to_string(), true),
            ("Second".to_string(), false),
        ],
        "one camera per window order, the first in scene order"
    );
}
