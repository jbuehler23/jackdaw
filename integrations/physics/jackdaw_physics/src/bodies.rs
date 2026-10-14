//! Keeping avian's view of authored bodies in step with the editor: bodies
//! moved by an edit, colliders on plain meshes, and which bodies a navmesh
//! bake leaves out.

use avian3d::prelude::*;
use bevy::prelude::*;
use jackdaw_api::brush::BrushSystems;
use jackdaw_api::prelude::*;
use jackdaw_avian_integration::AvianCollider;
use jackdaw_scene_types::{Brush, NavmeshIgnored};

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        PreUpdate,
        follow_moved_bodies.run_if(in_state(AppState::Editor)),
    )
    .add_systems(
        Update,
        (
            follow_moved_bodies
                .after(BrushSystems::Recenter)
                .before(BrushSystems::Rebuild),
            build_mesh_colliders,
            mark_bodies_the_navmesh_ignores,
        )
            .run_if(in_state(AppState::Editor)),
    );
}

/// Copy an edited body's `Transform` into avian's `Position` and `Rotation`
/// while the simulation is paused.
///
/// Avian treats `Position` as the physics pose and writes it back to
/// `Transform`, so a body moved by undo, a gizmo or brush recentring would
/// snap back to where the simulation last left it. Runs before the frame's
/// physics step and again after recentring.
fn follow_moved_bodies(
    time: Res<Time<Physics>>,
    mut helper: PhysicsTransformHelper,
    moved: Query<Entity, (With<RigidBody>, Changed<Transform>)>,
) {
    if !time.is_paused() {
        return;
    }
    for entity in &moved {
        let _ = helper.update_physics_transform(entity);
    }
}

/// Build the collider an `AvianCollider` asks for on an entity drawn by a
/// mesh rather than a brush, from that mesh.
fn build_mesh_colliders(
    mut commands: Commands,
    changed: Query<
        (Entity, &AvianCollider, &Mesh3d),
        (
            Without<Brush>,
            Or<(Changed<AvianCollider>, Changed<Mesh3d>)>,
        ),
    >,
    meshes: Res<Assets<Mesh>>,
) {
    for (entity, config, mesh) in &changed {
        let collider = if config.0.requires_mesh() {
            let Some(mesh) = meshes.get(&mesh.0) else {
                continue;
            };
            Collider::try_from_constructor(config.0.clone(), Some(mesh))
        } else {
            Collider::try_from_constructor(config.0.clone(), None)
        };
        if let Some(collider) = collider {
            commands.entity(entity).insert(collider);
        }
    }
}

/// A body the simulation moves is one frame of a simulation, not ground, and
/// nothing stands on a trigger volume; both are left out of a navmesh bake.
fn mark_bodies_the_navmesh_ignores(
    mut commands: Commands,
    bodies: Query<
        (Entity, Option<&RigidBody>, Has<Sensor>, Has<NavmeshIgnored>),
        Or<(Changed<RigidBody>, Added<Sensor>)>,
    >,
    mut removed_bodies: RemovedComponents<RigidBody>,
    mut removed_sensors: RemovedComponents<Sensor>,
    current: Query<(Option<&RigidBody>, Has<Sensor>, Has<NavmeshIgnored>)>,
) {
    let mut decide = |entity: Entity, body: Option<&RigidBody>, sensor: bool, marked: bool| {
        let ignored = sensor || matches!(body, Some(RigidBody::Dynamic | RigidBody::Kinematic));
        if ignored && !marked {
            commands.entity(entity).insert(NavmeshIgnored);
        } else if !ignored && marked {
            commands.entity(entity).remove::<NavmeshIgnored>();
        }
    };
    for (entity, body, sensor, marked) in &bodies {
        decide(entity, body, sensor, marked);
    }
    for entity in removed_bodies.read().chain(removed_sensors.read()) {
        if let Ok((body, sensor, marked)) = current.get(entity) {
            decide(entity, body, sensor, marked);
        }
    }
}
