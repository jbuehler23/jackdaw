//! Minimal Bevy game with `JackdawRemotePlugin` for testing remote connection.
//!
//! Run this game, then open Jackdaw and click the connection indicator in the
//! bottom-right of the status bar to connect.
//!
//! The game registers several custom components (Health, Speed, Inventory) so
//! the editor can fetch the type registry and verify `.jsn/components.jsn`
//! auto-generation.
//!
//! Run with: `cargo run --example remote_game`

use bevy_ecs::prelude::*;
use bevy_app::prelude::*;
use jackdaw_remote::prelude::*;

fn main() -> AppExit {
    App::new()
        // log errors instead of panicking
        .set_error_handler(bevy_ecs::error::error)
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Jackdaw Remote Test Game".to_string(),
                resolution: (800, 600).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(JackdawRemotePlugin::default().with_app_name("Remote Test Game"))
        .register_type::<Health>()
        .register_type::<Speed>()
        .register_type::<Inventory>()
        .register_type::<EnemyAi>()
        .add_systems(Startup, setup)
        .add_systems(Update, rotate_cubes)
        .run()
}

// --- Game components ---

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Health {
    current: f32,
    max: f32,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Speed {
    value: f32,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Inventory {
    slots: u32,
    weight: f32,
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct EnemyAi {
    aggro_range: f32,
    patrol_speed: f32,
    chase_speed: f32,
}

/// Tag for entities that rotate.
#[derive(Component)]
struct Rotating;

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Camera
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, 5.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));

    // Light
    commands.spawn((
        Name::new("Sun"),
        DirectionalLight {
            shadows_enabled: true,
            illuminance: 10000.0,
            ..default()
        },
        Transform::default().looking_to(Vec3::new(-1.0, -2.0, -1.0), Vec3::Y),
    ));

    // Ground plane
    commands.spawn((
        Name::new("Ground"),
        Mesh3d(meshes.add(Plane3d::new(Vec3::Y, Vec2::splat(10.0)))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.3, 0.5, 0.3),
            ..default()
        })),
    ));

    // Player
    commands.spawn((
        Name::new("Player"),
        Mesh3d(meshes.add(Cuboid::new(1.0, 2.0, 1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.2, 0.6, 1.0),
            ..default()
        })),
        Transform::from_xyz(0.0, 1.0, 0.0),
        Health {
            current: 100.0,
            max: 100.0,
        },
        Speed { value: 5.0 },
        Inventory {
            slots: 20,
            weight: 0.0,
        },
        Rotating,
    ));

    // Enemies
    for i in 0..3 {
        let angle = std::f32::consts::TAU * (i as f32 / 3.0);
        let x = angle.cos() * 4.0;
        let z = angle.sin() * 4.0;

        commands.spawn((
            Name::new(format!("Enemy_{i}")),
            Mesh3d(meshes.add(Cuboid::new(0.8, 1.5, 0.8))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgb(0.9, 0.2, 0.2),
                ..default()
            })),
            Transform::from_xyz(x, 0.75, z),
            Health {
                current: 50.0,
                max: 50.0,
            },
            Speed { value: 3.0 },
            EnemyAi {
                aggro_range: 8.0,
                patrol_speed: 2.0,
                chase_speed: 5.0,
            },
            Rotating,
        ));
    }

    // HUD
    commands.spawn((
        Text::new("Jackdaw Remote Test Game\nConnect from editor status bar"),
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Px(12.0),
            ..default()
        },
    ));
}

fn rotate_cubes(time: Res<Time>, mut query: Query<&mut Transform, With<Rotating>>) {
    for mut transform in &mut query {
        transform.rotate_y(time.delta_secs() * 0.5);
    }
}
