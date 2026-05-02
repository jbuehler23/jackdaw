//! Inspector operators: per-component buttons (add / remove / revert)
//! and the small set of typed actions (`physics.enable` / `physics.disable`,
//! `animation.toggle_keyframe`).

use bevy::ecs::component::ComponentId;
use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
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
        .register_operator::<AnimationToggleKeyframeOp>()
        .register_operator::<super::brush_display::BrushFaceClearMaterialOp>()
        .register_operator::<super::brush_display::BrushFaceApplyTextureToAllOp>()
        .register_operator::<super::brush_display::BrushFaceSetUvScalePresetOp>()
        .register_operator::<super::brush_display::BrushClearAllMaterialsOp>();
}

/// Inspector operators all act on the inspected entity (the primary
/// selection). Buttons that dispatch them get greyed out when nothing
/// is selected.
fn has_primary_selection(selection: Res<Selection>) -> bool {
    selection.primary().is_some()
}

/// Look up `(ComponentId, TypeId)` for a type path, registering
/// the component on a throwaway entity first if the world hasn't
/// seen it yet. Without this, types only `register_type`'d
/// (never inserted) would return `None` from
/// `world.components().get_id` and the picker would silently
/// no-op on click.
fn component_id_for_path(
    world: &mut World,
    type_path: &str,
) -> Option<(ComponentId, std::any::TypeId)> {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let type_id = {
        let registry_read = registry.read();
        let registration = registry_read.get_with_type_path(type_path)?;
        registration.type_id()
    };

    // Fast path: the world already knows about this component.
    if let Some(component_id) = world.components().get_id(type_id) {
        return Some((component_id, type_id));
    }

    // Slow path: insert on a throwaway entity to auto-register
    // the ComponentId. `build_reflective_default` covers types
    // without `#[derive(Default)]`.
    let (reflect_component, default_value) = {
        let registry_read = registry.read();
        let reflect_component = registry_read
            .get_with_type_path(type_path)?
            .data::<ReflectComponent>()?
            .clone();
        let default_value =
            crate::reflect_default::build_reflective_default(type_id, &registry_read)?;
        (reflect_component, default_value)
    };

    let temp = world.spawn_empty().id();
    {
        let registry_read = registry.read();
        reflect_component.insert(
            &mut world.entity_mut(temp),
            default_value.as_partial_reflect(),
            &registry_read,
        );
    }
    let component_id = world.components().get_id(type_id);
    world.despawn(temp);
    component_id.map(|id| (id, type_id))
}

/// Add a component to the target entity. Pushes a single undoable
/// history entry that recreates the component on undo.
#[operator(
    id = "component.add",
    label = "Add Component",
    description = "Add a component to the selected entity.",
    is_available = has_primary_selection,
    params(
        entity(Entity, doc = "Entity that receives the component."),
        type_path(String, doc = "Fully-qualified Bevy reflected type path of the component to add."),
    ),
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
        let Some((component_id, type_id)) = component_id_for_path(world, &type_path) else {
            warn!(
                "component.add: no registration for type_path '{type_path}'. \
                 Make sure your plugin calls `register_type::<T>()` and that `T` \
                 derives `Reflect, Default` with `#[reflect(Component, Default)]`."
            );
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
#[operator(
    id = "component.remove",
    label = "Remove Component",
    description = "Remove a component from the selected entity.",
    is_available = has_primary_selection,
    params(
        entity(Entity, doc = "Entity that loses the component."),
        type_path(String, doc = "Fully-qualified Bevy reflected type path of the component to remove."),
    ),
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

/// Restore an overridden component on a prefab instance to the prefab's
/// baseline value.
#[operator(
    id = "component.revert_baseline",
    label = "Revert To Prefab",
    description = "Restore the component to the value it had in the source prefab.",
    is_available = has_primary_selection,
    params(
        entity(Entity, doc = "Prefab instance entity to revert."),
        type_path(String, doc = "Fully-qualified Bevy reflected type path of the component to revert."),
    ),
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
        let Some((component_id, _)) = component_id_for_path(world, &type_path) else {
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
#[operator(
    id = "physics.enable",
    label = "Enable Physics",
    description = "Make the selected entity participate in the physics simulation.",
    is_available = has_primary_selection,
    params(entity(Entity, doc = "Entity to make physical.")),
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
#[operator(
    id = "physics.disable",
    label = "Disable Physics",
    description = "Stop the selected entity from participating in the physics simulation.",
    is_available = has_primary_selection,
    params(entity(Entity, doc = "Entity to make non-physical.")),
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
#[operator(
    id = "animation.toggle_keyframe",
    label = "Toggle Keyframe",
    description = "Add or replace a keyframe for this property at the current timeline cursor.",
    is_available = has_primary_selection,
    params(
        entity(Entity, doc = "Source entity whose property is being animated."),
        component_type_path(String, doc = "Fully-qualified Bevy reflected type path of the component that owns the property."),
        field_path(String, doc = "Dotted path to the field within the component (e.g. \"translation\")."),
    ),
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
