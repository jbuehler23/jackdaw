//! BSN over BRP: `jackdaw/apply_bsn` spawns BSN text into the live world,
//! `jackdaw/entity_bsn` serializes a live entity back to BSN text.

use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult, error_codes};
use jackdaw_bsn::{AstNodeRef, BsnSceneAssets, SceneBsnAst};
use serde_json::{Value, json};

fn invalid_params(message: String) -> BrpError {
    BrpError {
        code: error_codes::INVALID_PARAMS,
        message,
        data: None,
    }
}

pub fn jackdaw_apply_bsn_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let source = params
        .as_ref()
        .and_then(|p| p.get("source"))
        .and_then(|s| s.as_str())
        .ok_or_else(|| invalid_params("expected {\"source\": \"<bsn text>\"}".into()))?;

    let ast = jackdaw_bsn::parse_bsn_text(source).map_err(|err| invalid_params(err.to_string()))?;

    let prior_ast = world.remove_resource::<SceneBsnAst>();
    world.insert_resource(ast);
    world.init_resource::<BsnSceneAssets>();
    let spawned = jackdaw_bsn::spawn_from_ast(world);
    jackdaw_bsn::apply_dirty_ast_patches(world);

    // The AST resource is transient here; strip the back references so a
    // later apply or entity_bsn call starts from a clean world.
    for &entity in &spawned {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<AstNodeRef>();
        }
    }
    world.remove_resource::<SceneBsnAst>();

    if let Some(prior_ast) = prior_ast {
        world.insert_resource(prior_ast);
    }

    Ok(json!({ "entities": spawned }))
}

pub fn jackdaw_entity_bsn_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let bits = params
        .as_ref()
        .and_then(|p| p.get("entity"))
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_params("expected {\"entity\": <entity bits>}".into()))?;
    let root = Entity::try_from_bits(bits)
        .ok_or_else(|| invalid_params(format!("invalid entity bits {bits}")))?;
    if world.get_entity(root).is_err() {
        return Err(invalid_params(format!("entity {root} does not exist")));
    }

    let prior_ast = world.remove_resource::<SceneBsnAst>();
    world.init_resource::<SceneBsnAst>();

    // Build AST nodes for the entity and its descendants, parents first so
    // children attach to an existing parent node.
    let mut stack = vec![(root, None::<Entity>)];
    let mut visited = Vec::new();
    while let Some((entity, parent)) = stack.pop() {
        jackdaw_bsn::create_entity_in_ast(world, entity, parent);
        visited.push(entity);

        for type_id in reflected_component_type_ids(world, entity) {
            jackdaw_bsn::sync_to_ast(world, entity, type_id);
        }

        let children: Vec<Entity> = world
            .get::<Children>(entity)
            .map(|children| children.iter().collect())
            .unwrap_or_default();
        for child in children.into_iter().rev() {
            stack.push((child, Some(entity)));
        }
    }

    let emitted = {
        let ast = world.resource::<SceneBsnAst>();
        ast.ast_for(root)
            .map(|root_ast| jackdaw_bsn::emit_entity(ast, root_ast))
    };

    for entity in visited {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<AstNodeRef>();
        }
    }
    world.remove_resource::<SceneBsnAst>();

    if let Some(prior_ast) = prior_ast {
        world.insert_resource(prior_ast);
    }

    let text = emitted.ok_or_else(|| invalid_params(format!("no AST node for entity {root}")))?;

    Ok(json!({ "bsn": text }))
}

/// Type ids of the entity's reflected components, minus the ones BSN
/// encodes structurally (Name via the #label, hierarchy via Children)
/// and the conversion's own bookkeeping components.
fn reflected_component_type_ids(world: &World, entity: Entity) -> Vec<std::any::TypeId> {
    use std::any::TypeId;

    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let skip = [
        TypeId::of::<Name>(),
        TypeId::of::<Children>(),
        TypeId::of::<ChildOf>(),
        TypeId::of::<AstNodeRef>(),
        TypeId::of::<jackdaw_bsn::AstDirty>(),
    ];

    let Ok(entity_ref) = world.get_entity(entity) else {
        return Vec::new();
    };
    entity_ref
        .archetype()
        .components()
        .iter()
        .filter_map(|&component_id| world.components().get_info(component_id))
        .filter_map(bevy::ecs::component::ComponentInfo::type_id)
        .filter(|type_id| !skip.contains(type_id))
        .filter(|type_id| {
            registry
                .get(*type_id)
                .and_then(|registration| {
                    registration.data::<bevy::ecs::reflect::ReflectComponent>()
                })
                .is_some()
        })
        .collect()
}
