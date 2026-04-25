//! Inspector operators: per-component buttons (add / remove / revert)
//! and the small set of typed actions (`physics.enable` / `physics.disable`,
//! `animation.toggle_keyframe`).
//!
//! All ops route entity references through `i64` (`Entity::to_bits()`) and
//! type / field paths through `String`, since `PropertyValue` doesn't carry
//! `Entity` or `ComponentId` directly.

use bevy::ecs::component::ComponentId;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_jsn::PropertyValue;

use super::component_display::revert_component_to_baseline;
use super::physics_display::{DisablePhysics, enable_physics};
use crate::commands::{AddComponent, CommandHistory, EditorCommand};

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ComponentAddOp>()
        .register_operator::<ComponentRemoveOp>()
        .register_operator::<ComponentRevertBaselineOp>()
        .register_operator::<PhysicsEnableOp>()
        .register_operator::<PhysicsDisableOp>()
        .register_operator::<AnimationToggleKeyframeOp>();
}

fn entity_param(params: &OperatorParameters) -> Option<Entity> {
    let bits = match params.get("entity")? {
        PropertyValue::Int(i) => *i as u64,
        _ => return None,
    };
    Some(Entity::from_bits(bits))
}

fn string_param<'a>(params: &'a OperatorParameters, key: &str) -> Option<&'a str> {
    match params.get(key)? {
        PropertyValue::String(s) => Some(s.as_str()),
        _ => None,
    }
}

/// Look up the component id and type id for a fully-qualified type path.
fn component_id_for_path(
    world: &World,
    type_path: &str,
) -> Option<(ComponentId, std::any::TypeId)> {
    let registry = world.resource::<AppTypeRegistry>().read();
    let registration = registry.get_with_type_path(type_path)?;
    let type_id = registration.type_id();
    let component_id = world.components().get_id(type_id)?;
    Some((component_id, type_id))
}

#[operator(
    id = "component.add",
    label = "Add Component",
    description = "Add a component (looked up by `type_path`) to the entity in `entity` \
                   (entity bits). Pushes an `AddComponent` history entry and marks the \
                   inspector dirty.",
    allows_undo = false
)]
pub(crate) fn component_add(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = entity_param(&params) else {
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = string_param(&params, "type_path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let Some((component_id, type_id)) = component_id_for_path(world, &type_path) else {
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

#[operator(
    id = "component.remove",
    label = "Remove Component",
    description = "Remove a component (looked up by `type_path`) from the entity in \
                   `entity`. Marks the inspector dirty so the display rebuilds.",
    allows_undo = false
)]
pub(crate) fn component_remove(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = entity_param(&params) else {
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = string_param(&params, "type_path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let Some((component_id, _)) = component_id_for_path(world, &type_path) else {
            return;
        };
        if let Ok(mut ec) = world.get_entity_mut(entity) {
            ec.remove_by_id(component_id);
            ec.insert(super::InspectorDirty);
        }
    });
    OperatorResult::Finished
}

#[operator(
    id = "component.revert_baseline",
    label = "Revert To Prefab",
    description = "Restore the component's prefab baseline value on the entity in \
                   `entity`. The component must be marked as overridden in the AST.",
    allows_undo = false
)]
pub(crate) fn component_revert_baseline(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = entity_param(&params) else {
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = string_param(&params, "type_path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let Some((component_id, _)) = component_id_for_path(world, &type_path) else {
            return;
        };
        revert_component_to_baseline(world, entity, component_id);
    });
    OperatorResult::Finished
}

#[operator(
    id = "physics.enable",
    label = "Enable Physics",
    description = "Add `RigidBody` and `AvianCollider` to the entity (no-op if already present).",
    allows_undo = false
)]
pub(crate) fn physics_enable(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = entity_param(&params) else {
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

#[operator(
    id = "physics.disable",
    label = "Disable Physics",
    description = "Remove physics components from the entity, capturing the pre-disable \
                   state as a `DisablePhysics` history entry so undo restores them.",
    allows_undo = false
)]
pub(crate) fn physics_disable(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = entity_param(&params) else {
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

#[operator(
    id = "animation.toggle_keyframe",
    label = "Toggle Keyframe",
    description = "Spawn (or replace) a keyframe at the current timeline cursor for the \
                   `(entity, component_type_path, field_path)` triple. Creates the clip \
                   and track lazily if missing.",
    allows_undo = false
)]
pub(crate) fn animation_toggle_keyframe(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let Some(entity) = entity_param(&params) else {
        return OperatorResult::Cancelled;
    };
    let Some(type_path) = string_param(&params, "component_type_path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    let Some(field_path) = string_param(&params, "field_path").map(str::to_string) else {
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        super::anim_diamond::toggle_keyframe(world, entity, &type_path, &field_path);
    });
    OperatorResult::Finished
}
