//! Example showing how to register custom game components with the editor.
//!
//! Components that implement `EditorMeta` get descriptions and categories in the
//! component picker. Components without it still appear under "Game" automatically.
//!
//! Run with: `cargo run --example custom_components`

use bevy::prelude::*;
use jackdaw::{EditorMeta, ReflectEditorMeta, prelude::*};

fn main() -> AppExit {
    App::new()
        .add_plugins((DefaultPlugins, EditorPlugins::default()))
        .register_type::<Health>()
        .register_type::<Speed>()
        .register_type::<Team>()
        .register_type::<DamageOverTime>()
        .register_type::<Interactable>()
        .add_systems(Startup, spawn_scene)
        .run()
}

// --- Gameplay components ---

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, EditorMeta)]
struct Health {
    pub current: f32,
    pub max: f32,
}

impl EditorMeta for Health {
    fn description() -> &'static str {
        "Tracks entity health points"
    }
    fn category() -> &'static str {
        "Gameplay"
    }
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, EditorMeta)]
struct Speed {
    pub value: f32,
}

impl EditorMeta for Speed {
    fn description() -> &'static str {
        "Movement speed multiplier"
    }
    fn category() -> &'static str {
        "Gameplay"
    }
}

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, EditorMeta)]
struct DamageOverTime {
    pub damage_per_second: f32,
    pub duration: f32,
}

impl EditorMeta for DamageOverTime {
    fn description() -> &'static str {
        "Applies damage each second for a duration"
    }
    fn category() -> &'static str {
        "Gameplay"
    }
}

// --- AI components ---

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, EditorMeta)]
struct Team {
    pub id: u32,
}

impl EditorMeta for Team {
    fn description() -> &'static str {
        "Faction/team assignment for AI"
    }
    fn category() -> &'static str {
        "AI"
    }
}

// --- Component without EditorMeta (still works, appears under "Game") ---

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Interactable {
    pub radius: f32,
}

fn spawn_scene(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
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

    // Player: visible cube with custom components pre-attached
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
    ));

    // Enemy: second entity showing different custom components
    commands.spawn((
        Name::new("Enemy"),
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.5, 1.0))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.9, 0.2, 0.2),
            ..default()
        })),
        Transform::from_xyz(4.0, 0.75, 0.0),
        Team { id: 2 },
        DamageOverTime {
            damage_per_second: 10.0,
            duration: 5.0,
        },
    ));
}
