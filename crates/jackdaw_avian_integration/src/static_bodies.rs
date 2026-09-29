//! A collider with no rigid body stands still as a static body: an
//! [`AvianCollider`] with no [`RigidBody`] on its entity or above it gets
//! [`RigidBody::Static`], because avian only resolves contacts between
//! colliders that belong to bodies.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::AvianCollider;

/// Gives every collider without a body a static one, and takes that body away
/// again with the collider.
pub struct StaticCollidersPlugin;

impl Plugin for StaticCollidersPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            (
                keep_changed_bodies,
                remove_implied_bodies,
                add_implied_bodies,
            )
                .chain(),
        );
    }
}

/// Marks a [`RigidBody::Static`] that [`StaticCollidersPlugin`] added, rather
/// than one the scene authored.
#[derive(Component, Debug)]
pub struct ImpliedStaticBody;

/// An implied body someone has since changed, to Dynamic for example, is
/// theirs now and stays when the collider goes.
fn keep_changed_bodies(
    mut commands: Commands,
    bodies: Query<(Entity, Ref<RigidBody>), With<ImpliedStaticBody>>,
) {
    for (entity, body) in &bodies {
        if body.is_changed() && !body.is_added() {
            commands.entity(entity).remove::<ImpliedStaticBody>();
        }
    }
}

fn remove_implied_bodies(
    mut commands: Commands,
    orphaned: Query<Entity, (With<ImpliedStaticBody>, Without<AvianCollider>)>,
) {
    for entity in &orphaned {
        commands
            .entity(entity)
            .remove::<(RigidBody, ImpliedStaticBody)>();
    }
}

/// A collider under an entity that has a body, or will get one, belongs to
/// that body instead.
fn add_implied_bodies(
    mut commands: Commands,
    colliders: Query<Entity, (With<AvianCollider>, Without<RigidBody>)>,
    parents: Query<&ChildOf>,
    owners: Query<(), Or<(With<RigidBody>, With<AvianCollider>)>>,
) {
    for entity in &colliders {
        if parents
            .iter_ancestors(entity)
            .any(|ancestor| owners.contains(ancestor))
        {
            continue;
        }
        commands
            .entity(entity)
            .insert((RigidBody::Static, ImpliedStaticBody));
    }
}
