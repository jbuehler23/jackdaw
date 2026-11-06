//! BSN over BRP: `jackdaw/apply_bsn` spawns BSN text into the live world,
//! `jackdaw/entity_bsn` serializes a live entity back to BSN text.

use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult, error_codes};
use jackdaw_bsn::{AstNodeRef, BsnPatch, BsnSceneAssets, SceneBsnAst};
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
    // later apply starts from a clean world.
    for &entity in &spawned {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<AstNodeRef>();
        }
    }
    world.remove_resource::<SceneBsnAst>();

    if let Some(prior_ast) = prior_ast {
        world.insert_resource(prior_ast);
    }

    let entities: Vec<u64> = spawned.iter().map(|entity| entity.to_bits()).collect();
    Ok(json!({ "entities": entities }))
}

pub fn jackdaw_entity_bsn_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let bits = params
        .as_ref()
        .and_then(|p| p.get("entity"))
        .and_then(Value::as_u64)
        .ok_or_else(|| invalid_params("expected {\"entity\": <entity bits>}".into()))?;
    let root = Entity::try_from_bits(bits)
        .ok_or_else(|| invalid_params(format!("invalid entity bits {bits}")))?;

    let text = entity_bsn(world, root).map_err(invalid_params)?;
    Ok(json!({ "bsn": text }))
}

/// The BSN text for `root` and its descendants, read from the world without
/// writing to it.
///
/// The pure half of [`jackdaw_entity_bsn_handler`], so the editor's own
/// remote surface emits one node exactly the way the game side does
/// rather than growing a second conversion.
pub fn entity_bsn(world: &World, root: Entity) -> Result<String, String> {
    let no_names = bevy::platform::collections::HashMap::default();
    entity_bsn_naming(world, root, &no_names)
}

/// [`entity_bsn`] for a world that knows names for assets the asset server holds
/// no path for. Written `""` means cleared, so a handle with a name has to say it.
pub fn entity_bsn_naming(
    world: &World,
    root: Entity,
    names: &dyn jackdaw_bsn::AssetNames,
) -> Result<String, String> {
    if world.get_entity(root).is_err() {
        return Err(format!("entity {root} does not exist"));
    }

    let mut ast = SceneBsnAst::default();
    let mut root_node = None;

    // Parents first, so children attach to an existing parent node.
    let mut stack = vec![(root, None::<Entity>)];
    while let Some((entity, parent_node)) = stack.pop() {
        let name = world
            .get::<Name>(entity)
            .map(|name| BsnPatch::Name(name.to_string()));
        let node = ast.create_entity_node(name.into_iter().collect());
        match parent_node {
            Some(parent_node) => ast.add_child_to_ast(parent_node, node),
            None => {
                ast.add_to_roots(node);
                root_node = Some(node);
            }
        }

        for type_id in reflected_component_type_ids(world, entity) {
            jackdaw_bsn::sync::component_to_node(world, entity, type_id, names, &mut ast, node);
        }

        if let Some(children) = world.get::<Children>(entity) {
            for child in children.iter().rev() {
                if world.get_entity(child).is_err() {
                    continue;
                }
                stack.push((child, Some(node)));
            }
        }
    }

    root_node
        .map(|node| jackdaw_bsn::emit_entity(&ast, node))
        .ok_or_else(|| format!("no AST node for entity {root}"))
}

/// Type ids of the entity's reflected components, minus the ones BSN
/// encodes structurally (Name via the #label, hierarchy via Children),
/// the document's bookkeeping components, and `VisibilityClass`, which a hook
/// on the mesh components fills in and which has no document form.
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
        TypeId::of::<bevy::camera::visibility::VisibilityClass>(),
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
