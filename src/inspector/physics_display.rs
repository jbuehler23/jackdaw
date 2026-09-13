//! Command + helper backing for the `physics.enable` / `physics.disable`
//! operators. The dedicated Physics inspector section was removed in
//! favour of letting users add `RigidBody`, `AvianCollider`, and friends
//! through the standard component picker; the operators stay so the
//! command palette can still toggle the canonical bundle in one shot.

use avian3d::prelude::*;
use bevy::prelude::*;
use jackdaw_avian_integration::AvianCollider;

use crate::commands::{AddComponent, CommandGroup, CommandHistory, EditorCommand};

pub(crate) const RIGID_BODY_TYPE_PATH: &str = "avian3d::dynamics::rigid_body::RigidBody";
pub(crate) const AVIAN_COLLIDER_TYPE_PATH: &str = "jackdaw_avian_integration::AvianCollider";

/// Command that disables physics on an entity. Captures authored physics
/// patches (`RigidBody`, `AvianCollider`, and any other avian overrides the
/// user pinned) so undo restores them. Runtime `#[require]` companions are
/// never in the document and are rebuilt by avian on re-enable.
pub(crate) struct DisablePhysics {
    entity: Entity,
    /// Document patches that were removed, cloned for restore on undo.
    removed_patches: Vec<jackdaw_bsn::BsnPatch>,
}

/// Whether a type path names one of the physics components this command
/// removes: the canonical pair plus any authored avian overrides.
fn is_physics_type_path(type_path: &str) -> bool {
    type_path == RIGID_BODY_TYPE_PATH
        || type_path == AVIAN_COLLIDER_TYPE_PATH
        || type_path.starts_with("avian3d::")
}

/// The physics component a running preview owns on `entity`, if it owns
/// one of the three [`DisablePhysics`] takes off.
///
/// Matched by [`std::any::TypeId`] rather than by type path: the concrete
/// `Collider` avian builds carries a path this module cannot spell.
pub(crate) fn preview_owned_physics(world: &World, entity: Entity) -> Option<&'static str> {
    [
        (std::any::TypeId::of::<RigidBody>(), "RigidBody"),
        (std::any::TypeId::of::<AvianCollider>(), "AvianCollider"),
        (std::any::TypeId::of::<Collider>(), "Collider"),
    ]
    .into_iter()
    .find(|(type_id, _)| crate::preview_context::preview_writes(world, entity, *type_id))
    .map(|(_, name)| name)
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
        let ast = world.resource::<jackdaw_bsn::SceneBsnAst>();
        if let Some(node) = ast.ast_for(entity)
            && let Some(patches) = ast.get_patches(node)
        {
            for &pe in &patches.0 {
                if let Some(patch) = ast.get_patch(pe)
                    && patch_type_path(patch).is_some_and(is_physics_type_path)
                {
                    removed_patches.push(patch.clone());
                }
            }
        }
        Self {
            entity,
            removed_patches,
        }
    }
}

impl EditorCommand for DisablePhysics {
    fn execute(&mut self, world: &mut World) {
        let tracked = world.get::<jackdaw_bsn::AstNodeRef>(self.entity).is_some();
        if tracked {
            {
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
                }
            }
            crate::scene_io::resync_entity_from_ast(world, self.entity);
        } else if let Ok(mut ec) = world.get_entity_mut(self.entity) {
            ec.remove::<RigidBody>();
            ec.remove::<AvianCollider>();
            ec.remove::<Collider>();
        }
    }

    fn undo(&mut self, world: &mut World) {
        {
            let mut ast = world.resource_mut::<jackdaw_bsn::SceneBsnAst>();
            if let Some(node) = ast.ast_for(self.entity) {
                for patch in &self.removed_patches {
                    if patch_type_path(patch).is_some_and(|type_path| {
                        ast.find_patch_by_type_path(node, type_path).is_some()
                    }) {
                        continue;
                    }
                    let pe = ast.world.spawn(patch.clone()).id();
                    if let Some(patches) = ast.get_patches_mut(node) {
                        patches.0.push(pe);
                    }
                }
            }
        }
        crate::scene_io::resync_entity_from_ast(world, self.entity);
        if let Ok(mut ec) = world.get_entity_mut(self.entity) {
            ec.insert(super::InspectorDirty);
        }
    }

    fn description(&self) -> &str {
        "Disable physics"
    }
}

pub(crate) fn enable_physics(world: &mut World, entity: Entity) {
    if world
        .resource::<jackdaw_bsn::SceneBsnAst>()
        .ast_for(entity)
        .is_none()
    {
        warn!(
            "enable_physics: entity {entity:?} is not tracked in the scene document; \
             physics was not added."
        );
        return;
    }

    let rb_type_id = std::any::TypeId::of::<RigidBody>();
    let rb_component_id = world.components().get_id(rb_type_id);

    let ac_type_id = std::any::TypeId::of::<AvianCollider>();
    let ac_component_id = world.components().get_id(ac_type_id);

    let mut pending: Vec<AddComponent> = Vec::new();

    // Add AvianCollider FIRST so the Collider is built before RigidBody
    // triggers mass computation (avoids "no mass or inertia" warning).
    if let Some(ac_cid) = ac_component_id
        && !world
            .get_entity(entity)
            .is_ok_and(|e| e.contains::<AvianCollider>())
    {
        pending.push(AddComponent::new(
            entity,
            ac_type_id,
            ac_cid,
            AVIAN_COLLIDER_TYPE_PATH.to_string(),
        ));
    }

    if let Some(rb_cid) = rb_component_id
        && !world
            .get_entity(entity)
            .is_ok_and(|e| e.contains::<RigidBody>())
    {
        pending.push(AddComponent::new(
            entity,
            rb_type_id,
            rb_cid,
            RIGID_BODY_TYPE_PATH.to_string(),
        ));
    }

    let mut commands: Vec<Box<dyn EditorCommand>> = Vec::new();
    for mut cmd in pending {
        cmd.execute(world);
        commands.push(Box::new(cmd));
    }
    if commands.is_empty() {
        return;
    }

    let cmd = if commands.len() == 1 {
        commands.remove(0)
    } else {
        Box::new(CommandGroup {
            label: "Enable physics".to_string(),
            commands,
        })
    };
    world.resource_mut::<CommandHistory>().push_executed(cmd);
}
