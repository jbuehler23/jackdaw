//! Hammer-style physics placement tool.
//!
//! When `EditMode::Physics` is active, only selected entities simulate.
//! Non-selected `RigidBody` entities get `RigidBodyDisabled` so they act as
//! static collision geometry. The first drag on a selected entity unpauses
//! `Time<Physics>`. Space/Escape/switching tools commits transform changes
//! and exits.

use avian3d::prelude::*;
use bevy::{
    ecs::reflect::AppTypeRegistry,
    feathers::cursor::{EntityCursor, OverrideCursor},
    picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastVisibility},
    prelude::*,
    window::SystemCursorIcon,
};

use bevy_enhanced_input::prelude::{Press, *};
use jackdaw_api::prelude::*;

use crate::brush::{BrushSelection, EditMode};
use crate::commands::{CommandGroup, CommandHistory, EditorCommand, SetJsnField};
use crate::core_extension::CoreExtensionInputContext;
use crate::draw_brush::DrawBrushState;
use crate::selection::Selection;
use jackdaw_avian_integration::simulation::{PhysicsDrag, PhysicsToolState};

pub struct PhysicsToolPlugin;

impl Plugin for PhysicsToolPlugin {
    fn build(&self, app: &mut App) {
        // Exclusive-world system runs separately (can't chain with normal systems
        // reliably in Bevy 0.18).
        app.add_systems(
            Update,
            on_edit_mode_transition.run_if(in_state(crate::AppState::Editor)),
        );
        app.add_systems(
            Update,
            (sync_selection_disable_state, physics_tool_drag)
                .chain()
                .after(on_edit_mode_transition)
                .run_if(in_state(crate::AppState::Editor)),
        );
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<PhysicsActivateOp>()
        .register_operator::<PhysicsCommitOp>();

    let ext = ctx.id();
    ctx.spawn((
        Action::<PhysicsActivateOp>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(
            KeyCode::KeyP.with_mod_keys(ModKeys::SHIFT),
            Press::default(),
        )],
    ));
    ctx.spawn((
        Action::<PhysicsCommitOp>::new(),
        ActionOf::<CoreExtensionInputContext>::new(ext),
        bindings![(KeyCode::Space, Press::default())],
    ));
}

/// Enter the Physics Tool placement mode.
///
/// Shift+P toggles the editor into `EditMode::Physics`. While the
/// modal is active:
///
/// - **Selected** `RigidBody` entities simulate under gravity / forces.
/// - **Non-selected** dynamic and kinematic bodies are paused
///   (`RigidBodyDisabled`) so they act as static collision geometry
///   for the simulated set to land on.
/// - **Static** bodies always stay solid; they're never frozen.
///
/// The simulation starts paused. The first click-and-drag on a
/// selected entity unpauses `Time<Physics>` and begins the sim;
/// subsequent drags keep nudging while gravity continues.
///
/// Exit with **Space** (`physics.commit`) to keep the settled
/// positions as one undoable change, or **Esc** to cancel the modal.
#[operator(
    id = "physics.activate",
    label = "Physics Tool",
    description = "Enter Physics Tool: Shift+P drops you into placement mode where selected \
                   bodies simulate (others freeze as static collision). Drag a selected entity \
                   to release it under gravity; Space commits the settled positions and exits, \
                   Esc cancels.",
    modal = true,
    cancel = cancel_physics_activate,
)]
pub fn physics_activate(
    _: In<OperatorParameters>,
    mut edit_mode: ResMut<EditMode>,
    mut draw_state: ResMut<DrawBrushState>,
    mut brush_selection: ResMut<BrushSelection>,
    active: ActiveModalQuery,
) -> OperatorResult {
    if !active.is_modal_running() {
        draw_state.active = None;
        brush_selection.clear();
        *edit_mode = EditMode::Physics;
        return OperatorResult::Running;
    }

    if *edit_mode != EditMode::Physics {
        return OperatorResult::Finished;
    }

    OperatorResult::Running
}

fn cancel_physics_activate(mut edit_mode: ResMut<EditMode>) {
    if *edit_mode == EditMode::Physics {
        *edit_mode = EditMode::Object;
    }
}

fn is_physics_modal_running(active: ActiveModalQuery) -> bool {
    active.is_operator(PhysicsActivateOp::ID)
}

/// Commit and exit the Physics Tool.
///
/// Space ends the placement modal. Every simulated body's transform
/// diff (from where it sat when you entered Physics mode to where it
/// settled) is pushed onto the undo stack as one `CommandGroup`, so
/// `Ctrl+Z` returns you to physics mode at those positions for
/// further nudging or re-simulation.
///
/// This is **not** a pause: physics simulation stops because
/// `EditMode::Physics` ends, not because the simulation is held.
/// To resume tweaking the same scene, hit Shift+P again.
#[operator(
    id = "physics.commit",
    label = "Commit Physics Tool",
    description = "Commit Physics Tool (Space): keep the settled positions of every simulated \
                   body, push the transform changes onto the undo stack as one entry, and \
                   return to Object mode. This is not a pause; re-enter with Shift+P.",
    is_available = is_physics_modal_running,
)]
pub(crate) fn physics_commit(
    _: In<OperatorParameters>,
    mut edit_mode: ResMut<EditMode>,
) -> OperatorResult {
    *edit_mode = EditMode::Object;
    OperatorResult::Finished
}

/// Track the previous `EditMode` to detect transitions into/out of Physics.
#[derive(Resource, Default)]
struct PreviousEditMode(EditMode);

/// Detect when `EditMode` changes to/from Physics and run entry/exit logic.
fn on_edit_mode_transition(world: &mut World) {
    let edit_mode = *world.resource::<EditMode>();
    let prev = world.get_resource_or_init::<PreviousEditMode>().0;

    if edit_mode == prev {
        return;
    }

    // Entering Physics mode
    if edit_mode == EditMode::Physics && prev != EditMode::Physics {
        enter_physics_tool(world);
    }

    // Exiting Physics mode
    if prev == EditMode::Physics && edit_mode != EditMode::Physics {
        exit_physics_tool(world);
    }

    world.resource_mut::<PreviousEditMode>().0 = edit_mode;
}

fn enter_physics_tool(world: &mut World) {
    let selection = world.resource::<Selection>();
    let selected: Vec<Entity> = selection.entities.clone();

    let mut state = world.resource_mut::<PhysicsToolState>();
    state.snapshots.clear();
    state.disabled_by_us.clear();
    state.sim_active = false;
    state.drag = None;

    // Snapshot all RigidBody transforms; disable non-selected Dynamic/Kinematic
    // bodies. Static bodies are NEVER disabled  -- they need to remain as solid
    // collision surfaces for the simulated objects to land on.
    let mut bodies: Vec<(Entity, Transform, RigidBody, bool)> = Vec::new();
    let mut query = world.query_filtered::<(Entity, &Transform, &RigidBody), With<RigidBody>>();
    for (entity, tf, rb) in query.iter(world) {
        let is_selected = selected.contains(&entity);
        bodies.push((entity, *tf, *rb, is_selected));
    }

    let mut state = world.resource_mut::<PhysicsToolState>();
    for &(entity, tf, _, _) in &bodies {
        state.snapshots.insert(entity, tf);
    }

    for &(entity, _, rb, is_selected) in &bodies {
        if rb == RigidBody::Static {
            // Static bodies are always solid  -- never disable them
            continue;
        }
        if !is_selected {
            world
                .resource_mut::<PhysicsToolState>()
                .disabled_by_us
                .insert(entity);
            if let Ok(mut ec) = world.get_entity_mut(entity) {
                ec.insert(RigidBodyDisabled);
            }
        } else {
            // Ensure selected entities are NOT disabled and are awake.
            // Removing Sleeping + zeroing isn't enough  -- we also need WakeBody
            // to register the entity in avian's island system.
            if let Ok(mut ec) = world.get_entity_mut(entity) {
                ec.remove::<(RigidBodyDisabled, Sleeping)>();
            }
        }
    }
}

fn exit_physics_tool(world: &mut World) {
    // Pause physics
    world.resource_mut::<Time<Physics>>().pause();

    // Restore cursor
    let mut cursor = world.resource_mut::<OverrideCursor>();
    if cursor.0 == Some(EntityCursor::System(SystemCursorIcon::Grabbing)) {
        cursor.0 = None;
    }

    // Restore disabled state
    let disabled: Vec<Entity> = world
        .resource::<PhysicsToolState>()
        .disabled_by_us
        .iter()
        .copied()
        .collect();
    for entity in disabled {
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.remove::<RigidBodyDisabled>();
        }
    }

    // Commit transform diffs
    commit_physics_transforms(world);

    // Clear tool state
    let mut state = world.resource_mut::<PhysicsToolState>();
    state.snapshots.clear();
    state.disabled_by_us.clear();
    state.sim_active = false;
    state.drag = None;
}

/// Toggle `RigidBodyDisabled` when selection changes during Physics mode.
/// Static bodies are never disabled  -- they must remain as solid surfaces.
fn sync_selection_disable_state(
    edit_mode: Res<EditMode>,
    selection: Res<Selection>,
    mut commands: Commands,
    mut tool_state: ResMut<PhysicsToolState>,
    bodies: Query<(Entity, &RigidBody)>,
) {
    if *edit_mode != EditMode::Physics || !selection.is_changed() {
        return;
    }

    for (entity, rb) in bodies.iter() {
        if *rb == RigidBody::Static {
            continue;
        }
        let is_selected = selection.entities.contains(&entity);
        let was_disabled = tool_state.disabled_by_us.contains(&entity);

        if is_selected && was_disabled {
            // Newly selected → enable physics
            commands.entity(entity).remove::<RigidBodyDisabled>();
            commands.queue(WakeBody(entity));
            tool_state.disabled_by_us.remove(&entity);
        } else if !is_selected && !was_disabled {
            // Newly deselected → freeze
            commands.entity(entity).insert((
                RigidBodyDisabled,
                LinearVelocity(Vec3::ZERO),
                AngularVelocity(Vec3::ZERO),
            ));
            tool_state.disabled_by_us.insert(entity);
        }
    }
}

/// Raycast-based drag interaction: click and drag selected `RigidBody`
/// entities to move them on a camera-facing plane. First drag unpauses
/// `Time<Physics>`.
fn physics_tool_drag(
    edit_mode: Res<EditMode>,
    mouse: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window>,
    camera_query: Query<(&Camera, &GlobalTransform), With<crate::viewport::MainViewportCamera>>,
    viewport_query: Query<
        (&ComputedNode, &UiGlobalTransform),
        With<crate::viewport::SceneViewport>,
    >,
    selection: Res<Selection>,
    parents: Query<&ChildOf>,
    mut tool_state: ResMut<PhysicsToolState>,
    mut ray_cast: MeshRayCast,
    mut physics_time: ResMut<Time<Physics>>,
    mut transforms: Query<&mut Transform>,
    mut velocities: Query<(&mut LinearVelocity, &mut AngularVelocity)>,
    rb_check: Query<(), With<RigidBody>>,
    mut override_cursor: ResMut<OverrideCursor>,
) {
    if *edit_mode != EditMode::Physics {
        return;
    }

    let Ok(window) = windows.single() else { return };
    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };
    let Ok((camera, cam_tf)) = camera_query.single() else {
        return;
    };

    let Some(viewport_cursor) =
        crate::viewport_util::window_to_viewport_cursor(cursor_pos, camera, &viewport_query)
    else {
        return;
    };

    let Ok(ray) = camera.viewport_to_world(cam_tf, viewport_cursor) else {
        return;
    };

    // --- Start drag ---
    if mouse.just_pressed(MouseButton::Left) && tool_state.drag.is_none() {
        let settings = MeshRayCastSettings::default().with_visibility(RayCastVisibility::Any);
        let hits = ray_cast.cast_ray(ray, &settings);

        for (hit_entity, hit_data) in hits {
            // Walk up ChildOf to find the root entity
            let mut root = *hit_entity;
            loop {
                if rb_check.contains(root) && selection.entities.contains(&root) {
                    break;
                }
                if let Ok(child_of) = parents.get(root) {
                    root = child_of.0;
                } else {
                    break;
                }
            }

            if !rb_check.contains(root) || !selection.entities.contains(&root) {
                continue;
            }

            let hit_point = hit_data.point;
            let Ok(entity_tf) = transforms.get(root) else {
                continue;
            };

            let plane_normal = cam_tf.forward().as_vec3();
            let grab_offset = entity_tf.translation - hit_point;

            // Capture starting positions of ALL selected RigidBody entities
            let mut start_positions = bevy::platform::collections::HashMap::default();
            for &sel_entity in &selection.entities {
                if rb_check.contains(sel_entity)
                    && let Ok(sel_tf) = transforms.get(sel_entity)
                {
                    start_positions.insert(sel_entity, sel_tf.translation);
                }
            }

            tool_state.drag = Some(PhysicsDrag {
                entity: root,
                plane_origin: hit_point,
                plane_normal,
                grab_offset,
                drag_start_pos: entity_tf.translation,
                start_positions,
            });

            // Activate simulation on first drag
            if !tool_state.sim_active {
                physics_time.unpause();
                tool_state.sim_active = true;
            }
            override_cursor.0 = Some(EntityCursor::System(SystemCursorIcon::Grabbing));
            break;
        }
    }

    // --- Update drag ---
    if let Some(ref drag) = tool_state.drag {
        if mouse.pressed(MouseButton::Left) {
            // Project cursor ray onto the drag plane
            let denom = ray.direction.dot(drag.plane_normal);
            if denom.abs() > 1e-6 {
                let t = (drag.plane_origin - ray.origin).dot(drag.plane_normal) / denom;
                if t > 0.0 {
                    let target = ray.origin + ray.direction * t + drag.grab_offset;
                    // Compute movement delta from the primary dragged entity
                    let delta = target - drag.drag_start_pos;

                    // Apply delta to ALL selected entities in the group
                    for (&entity, &start_pos) in &drag.start_positions {
                        if let Ok(mut tf) = transforms.get_mut(entity) {
                            tf.translation = start_pos + delta;
                        }
                        if let Ok((mut lv, mut av)) = velocities.get_mut(entity) {
                            lv.0 = Vec3::ZERO;
                            av.0 = Vec3::ZERO;
                        }
                    }
                }
            }
        } else {
            // Released
            tool_state.drag = None;
            if override_cursor.0 == Some(EntityCursor::System(SystemCursorIcon::Grabbing)) {
                override_cursor.0 = None;
            }
        }
    }
}

/// Diff snapshots vs current transforms, push one undoable `CommandGroup`
/// of `SetJsnField` commands.
fn commit_physics_transforms(world: &mut World) {
    let snapshots = world.resource::<PhysicsToolState>().snapshots.clone();

    if snapshots.is_empty() {
        return;
    }

    let registry_res = world.resource::<AppTypeRegistry>().clone();
    let processor = crate::scene_io::AstSerializerProcessor;
    let type_path = "bevy_transform::components::transform::Transform";

    let mut sub_commands: Vec<Box<dyn EditorCommand>> = Vec::new();

    for (entity, old_tf) in &snapshots {
        let Ok(entity_ref) = world.get_entity(*entity) else {
            continue;
        };
        let Some(new_tf) = entity_ref.get::<Transform>().copied() else {
            continue;
        };

        // Skip if unchanged
        let pos_diff = (old_tf.translation - new_tf.translation).length_squared();
        let rot_diff = old_tf.rotation.angle_between(new_tf.rotation);
        if pos_diff < 0.000001 && rot_diff < 0.001 {
            continue;
        }

        let registry = registry_res.read();
        let old_ser = bevy::reflect::serde::TypedReflectSerializer::with_processor(
            old_tf, &registry, &processor,
        );
        let Ok(old_json) = serde_json::to_value(&old_ser) else {
            continue;
        };
        let new_ser = bevy::reflect::serde::TypedReflectSerializer::with_processor(
            &new_tf, &registry, &processor,
        );
        let Ok(new_json) = serde_json::to_value(&new_ser) else {
            continue;
        };
        drop(registry);

        sub_commands.push(Box::new(SetJsnField {
            entity: *entity,
            type_path: type_path.to_string(),
            field_path: String::new(),
            old_value: old_json,
            new_value: new_json,
            was_derived: false,
        }));
    }

    if sub_commands.is_empty() {
        return;
    }

    let mut cmd: Box<dyn EditorCommand> = if sub_commands.len() == 1 {
        sub_commands.pop().unwrap()
    } else {
        Box::new(CommandGroup {
            label: "Physics tool".to_string(),
            commands: sub_commands,
        })
    };
    cmd.execute(world);
    let mut history = world.resource_mut::<CommandHistory>();
    history.push_executed(cmd);
}
