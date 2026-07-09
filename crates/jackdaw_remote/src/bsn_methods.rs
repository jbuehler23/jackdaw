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
