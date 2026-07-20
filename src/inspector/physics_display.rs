//! Command + helper backing for the `physics.enable` / `physics.disable`
//! operators. The dedicated Physics inspector section was removed in
//! favour of letting users add `RigidBody`, `AvianCollider`, and friends
//! through the standard component picker; the operators stay so the
//! command palette can still toggle the canonical bundle in one shot.

use avian3d::prelude::*;
use bevy::{ecs::reflect::AppTypeRegistry, prelude::*};
use jackdaw_avian_integration::AvianCollider;

use crate::commands::{AddComponent, CommandGroup, CommandHistory, EditorCommand};

pub(crate) const RIGID_BODY_TYPE_PATH: &str = "avian3d::dynamics::rigid_body::RigidBody";
pub(crate) const AVIAN_COLLIDER_TYPE_PATH: &str = "jackdaw_avian_integration::AvianCollider";

/// Command that disables physics on an entity. Captures the full pre-disable
/// state (`RigidBody`, `AvianCollider`, and all derived avian components in
/// the document) so undo restores them.
pub(crate) struct DisablePhysics {
    entity: Entity,
    /// Document patches that were removed, cloned for restore on undo.
    removed_patches: Vec<jackdaw_bsn::BsnPatch>,
    /// Derived components that were cleared on execute, for re-adding on undo.
    removed_derived: std::collections::HashSet<String>,
}

/// Whether a type path names one of the physics components this command
/// removes: the canonical pair plus every derived avian component.
fn is_physics_type_path(type_path: &str) -> bool {
    type_path == RIGID_BODY_TYPE_PATH
        || type_path == AVIAN_COLLIDER_TYPE_PATH
        || type_path.starts_with("avian3d::")
}

/// The component type path carried by a document patch, if it is a
/// component patch.
fn patch_type_path(patch: &jackdaw_bsn::BsnPatch) -> Option<&str> {
    match patch {
        jackdaw_bsn::BsnPatch::Struct(data) => Some(&data.type_path),
        jackdaw_bsn::BsnPatch::TupleStruct(data) => Some(&data.type_path),
        jackdaw_bsn::BsnPatch::Type(tp) => Some(tp),
        _ => None,
    }
}

impl DisablePhysics {
    pub(crate) fn from_world(world: &World, entity: Entity) -> Self {
        let mut removed_patches = Vec::new();
        let mut removed_derived = std::collections::HashSet::new();
        let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
        if let Some(node) = ast.ast_for(entity) {
            if let Some(patches) = ast.get_patches(node) {
                for &pe in &patches.0 {
                    if let Some(patch) = ast.get_patch(pe)
                        && patch_type_path(patch).is_some_and(is_physics_type_path)
                    {
                        removed_patches.push(patch.clone());
                    }
                }
            }
            if let Some(derived) = ast.world.get::<jackdaw_bsn::DerivedComponents>(node) {
                for type_path in derived.0.iter() {
                    if is_physics_type_path(type_path) {
                        removed_derived.insert(type_path.clone());
                    }
                }
            }
        }
        Self {
            entity,
            removed_patches,
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
        // Clean up the document (matches previous behavior)
        let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
        if let Some(node) = ast.ast_for(self.entity) {
            let physics_paths: Vec<String> = ast
                .component_type_paths(node)
                .into_iter()
                .filter(|tp| is_physics_type_path(tp))
                .collect();
            for type_path in physics_paths {
                ast.remove_component_patch(node, &type_path);
            }
            if let Some(mut derived) = ast.world.get_mut::<jackdaw_bsn::DerivedComponents>(node) {
                derived.0.clear();
            }
        }
    }

    fn undo(&mut self, world: &mut World) {
        // Restore the document patches, then mirror them onto the ECS entity.
        {
            let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
            if let Some(node) = ast.ast_for(self.entity) {
                for patch in &self.removed_patches {
                    let pe = ast.world.spawn(patch.clone()).id();
                    if let Some(patches) = ast.get_patches_mut(node) {
                        patches.0.push(pe);
                    }
                }
                if !self.removed_derived.is_empty() {
                    let set: bevy::platform::collections::HashSet<String> =
                        self.removed_derived.iter().cloned().collect();
                    match ast.world.get_mut::<jackdaw_bsn::DerivedComponents>(node) {
                        Some(mut derived) => derived.0.extend(set),
                        None => {
                            ast.world
                                .entity_mut(node)
                                .insert(jackdaw_bsn::DerivedComponents(set));
                        }
                    }
                }
            }
        }
        for patch in &self.removed_patches {
            jackdaw_bsn::apply_component_patch(world, self.entity, patch);
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

    let rb_type_id = std::any::TypeId::of::<RigidBody>();
    let rb_component_id = components_res.get_id(rb_type_id);

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
