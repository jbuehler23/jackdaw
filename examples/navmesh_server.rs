//! A test scene that loads a dungeon GLTF model and exposes a BRP server.
//! Run this, then connect from the jackdaw editor via Fetch Scene.
//! Press space to generate a navmesh locally for verification.

use bevy::{
    input::common_conditions::input_just_pressed,
    prelude::*,
    remote::{RemotePlugin, http::RemoteHttpPlugin},
};
use bevy_rerecast::{Mesh3dBackendPlugin, debug::DetailNavmeshGizmo, prelude::*};

fn main() -> AppExit {
    App::new()
        // log errors instead of panicking
        .set_error_handler(bevy_ecs::error::error)
        .add_plugins(DefaultPlugins)
        .add_plugins((RemotePlugin::default(), RemoteHttpPlugin::default()))
        .add_plugins((NavmeshPlugins::default(), Mesh3dBackendPlugin::default()))
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            generate_navmesh.run_if(input_just_pressed(KeyCode::Space)),
        )
        .add_observer(configure_camera)
        .run()
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    commands.spawn((
        Name::new("Level"),
        SceneRoot(asset_server.load("models/dungeon.glb#Scene0")),
    ));
    commands.spawn((
        DirectionalLight::default(),
        Transform::default().looking_to(Vec3::new(0.5, -1.0, 0.3), Vec3::Y),
    ));
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(-10.0, 40.0, 40.0).looking_at(Vec3::Y * 20.0, Vec3::Y),
    ));

    commands.spawn((
        Text::new("Press space to generate navmesh"),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

#[derive(Resource, Deref, DerefMut)]
struct NavmeshHandle(Handle<Navmesh>);

fn generate_navmesh(mut generator: NavmeshGenerator, mut commands: Commands) {
    let settings = NavmeshSettings::default();
    let navmesh = generator.generate(settings);
    commands.spawn(DetailNavmeshGizmo::new(&navmesh));
    commands.insert_resource(NavmeshHandle(navmesh));
}

fn configure_camera(
    trigger: On<Add, Camera>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
) {
    commands.entity(trigger.entity).insert(EnvironmentMapLight {
        diffuse_map: asset_server.load("environment_maps/voortrekker_interior_1k_diffuse.ktx2"),
        specular_map: asset_server.load("environment_maps/voortrekker_interior_1k_specular.ktx2"),
        intensity: 2000.0,
        ..default()
    });
}
