use bevy::prelude::*;
use jackdaw::prelude::*;

fn main() -> AppExit {
    App::new()
        // log errors instead of panicking
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins((
            DefaultPlugins,
            EnhancedInputPlugin,
            PhysicsPlugins::default(),
            EditorPlugins::default(),
        ))
        .add_systems(Startup, spawn_scene)
        .run()
}

fn spawn_scene(mut commands: Commands) {
    // Directional light with shadows, positioned away from origin
    commands.spawn((
        Name::new("Sun"),
        DirectionalLight {
            shadows_enabled: true,
            illuminance: 10000.0,
            ..default()
        },
        Transform::from_xyz(10.0, 20.0, 10.0).with_rotation(Quat::from_euler(
            EulerRot::XYZ,
            -0.8,
            0.4,
            0.0,
        )),
    ));
}
