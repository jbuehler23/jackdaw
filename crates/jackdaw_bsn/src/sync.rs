//! Sync helpers for writing ECS component state back to the BSN AST.
//!
//! Used by reflection-based operations (enum variant switches, component
//! reverts) where the concrete type is not known at compile time.

use std::any::TypeId;

use bevy::ecs::reflect::{AppTypeRegistry, ReflectComponent};
use bevy::prelude::*;

use crate::{AstNodeRef, SceneBsnAst, component_to_bsn_patch};

/// After modifying an ECS component, sync its current value to the BSN AST.
///
/// Should be called after every ECS component mutation that needs to persist
/// to the scene file.
pub fn sync_to_ast(world: &mut World, entity: Entity, component_type_id: TypeId) {
    let Some(ast_ref) = world.get::<AstNodeRef>(entity) else {
        return;
    };
    let patches_entity = ast_ref.patches_entity;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();

    let Some(registration) = registry.get(component_type_id) else {
        return;
    };
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        return;
    };
    let Some(reflected) = reflect_component.reflect(world.entity(entity)) else {
        return;
    };

    let patch = component_to_bsn_patch(reflected, &registry);

    let type_path = reflected
        .get_represented_type_info()
        .map(|info| info.type_path().to_string())
        .unwrap_or_default();

    drop(registry);

    let mut ast = world.resource_mut::<SceneBsnAst>();

    if let Some(existing) = ast.find_patch_by_type_path(patches_entity, &type_path) {
        ast.set_patch(existing, patch);
    } else {
        let patch_entity = ast.world.spawn(patch).id();
        if let Some(patches) = ast.get_patches_mut(patches_entity) {
            patches.0.push(patch_entity);
        }
    }
}

/// Create an AST node for a new ECS entity and link them.
///
/// Inserts the node into the parent's `Children` patch (or roots if no parent).
pub fn create_entity_in_ast(world: &mut World, entity: Entity, parent: Option<Entity>) {
    let name = world.get::<Name>(entity).map(ToString::to_string);

    let mut initial_patches = Vec::new();
    if let Some(name) = name {
        initial_patches.push(crate::BsnPatch::Name(name));
    }

    let ast_entity = {
        let mut ast = world.resource_mut::<SceneBsnAst>();
        let ast_entity = ast.create_entity_node(initial_patches);

        let parent_ast = parent.and_then(|p| ast.ast_for(p));
        if let Some(parent_ast) = parent_ast {
            ast.add_child_to_ast(parent_ast, ast_entity);
        } else {
            ast.add_to_roots(ast_entity);
        }
        ast.link(entity, ast_entity);
        ast_entity
    };

    world.entity_mut(entity).insert(crate::AstNodeRef {
        patches_entity: ast_entity,
    });
}

/// Remove an ECS entity's AST node and unlink it.
///
/// Delegates to [`SceneBsnAst::remove_entity_node`], the single implementation
/// that detaches the node from its parent's `Children` patch (or roots),
/// despawns the AST subtree, and unlinks the ECS mapping for the node and every
/// linked descendant.
pub fn delete_entity_from_ast(world: &mut World, entity: Entity) {
    world
        .resource_mut::<SceneBsnAst>()
        .remove_entity_node(entity);
}

/// After reparenting an ECS entity, move its AST node to the new parent's
/// Children block.
pub fn sync_hierarchy_to_ast(world: &mut World, entity: Entity, new_parent: Option<Entity>) {
    let Some(ast_ref) = world.get::<AstNodeRef>(entity) else {
        return;
    };
    let node_ast = ast_ref.patches_entity;

    let parent_ast = new_parent.and_then(|p| world.get::<AstNodeRef>(p).map(|r| r.patches_entity));

    let mut ast = world.resource_mut::<SceneBsnAst>();
    let old_parent_ast = ast.ast_parent_of(node_ast);
    ast.move_to_parent(node_ast, old_parent_ast, parent_ast);
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::transform::components::Transform;

    fn world_with_registry() -> World {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        registry.write().register::<Transform>();
        world.insert_resource(registry);
        world.insert_resource(SceneBsnAst::default());
        world
    }

    #[test]
    fn sync_to_ast_updates_existing_patch_with_current_component_value() {
        let mut world = world_with_registry();

        // Create an AST node for the entity, then give it a Transform component.
        let entity = world.spawn_empty().id();
        create_entity_in_ast(&mut world, entity, None);

        world
            .entity_mut(entity)
            .insert(Transform::from_xyz(1.0, 2.0, 3.0));

        sync_to_ast(&mut world, entity, TypeId::of::<Transform>());

        let ast = world.resource::<SceneBsnAst>();
        let patches_entity = world.get::<AstNodeRef>(entity).unwrap().patches_entity;
        let value = crate::get_bsn_field(
            ast,
            patches_entity,
            "bevy_transform::components::transform::Transform",
            "translation",
        );
        let Some(crate::BsnValue::Struct(data)) = value else {
            panic!("expected translation to sync as a struct value");
        };
        let x = data
            .fields
            .0
            .iter()
            .find(|f| f.name == "x")
            .and_then(|f| match &f.value {
                crate::BsnValue::Float(f) => Some(*f),
                _ => None,
            })
            .expect("x field");
        assert!((x - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn sync_to_ast_creates_patch_when_missing() {
        let mut world = world_with_registry();

        let entity = world.spawn(Transform::from_xyz(4.0, 5.0, 6.0)).id();
        // Attach an AST node with no patches at all.
        let mut ast = world.resource_mut::<SceneBsnAst>();
        let ast_entity = ast.create_entity_node(Vec::new());
        ast.add_to_roots(ast_entity);
        ast.link(entity, ast_entity);
        world.entity_mut(entity).insert(AstNodeRef {
            patches_entity: ast_entity,
        });

        sync_to_ast(&mut world, entity, TypeId::of::<Transform>());

        let ast = world.resource::<SceneBsnAst>();
        assert!(
            ast.find_patch_by_type_path(
                ast_entity,
                "bevy_transform::components::transform::Transform"
            )
            .is_some(),
            "sync_to_ast should create a patch for a component with no prior patch"
        );
    }

    #[test]
    fn create_entity_in_ast_links_new_entity_under_parent() {
        let mut world = world_with_registry();

        let parent = world.spawn_empty().id();
        create_entity_in_ast(&mut world, parent, None);
        let parent_ast = world.get::<AstNodeRef>(parent).unwrap().patches_entity;

        let child = world.spawn(Name::new("Child")).id();
        create_entity_in_ast(&mut world, child, Some(parent));
        let child_ast = world.get::<AstNodeRef>(child).unwrap().patches_entity;

        let ast = world.resource::<SceneBsnAst>();
        assert_eq!(
            ast.get_children_ast(parent_ast),
            vec![child_ast],
            "child AST node should be linked under the parent's Children patch"
        );
        assert!(
            !ast.roots.contains(&child_ast),
            "linked child should not also appear at the root level"
        );
    }

    #[test]
    fn delete_entity_from_ast_removes_node_and_subtree() {
        let mut world = world_with_registry();

        let parent = world.spawn_empty().id();
        create_entity_in_ast(&mut world, parent, None);
        let parent_ast = world.get::<AstNodeRef>(parent).unwrap().patches_entity;

        let child = world.spawn_empty().id();
        create_entity_in_ast(&mut world, child, Some(parent));
        let child_ast = world.get::<AstNodeRef>(child).unwrap().patches_entity;

        delete_entity_from_ast(&mut world, child);

        let ast = world.resource::<SceneBsnAst>();
        assert!(
            ast.get_children_ast(parent_ast).is_empty(),
            "deleted child should be removed from the parent's Children patch"
        );
        assert!(
            ast.world.get_entity(child_ast).is_err(),
            "deleted child's AST node entity should be despawned"
        );
        assert!(
            ast.ast_for(child).is_none(),
            "ECS -> AST link should be removed for the deleted entity"
        );
    }

    #[test]
    fn delete_subtree_leaves_no_stale_links_for_descendants() {
        let mut world = world_with_registry();

        // parent -> child -> grandchild, every level linked to an ECS entity.
        let parent = world.spawn_empty().id();
        create_entity_in_ast(&mut world, parent, None);

        let child = world.spawn_empty().id();
        create_entity_in_ast(&mut world, child, Some(parent));
        let child_ast = world.get::<AstNodeRef>(child).unwrap().patches_entity;

        let grandchild = world.spawn_empty().id();
        create_entity_in_ast(&mut world, grandchild, Some(child));
        let grandchild_ast = world.get::<AstNodeRef>(grandchild).unwrap().patches_entity;

        delete_entity_from_ast(&mut world, parent);

        let ast = world.resource::<SceneBsnAst>();
        // Every level's link must be gone in both directions, not just the top.
        assert!(
            ast.ecs_to_ast.is_empty(),
            "no ecs_to_ast entries should survive a subtree delete, found: {:?}",
            ast.ecs_to_ast.keys().collect::<Vec<_>>()
        );
        assert!(
            ast.ast_to_ecs.is_empty(),
            "no ast_to_ecs entries should survive a subtree delete, found: {:?}",
            ast.ast_to_ecs.keys().collect::<Vec<_>>()
        );
        assert!(
            ast.ast_for(child).is_none() && ast.ast_for(grandchild).is_none(),
            "descendant ECS entities must be unlinked"
        );
        assert!(
            ast.world.get_entity(child_ast).is_err()
                && ast.world.get_entity(grandchild_ast).is_err(),
            "descendant AST node entities must be despawned"
        );
    }

    #[test]
    fn sync_hierarchy_to_ast_reparents_node() {
        let mut world = world_with_registry();

        let parent_a = world.spawn_empty().id();
        create_entity_in_ast(&mut world, parent_a, None);
        let parent_a_ast = world.get::<AstNodeRef>(parent_a).unwrap().patches_entity;

        let parent_b = world.spawn_empty().id();
        create_entity_in_ast(&mut world, parent_b, None);
        let parent_b_ast = world.get::<AstNodeRef>(parent_b).unwrap().patches_entity;

        let child = world.spawn_empty().id();
        create_entity_in_ast(&mut world, child, Some(parent_a));
        let child_ast = world.get::<AstNodeRef>(child).unwrap().patches_entity;

        sync_hierarchy_to_ast(&mut world, child, Some(parent_b));

        let ast = world.resource::<SceneBsnAst>();
        assert!(
            ast.get_children_ast(parent_a_ast).is_empty(),
            "old parent should no longer list the reparented child"
        );
        assert_eq!(
            ast.get_children_ast(parent_b_ast),
            vec![child_ast],
            "new parent should list the reparented child"
        );
    }
}
