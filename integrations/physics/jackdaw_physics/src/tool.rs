//! The physics tool: selected bodies simulate while the rest hold still, a
//! drag throws them about, and Space keeps where they settled as one undo
//! step.
//!
//! The tool is the modal operator `physics.activate`. While it runs:
//!
//! - selected dynamic and kinematic bodies simulate;
//! - other dynamic and kinematic bodies are frozen with `RigidBodyDisabled`,
//!   so they act as static collision for the simulated ones;
//! - static bodies stay solid.
//!
//! The simulation starts paused and the first drag on a selected body starts
//! it. Space (`physics.commit`) records every moved body with
//! `entity.set_transform` as one undo step and ends the tool. Esc puts every
//! body back where it started.

use avian3d::prelude::*;
use bevy::picking::cursor::{EntityCursor, OverrideCursor};
use bevy::picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::window::SystemCursorIcon;
use jackdaw_api::keymap::PresetInput;
use jackdaw_api::op::with_history_span;
use jackdaw_api::prelude::*;
use jackdaw_api::tools::{ModalInputs, ToolStatus};

use crate::simulation::{PhysicsDrag, PhysicsToolState};

/// What the status bar says while the tool runs.
pub const STATUS: &str = "Physics Tool | drag selected to release | Space commit | Esc cancel";

/// Transform changes smaller than these are not recorded as moves.
const MOVED_DISTANCE_SQUARED: f32 = 0.000_001;
const MOVED_ANGLE: f32 = 0.001;

pub(crate) fn register(ctx: &mut ExtensionRegistrar) {
    ctx.register_operator::<PhysicsActivateOp>()
        .register_operator::<PhysicsCommitOp>()
        .bind_operator_host::<PhysicsActivateOp>([PresetInput::key("KeyP").shift()])
        .bind_operator_host::<PhysicsCommitOp>([PresetInput::key("Space")])
        .register_toolbar_button::<PhysicsActivateOp>(Icon::Zap);
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        commit_settled_bodies.run_if(in_state(AppState::Editor)),
    );
}

/// Enter the physics tool.
///
/// Selected bodies simulate while every other moving body is frozen as
/// static collision. The simulation starts on the first drag of a selected
/// body. Space keeps the settled positions as one undo step; Esc puts every
/// body back.
#[operator(
    id = "physics.activate",
    label = "Physics Tool",
    description = "Simulate the selected bodies: drag one to release it under gravity, Space \
                   keeps where they settle as one undo step, Esc puts them back.",
    modal = true,
    allows_undo = false,
    cancel = cancel_physics_tool,
)]
pub fn physics_activate(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let running = world
        .run_system_cached(is_physics_tool_running)
        .unwrap_or(false);
    if !running {
        start(world);
        return OperatorResult::Running;
    }
    if world.resource::<PhysicsToolState>().ended {
        world.resource_mut::<PhysicsToolState>().ended = false;
        return OperatorResult::Finished;
    }
    let (confirm, cancel) = world
        .run_system_cached(|keys: ModalInputs| (keys.confirm(), keys.cancel()))
        .unwrap_or_default();
    if cancel {
        return OperatorResult::Cancelled;
    }
    if confirm {
        world.resource_mut::<PhysicsToolState>().commit_requested = true;
    }
    if let Err(error) = world.run_system_cached(freeze_unselected_bodies) {
        warn!("physics tool: {error}");
    }
    if let Err(error) = world.run_system_cached(drag_selected_bodies) {
        warn!("physics tool: {error}");
    }
    OperatorResult::Running
}

fn cancel_physics_tool(world: &mut World) {
    finish(world, Outcome::PutBack);
}

/// Whether the physics tool is the running modal.
pub fn is_physics_tool_running(active: ActiveModalQuery) -> bool {
    active.is_operator(PhysicsActivateOp::ID)
}

/// Keep where the simulated bodies settled and end the physics tool.
#[operator(
    id = "physics.commit",
    label = "Commit Physics Tool",
    description = "Keep where the simulated bodies settled, as one undo step, and end the \
                   physics tool.",
    allows_undo = false,
    is_available = is_physics_tool_running,
)]
pub fn physics_commit(
    _: In<OperatorParameters>,
    mut state: ResMut<PhysicsToolState>,
) -> OperatorResult {
    state.commit_requested = true;
    OperatorResult::Finished
}

/// Run a requested commit outside any operator, since recording the moves
/// calls `entity.set_transform`.
fn commit_settled_bodies(world: &mut World) {
    if !world.resource::<PhysicsToolState>().commit_requested {
        return;
    }
    world.resource_mut::<PhysicsToolState>().commit_requested = false;
    finish(world, Outcome::Keep);
    world.resource_mut::<PhysicsToolState>().ended = true;
}

/// Snapshot every body, freeze the unselected moving ones and wake the
/// selected ones.
fn start(world: &mut World) {
    let selected = world.resource::<Selection>().entities.clone();
    let mut bodies: Vec<(Entity, Transform, RigidBody)> = Vec::new();
    let mut query = world.query::<(Entity, &Transform, &RigidBody)>();
    for (entity, transform, body) in query.iter(world) {
        bodies.push((entity, *transform, *body));
    }

    let mut state = world.resource_mut::<PhysicsToolState>();
    *state = PhysicsToolState::default();
    for &(entity, transform, _) in &bodies {
        state.snapshots.insert(entity, transform);
    }

    for (entity, _, body) in bodies {
        if body == RigidBody::Static {
            continue;
        }
        if selected.contains(&entity) {
            if let Ok(mut entity) = world.get_entity_mut(entity) {
                entity.remove::<(RigidBodyDisabled, Sleeping)>();
            }
        } else {
            world
                .resource_mut::<PhysicsToolState>()
                .disabled_by_us
                .insert(entity);
            if let Ok(mut entity) = world.get_entity_mut(entity) {
                entity.insert(RigidBodyDisabled);
            }
        }
    }
    world.resource_mut::<ToolStatus>().0 = Some(STATUS.to_string());
}

/// How the tool ends.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    /// Record where the bodies settled.
    Keep,
    /// Put every body back where it started.
    PutBack,
}

/// Stop the simulation, free the frozen bodies, and keep or undo the moves.
fn finish(world: &mut World, outcome: Outcome) {
    world.resource_mut::<Time<Physics>>().pause();
    let mut cursor = world.resource_mut::<OverrideCursor>();
    if cursor.0 == Some(EntityCursor::System(SystemCursorIcon::Grabbing)) {
        cursor.0 = None;
    }

    let state = std::mem::take(&mut *world.resource_mut::<PhysicsToolState>());
    for entity in &state.disabled_by_us {
        if let Ok(mut entity) = world.get_entity_mut(*entity) {
            entity.remove::<RigidBodyDisabled>();
        }
    }

    let moved: Vec<(Entity, Transform, Transform)> = state
        .snapshots
        .iter()
        .filter_map(|(&entity, &start)| {
            let now = *world.get::<Transform>(entity)?;
            let distance = (start.translation - now.translation).length_squared();
            let angle = start.rotation.angle_between(now.rotation);
            (distance >= MOVED_DISTANCE_SQUARED || angle >= MOVED_ANGLE)
                .then_some((entity, start, now))
        })
        .collect();

    for &(entity, start, _) in &moved {
        put_back(world, entity, start);
    }
    if outcome == Outcome::Keep && !moved.is_empty() {
        with_history_span(world, PhysicsActivateOp::LABEL, |world| {
            for &(entity, _, now) in &moved {
                record_transform(world, entity, now);
            }
        });
    }
}

/// Return a body to `transform` and stop it.
fn put_back(world: &mut World, entity: Entity, transform: Transform) {
    let Ok(mut entity) = world.get_entity_mut(entity) else {
        return;
    };
    entity.insert(transform);
    if let Some(mut velocity) = entity.get_mut::<LinearVelocity>() {
        velocity.0 = Vec3::ZERO;
    }
    if let Some(mut velocity) = entity.get_mut::<AngularVelocity>() {
        velocity.0 = Vec3::ZERO;
    }
}

/// Write `transform` to `entity` through `entity.set_transform`, so it
/// reaches the scene document and undo.
fn record_transform(world: &mut World, entity: Entity, transform: Transform) {
    let (yaw, pitch, roll) = transform.rotation.to_euler(EulerRot::YXZ);
    let result = world
        .operator("entity.set_transform")
        .param("entity", entity)
        .param("x", f64::from(transform.translation.x))
        .param("y", f64::from(transform.translation.y))
        .param("z", f64::from(transform.translation.z))
        .param("yaw", f64::from(yaw.to_degrees()))
        .param("pitch", f64::from(pitch.to_degrees()))
        .param("roll", f64::from(roll.to_degrees()))
        .call();
    if let Err(error) = result {
        warn!("physics tool: could not record where {entity} settled: {error}");
    }
}

/// Freeze bodies that leave the selection and free the ones that join it.
/// Static bodies are never frozen: they are what the others land on.
fn freeze_unselected_bodies(
    selection: Res<Selection>,
    mut commands: Commands,
    mut state: ResMut<PhysicsToolState>,
    bodies: Query<(Entity, &RigidBody)>,
) {
    if !selection.is_changed() {
        return;
    }
    for (entity, body) in &bodies {
        if *body == RigidBody::Static {
            continue;
        }
        let selected = selection.entities.contains(&entity);
        let frozen = state.disabled_by_us.contains(&entity);
        if selected && frozen {
            commands.entity(entity).remove::<RigidBodyDisabled>();
            commands.queue(WakeBody(entity));
            state.disabled_by_us.remove(&entity);
        } else if !selected && !frozen {
            commands.entity(entity).insert((
                RigidBodyDisabled,
                LinearVelocity(Vec3::ZERO),
                AngularVelocity(Vec3::ZERO),
            ));
            state.disabled_by_us.insert(entity);
        }
    }
}

/// Drag selected bodies on a plane facing the camera. The first drag starts
/// the simulation.
#[expect(
    clippy::too_many_arguments,
    reason = "a Bevy system takes what it reads as parameters"
)]
fn drag_selected_bodies(
    mouse: Res<ButtonInput<MouseButton>>,
    cursor: ViewportCursor,
    selection: Res<Selection>,
    parents: Query<&ChildOf>,
    mut state: ResMut<PhysicsToolState>,
    mut ray_cast: MeshRayCast,
    mut physics_time: ResMut<Time<Physics>>,
    mut transforms: Query<&mut Transform>,
    mut velocities: Query<(&mut LinearVelocity, &mut AngularVelocity)>,
    bodies: Query<(), With<RigidBody>>,
    editor_entities: Query<(), With<EditorEntity>>,
    mut override_cursor: ResMut<OverrideCursor>,
) {
    let Some(window_cursor) = cursor.cursor() else {
        return;
    };

    if mouse.just_pressed(MouseButton::Left) && state.drag.is_none() {
        let (Some(camera), Some(viewport)) = (cursor.camera_entity(), cursor.viewport_entity())
        else {
            return;
        };
        let Some(ray) = cursor.ray_for(camera, viewport, window_cursor) else {
            return;
        };
        let Some((_, camera_transform)) = cursor.camera_for(camera) else {
            return;
        };
        let plane_normal = camera_transform.forward().as_vec3();
        let scene_only = |entity: Entity| !editor_entities.contains(entity);
        let settings = MeshRayCastSettings::default()
            .with_visibility(RayCastVisibility::Any)
            .with_filter(&scene_only);
        for (hit_entity, hit) in ray_cast.cast_ray(ray, &settings) {
            let Some(root) = selected_body_above(*hit_entity, &parents, &bodies, &selection) else {
                continue;
            };
            let Ok(root_transform) = transforms.get(root) else {
                continue;
            };
            let drag_start_pos = root_transform.translation;
            let start_positions: HashMap<Entity, Vec3> = selection
                .entities
                .iter()
                .filter(|entity| bodies.contains(**entity))
                .filter_map(|&entity| Some((entity, transforms.get(entity).ok()?.translation)))
                .collect();
            state.drag = Some(PhysicsDrag {
                entity: root,
                plane_origin: hit.point,
                plane_normal,
                grab_offset: drag_start_pos - hit.point,
                drag_start_pos,
                start_positions,
                camera,
                viewport,
            });
            if !state.sim_active {
                physics_time.unpause();
                state.sim_active = true;
            }
            override_cursor.0 = Some(EntityCursor::System(SystemCursorIcon::Grabbing));
            break;
        }
    }

    let Some(drag) = state.drag.clone() else {
        return;
    };
    if !mouse.pressed(MouseButton::Left) {
        state.drag = None;
        if override_cursor.0 == Some(EntityCursor::System(SystemCursorIcon::Grabbing)) {
            override_cursor.0 = None;
        }
        return;
    }
    let Some(ray) = cursor.ray_for(drag.camera, drag.viewport, window_cursor) else {
        return;
    };
    let Some(hit) = jackdaw_geometry::ray_plane_intersection(
        ray.origin,
        *ray.direction,
        drag.plane_origin,
        drag.plane_normal,
    ) else {
        return;
    };
    let delta = hit + drag.grab_offset - drag.drag_start_pos;
    for (&entity, &start) in &drag.start_positions {
        if let Ok(mut transform) = transforms.get_mut(entity) {
            transform.translation = start + delta;
        }
        if let Ok((mut linear, mut angular)) = velocities.get_mut(entity) {
            linear.0 = Vec3::ZERO;
            angular.0 = Vec3::ZERO;
        }
    }
}

/// The selected body `entity` belongs to: itself or the nearest ancestor that
/// is a selected rigid body.
fn selected_body_above(
    entity: Entity,
    parents: &Query<&ChildOf>,
    bodies: &Query<(), With<RigidBody>>,
    selection: &Selection,
) -> Option<Entity> {
    let mut at = entity;
    loop {
        if bodies.contains(at) && selection.is_selected(at) {
            return Some(at);
        }
        at = parents.get(at).ok()?.parent();
    }
}
