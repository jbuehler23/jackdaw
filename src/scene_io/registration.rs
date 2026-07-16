use bevy::{ecs::reflect::AppTypeRegistry, prelude::*};

use super::{doc_skip_type_ids, should_skip_component};

/// Register a single ECS entity in the live scene document: create and link
/// its node, then upsert a patch for every serializable component. Ensures
/// the entity carries a stable `SceneNodeId` (adopting an existing one,
/// minting a fresh one otherwise) so the node keeps a cross-process
/// identity. Skips entities already in the document.
pub fn register_entity_in_ast(world: &mut World, entity: Entity) {
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
    let values: Vec<Box<dyn bevy::reflect::PartialReflect>> = {
        let reg = registry.read();
        let entity_ref = world.entity(entity);
        reg.iter()
            .filter(|registration| !skip_ids.contains(&registration.type_id()))
            .filter(|registration| {
                !should_skip_component(registration.type_info().type_path_table().path())
            })
            .filter_map(|registration| registration.data::<ReflectComponent>())
            .filter_map(|reflect_component| reflect_component.reflect(entity_ref))
            .map(bevy::reflect::PartialReflect::to_dynamic)
            .collect()
    };
    for value in values {
        crate::commands::sync_component_to_bsn_doc(world, entity, &*value, &registry);
    }
}

/// Register multiple ECS entities in the live scene document.
pub fn register_entities_in_ast(world: &mut World, entities: &[Entity]) {
    for &entity in entities {
        register_entity_in_ast(world, entity);
    }
}
