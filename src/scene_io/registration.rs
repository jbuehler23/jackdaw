use bevy::{ecs::reflect::AppTypeRegistry, prelude::*};

use super::{doc_skip_type_ids, should_skip_component};

/// Register a single ECS entity in the live scene document: create and link
/// its node, then upsert a patch for every serializable component. Ensures
/// the entity carries a stable `SceneNodeId` (adopting an existing one,
/// minting a fresh one otherwise) so the node keeps a cross-process
/// identity. Skips entities already in the document.
pub fn register_entity_in_ast(world: &mut World, entity: Entity) {
    // A dead entity has no components to read and no node worth linking.
    // Guarded here rather than at each read below, one of which is a bare
    // `world.entity(entity)` that turns a stale id into a panic a long way
    // from wherever the id went stale.
    if world.get_entity(entity).is_err() {
        return;
    }
    let Some(doc) = world.get_resource::<jackdaw_bsn::SceneBsnAst>() else {
        return;
    };
    if doc.ast_for(entity).is_some() {
        return;
    }
    if world
        .get::<jackdaw_scene_types::SceneNodeId>(entity)
        .is_none()
        && let Ok(mut entity_mut) = world.get_entity_mut(entity)
    {
        entity_mut.insert(jackdaw_scene_types::SceneNodeId::next());
    }
    let doc = world.resource::<jackdaw_bsn::SceneBsnAst>();
    let parent = world
        .get::<ChildOf>(entity)
        .map(ChildOf::parent)
        .filter(|p| doc.ast_for(*p).is_some());
    jackdaw_bsn::create_entity_in_ast(world, entity, parent);

    let registry = world.resource::<AppTypeRegistry>().clone();
    let skip_ids = doc_skip_type_ids();
    let asset_server = world.get_resource::<AssetServer>().cloned();
    // Asset paths in the document are relative to the assets folder, which is
    // what a handle's own path already is.
    let ctx = asset_server
        .as_ref()
        .map(|asset_server| jackdaw_bsn::BsnAssetContext {
            asset_server,
            parent_path: std::path::Path::new(""),
            asset_names: None,
        });
    let patches: Vec<(String, jackdaw_bsn::BsnPatch)> = {
        let reg = registry.read();
        let entity_ref = world.entity(entity);
        reg.iter()
            .filter(|registration| !skip_ids.contains(&registration.type_id()))
            .filter(|registration| {
                !should_skip_component(registration.type_info().type_path_table().path())
            })
            .filter_map(|registration| {
                let value = registration
                    .data::<ReflectComponent>()?
                    .reflect(entity_ref)?;
                let patch = match &ctx {
                    Some(ctx) => jackdaw_bsn::component_to_bsn_patch_with_assets(
                        value.as_partial_reflect(),
                        &reg,
                        ctx,
                    ),
                    None => jackdaw_bsn::component_to_bsn_patch(value.as_partial_reflect(), &reg),
                };
                Some((registration.type_info().type_path().to_string(), patch))
            })
            .collect()
    };
    for (type_path, patch) in patches {
        crate::commands::upsert_bsn_patch(world, entity, &type_path, patch);
    }
}

/// Register multiple ECS entities in the live scene document.
pub fn register_entities_in_ast(world: &mut World, entities: &[Entity]) {
    for &entity in entities {
        register_entity_in_ast(world, entity);
    }
}
