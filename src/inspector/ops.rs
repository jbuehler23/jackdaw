//! Inspector operators: per-component buttons (add / remove / revert)
//! and the small set of typed actions (`physics.enable` / `physics.disable`,
//! `animation.toggle_keyframe`).

use bevy::ecs::component::{ComponentId, Components};
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;
use jackdaw_api::prelude::*;

use super::component_display::revert_component_to_baseline;
use super::physics_display::{DisablePhysics, enable_physics};
use crate::commands::{AddComponent, CommandHistory, EditorCommand};
use crate::selection::Selection;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ComponentAddOp>()
        .register_operator::<ComponentRemoveOp>()
        .register_operator::<ComponentRevertBaselineOp>()
        .register_operator::<PhysicsEnableOp>()
        .register_operator::<PhysicsDisableOp>()
        .register_operator::<AnimationToggleKeyframeOp>();
}

/// Inspector operators all act on the inspected entity (the primary
/// selection). Buttons that dispatch them get greyed out when nothing
/// is selected.
fn has_primary_selection(selection: Res<Selection>) -> bool {
    selection.primary().is_some()
}

/// Look up the component id and type id for a fully-qualified type path.
fn component_id_for_path(
    type_registry: &AppTypeRegistry,
    components: &Components,
    type_path: &str,
) -> Option<(ComponentId, std::any::TypeId)> {
    let registry = type_registry.read();
    let registration = registry.get_with_type_path(type_path)?;
    let type_id = registration.type_id();
    let component_id = components.get_id(type_id)?;
    Some((component_id, type_id))
}

/// Add a component to the target entity.
///
/// # Parameters
/// - `entity`: the entity that will receive the component, encoded as
///   `i64` via [`Entity::to_bits()`] (use [`OperatorParameters::as_entity`]
///   to read it back).
/// - `type_path`: the fully-qualified Bevy reflected type path of the
///   component to add (e.g. `"bevy_transform::components::transform::Transform"`).
///
/// Pushes a single undoable history entry that recreates the component
/// on undo.
#[operator(
    id = "component.add",
    label = "Add Component",
    description = "Add a component to the selected entity.",
    is_available = has_primary_selection
)]
pub(crate) fn component_add(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = params.as_entity("entity") else {
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = params.as_str("type_path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let Some((component_id, type_id)) =
            component_id_for_path(&registry, world.components(), &type_path)
        else {
            return;
        };
        let mut cmd: Box<dyn EditorCommand> =
            Box::new(AddComponent::new(entity, type_id, component_id, type_path));
        cmd.execute(world);
        world.resource_mut::<CommandHistory>().push_executed(cmd);
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.insert(super::InspectorDirty);
        }
    });
    OperatorResult::Finished
}

/// Remove a component from the target entity.
///
/// # Parameters
/// - `entity`: the entity to remove the component from.
/// - `type_path`: the fully-qualified Bevy reflected type path of the
///   component to remove.
#[operator(
    id = "component.remove",
    label = "Remove Component",
    description = "Remove a component from the selected entity.",
    is_available = has_primary_selection
)]
pub(crate) fn component_remove(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = params.as_entity("entity") else {
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = params.as_str("type_path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let Some((component_id, _)) =
            component_id_for_path(&registry, world.components(), &type_path)
        else {
            return;
        };
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.remove_by_id(component_id);
            ec.insert(super::InspectorDirty);
        }
    });
    OperatorResult::Finished
}

/// Restore an overridden component on a prefab instance to the prefab's
/// baseline value.
///
/// # Parameters
/// - `entity`: the prefab instance entity.
/// - `type_path`: the fully-qualified Bevy reflected type path of the
///   component to revert.
#[operator(
    id = "component.revert_baseline",
    label = "Revert To Prefab",
    description = "Restore the component to the value it had in the source prefab.",
    is_available = has_primary_selection
)]
pub(crate) fn component_revert_baseline(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = params.as_entity("entity") else {
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = params.as_str("type_path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let Some((component_id, _)) =
            component_id_for_path(&registry, world.components(), &type_path)
        else {
            return;
        };
        if let Err(err) =
            world.run_system_cached_with(revert_component_to_baseline, (entity, component_id))
        {
            error!("revert_component_to_baseline failed: {err}");
        }
    });
    OperatorResult::Finished
}

/// Add `RigidBody` and `AvianCollider` to the entity so it participates
/// in the physics simulation. No-op if those components are already
/// present.
///
/// # Parameters
/// - `entity`: the entity to make physical.
#[operator(
    id = "physics.enable",
    label = "Enable Physics",
    description = "Make the selected entity participate in the physics simulation.",
    is_available = has_primary_selection
)]
pub(crate) fn physics_enable(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = params.as_entity("entity") else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        enable_physics(world, entity);
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.insert(super::InspectorDirty);
        }
    });
    OperatorResult::Finished
}

/// Remove physics components from the entity, capturing the pre-disable
/// state so undo restores them.
///
/// # Parameters
/// - `entity`: the entity to make non-physical.
#[operator(
    id = "physics.disable",
    label = "Disable Physics",
    description = "Stop the selected entity from participating in the physics simulation.",
    is_available = has_primary_selection
)]
pub(crate) fn physics_disable(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = params.as_entity("entity") else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let mut cmd: Box<dyn EditorCommand> = Box::new(DisablePhysics::from_world(world, entity));
        cmd.execute(world);
        world.resource_mut::<CommandHistory>().push_executed(cmd);
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.insert(super::InspectorDirty);
        }
    });
    OperatorResult::Finished
}

/// Spawn (or replace) a keyframe at the current timeline cursor for one
/// of the entity's animatable properties. Creates the clip and track
/// lazily if they don't exist yet.
///
/// # Parameters
/// - `entity`: the source entity whose property is being animated.
/// - `component_type_path`: the fully-qualified Bevy reflected type
///   path of the component that owns the property
///   (e.g. `"bevy_transform::components::transform::Transform"`).
/// - `field_path`: the dotted path to the field within that component
///   (e.g. `"translation"` or `"rotation"`).
#[operator(
    id = "animation.toggle_keyframe",
    label = "Toggle Keyframe",
    description = "Add or replace a keyframe for this property at the current timeline cursor.",
    is_available = has_primary_selection
)]
pub(crate) fn animation_toggle_keyframe(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = params.as_entity("entity") else {
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = params.as_str("component_type_path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    let Some(field_path) = params.as_str("field_path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        world
            .run_system_cached_with(
                super::anim_diamond::toggle_keyframe,
                (entity, type_path, field_path),
            )
            .ok();
    });
    OperatorResult::Finished
}
