//! Whole-graph extern redirect with an ecosystem dependency.
//!
//! avian3d depends on the bevy facade AND on bevy_math directly (plus
//! bevy_heavy, a third-party crate over bevy_math). If the redirect is
//! incoherent, this crate cannot even compile: the spawn function below
//! mixes avian components and bevy components in one bundle, which
//! requires both to agree on a single bevy_ecs.

use avian3d::prelude::*;
use bevy::prelude::*;

/// The probe target: a user component embedding plain data, registered
/// by nothing anywhere.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct PhysicsBody {
    pub launch_speed: f32,
}

/// Compile-time coherence proof: avian types and bevy types in one
/// bundle expression. Under a split bevy graph this fails to compile
/// with mismatched trait/type errors.
pub fn spawn_physics_ball(commands: &mut Commands) {
    commands.spawn((
        PhysicsBody { launch_speed: 5.0 },
        RigidBody::Dynamic,
        Collider::sphere(0.5),
        Transform::from_xyz(0.0, 3.0, 0.0),
    ));
}
