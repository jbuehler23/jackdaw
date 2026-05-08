//! Command + helper backing for the `physics.enable` / `physics.disable`
//! operators. The dedicated Physics inspector section was removed in
//! favour of letting users add `RigidBody`, `AvianCollider`, and friends
//! through the standard component picker; the operators stay so the
//! command palette can still toggle the canonical bundle in one shot.

use avian3d::prelude::*;
use bevy::{ecs::reflect::AppTypeRegistry, prelude::*};
use jackdaw_avian_integration::AvianCollider;

use crate::commands::{AddComponent, CommandGroup, CommandHistory, EditorCommand};

const RIGID_BODY_TYPE_PATH: &str = "avian3d::dynamics::rigid_body::RigidBody";
const AVIAN_COLLIDER_TYPE_PATH: &str = "jackdaw_avian_integration::AvianCollider";

/// Command that disables physics on an entity. Captures the full pre-disable
/// state (`RigidBody`, `AvianCollider`, and all derived avian components in the
/// AST) so undo restores them.
pub(crate) struct DisablePhysics {
    entity: Entity,
    /// Snapshot of AST components that were removed, keyed by `type_path`.
    removed_components: std::collections::HashMap<String, serde_json::Value>,
    /// Derived components that were cleared on execute, for re-adding on undo.
    removed_derived: std::collections::HashSet<String>,
}

impl DisablePhysics {
    pub(crate) fn from_world(world: &World, entity: Entity) -> Self {
        let mut removed_components = std::collections::HashMap::new();
        let mut removed_derived = std::collections::HashSet::new();
        if let Some(node) = world
            .resource::<jackdaw_jsn::SceneJsnAst>()
            .node_for_entity(entity)
        {
            for (type_path, value) in &node.components {
                if type_path == RIGID_BODY_TYPE_PATH
                    || type_path == AVIAN_COLLIDER_TYPE_PATH
                    || type_path.starts_with("avian3d::")
                {
                    removed_components.insert(type_path.clone(), value.clone());
                }
            }
            for type_path in &node.derived_components {
                if type_path == RIGID_BODY_TYPE_PATH
                    || type_path == AVIAN_COLLIDER_TYPE_PATH
                    || type_path.starts_with("avian3d::")
                {
                    removed_derived.insert(type_path.clone());
                }
            }
        }
        Self {
            entity,
            removed_components,
            removed_derived,
        }
    }
}

impl EditorCommand for DisablePhysics {
    fn execute(&mut self, world: &mut World) {
        // Remove ECS components
        if let Ok(mut ec) = world.get_entity_mut(self.entity) {
            ec.remove::<RigidBody>();
            ec.remove::<AvianCollider>();
            ec.remove::<Collider>();
        }
        // Clean up AST (matches previous behavior)
        if let Some(node) = world
            .resource_mut::<jackdaw_jsn::SceneJsnAst>()
            .node_for_entity_mut(self.entity)
        {
            node.components.remove(RIGID_BODY_TYPE_PATH);
            node.components.remove(AVIAN_COLLIDER_TYPE_PATH);
            node.derived_components.clear();
            node.components.retain(|k, _| !k.starts_with("avian3d::"));
        }
    }

    fn undo(&mut self, world: &mut World) {
        // Restore each component via AST set_component + reflection insert into ECS
        let registry = world.resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        for (type_path, value) in &self.removed_components {
            let Some(registration) = reg.get_with_type_path(type_path) else {
                continue;
            };
            let Some(reflect_component) =
                registration.data::<bevy::ecs::reflect::ReflectComponent>()
            else {
                continue;
            };
            // Deserialize JSON -> reflected value -> insert into ECS
            let deserializer =
                bevy::reflect::serde::TypedReflectDeserializer::new(registration, &reg);
            use serde::de::DeserializeSeed;
            let Ok(reflected) = deserializer.deserialize(value) else {
                continue;
            };
            let Ok(mut entity_mut) = world.get_entity_mut(self.entity) else {
                continue;
            };
            reflect_component.insert(&mut entity_mut, reflected.as_ref(), &reg);
        }
        drop(reg);
        // Restore AST entries
        if let Some(node) = world
            .resource_mut::<jackdaw_jsn::SceneJsnAst>()
            .node_for_entity_mut(self.entity)
        {
            for (type_path, value) in &self.removed_components {
                node.components.insert(type_path.clone(), value.clone());
            }
            for type_path in &self.removed_derived {
                node.derived_components.insert(type_path.clone());
            }
        }
        // Rebuild inspector to reflect restored state
        if let Ok(mut ec) = world.get_entity_mut(self.entity) {
            ec.insert(super::InspectorDirty);
        }
    }

    fn description(&self) -> &str {
        "Disable physics"
    }
}

pub(crate) fn enable_physics(world: &mut World, entity: Entity) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let reg = registry.read();
    let components_res = world.components();

    // Build AddComponent for RigidBody
    let rb_type_id = std::any::TypeId::of::<RigidBody>();
    let rb_component_id = components_res.get_id(rb_type_id);

    // Build AddComponent for AvianCollider
    let ac_type_id = std::any::TypeId::of::<AvianCollider>();
    let ac_component_id = components_res.get_id(ac_type_id);

    drop(reg);

    let mut sub_commands: Vec<Box<dyn EditorCommand>> = Vec::new();

    // Add AvianCollider FIRST so the Collider is built before RigidBody
    // triggers mass computation (avoids "no mass or inertia" warning).
    if let Some(ac_cid) = ac_component_id
        && !world
            .get_entity(entity)
            .is_ok_and(|e| e.contains::<AvianCollider>())
    {
        sub_commands.push(Box::new(AddComponent::new(
            entity,
            ac_type_id,
            ac_cid,
            AVIAN_COLLIDER_TYPE_PATH.to_string(),
        )));
    }

    if let Some(rb_cid) = rb_component_id
        && !world
            .get_entity(entity)
            .is_ok_and(|e| e.contains::<RigidBody>())
    {
        sub_commands.push(Box::new(AddComponent::new(
            entity,
            rb_type_id,
            rb_cid,
            RIGID_BODY_TYPE_PATH.to_string(),
        )));
    }

    if sub_commands.is_empty() {
        return;
    }

    let mut cmd: Box<dyn EditorCommand> = if sub_commands.len() == 1 {
        sub_commands.pop().unwrap()
    } else {
        Box::new(CommandGroup {
            label: "Enable physics".to_string(),
            commands: sub_commands,
        })
    };
    cmd.execute(world);
    let mut history = world.resource_mut::<CommandHistory>();
    history.push_executed(cmd);
}
