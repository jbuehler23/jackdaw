//! Custom game components in the editor.
//!
//! `reflect_documentation` (enabled in the workspace `bevy`
//! dep) captures `///` doc comments into the type registry; the
//! picker uses them as tooltip text. Override with
//! `#[reflect(@EditorDescription("..."))]`, or bucket into a
//! named group with `#[reflect(@EditorCategory("Gameplay"))]`.
//!
//! Run: `cargo run --example components_in_editor`

use avian3d::prelude::PhysicsPlugins;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::EnhancedInputPlugin;
use jackdaw::prelude::*;

fn main() -> AppExit {
    App::new()
        // Ambient plugins at the binary boundary. Editor crates
        // assert presence, so user plugins can add the same
        // plugins without a duplicate panic.
        .add_plugins((
            DefaultPlugins,
            PhysicsPlugins::default(),
            EnhancedInputPlugin,
        ))
        .add_plugins(EditorPlugins::default())
        .add_systems(Startup, spawn_scene)
        .run()
}

// Gameplay components below: no `register_type` calls;
// `reflect_auto_register` (default-on in bevy 0.18) handles it.

/// Tracks entity health points.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, @EditorCategory::new("Gameplay"))]
struct Health {
    pub current: f32,
    pub max: f32,
}

/// Movement speed multiplier.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, @EditorCategory::new("Gameplay"))]
struct Speed {
    pub value: f32,
}

/// Applies damage each second for a duration.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, @EditorCategory::new("Gameplay"))]
struct DamageOverTime {
    pub damage_per_second: f32,
    pub duration: f32,
}

// --- AI components ---

/// Faction/team assignment for AI.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, @EditorCategory::new("AI"))]
struct Team {
    pub id: u32,
}

// --- Component without category override (still works, appears under "Game") ---

/// Range within which the player can interact with this entity.
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
        Interactable { radius: 1.5 },
    ));
}
