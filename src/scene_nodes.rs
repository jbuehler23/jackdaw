//! Which entity holds each document node now, and which node each despawned
//! entity held.
//!
//! A history entry names the entities it changed, and a respawn (a snapshot
//! undo, a tab switch, a prefab reload) mints new entities for the same nodes.
//! [`live_entity`] finds the entity that holds a recorded entity's node now.

use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use jackdaw_bsn::{BsnPatch, BsnTupleStructData, BsnValue, SceneBsnAst};
use jackdaw_scene_types::{SCENE_NODE_ID_TYPE_PATH, SceneNodeId};

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<SceneNodes>()
        .add_observer(index_node)
        .add_observer(retire_node);
}

/// The live entity of every document node, and the node each despawned scene
/// entity held.
///
/// The nodes inside a prefab instance carry the ids the prefab file gave them,
/// so two instances of one prefab share them. A node held by more than one
/// entity resolves to none.
#[derive(Resource, Default)]
pub struct SceneNodes {
    live: HashMap<SceneNodeId, Entity>,
    shared: HashMap<SceneNodeId, Vec<Entity>>,
    retired: HashMap<Entity, SceneNodeId>,
}

impl SceneNodes {
    /// The one entity holding `node` now.
    pub fn entity(&self, node: SceneNodeId) -> Option<Entity> {
        if self.shared.contains_key(&node) {
            return None;
        }
        self.live.get(&node).copied()
    }

    fn hold(&mut self, node: SceneNodeId, entity: Entity) {
        let first = *self.live.entry(node).or_insert(entity);
        if first == entity {
            return;
        }
        let holders = self.shared.entry(node).or_insert_with(|| vec![first]);
        if !holders.contains(&entity) {
            holders.push(entity);
        }
    }

    fn release(&mut self, node: SceneNodeId, entity: Entity) {
        let Some(holders) = self.shared.get_mut(&node) else {
            if self.live.get(&node) == Some(&entity) {
                self.live.remove(&node);
            }
            return;
        };
        holders.retain(|&held| held != entity);
        let remaining = holders.first().copied();
        if holders.len() <= 1 {
            self.shared.remove(&node);
        }
        match remaining {
            Some(held) => self.live.insert(node, held),
            None => self.live.remove(&node),
        };
    }
}

fn index_node(
    added: On<Insert, SceneNodeId>,
    ids: Query<&SceneNodeId>,
    mut nodes: ResMut<SceneNodes>,
) {
    let entity = added.event_target();
    if let Ok(&node) = ids.get(entity) {
        nodes.hold(node, entity);
    }
}

fn retire_node(
    discarded: On<Discard, SceneNodeId>,
    ids: Query<&SceneNodeId>,
    mut nodes: ResMut<SceneNodes>,
) {
    let entity = discarded.event_target();
    let Ok(&node) = ids.get(entity) else {
        return;
    };
    nodes.release(node, entity);
    nodes.retired.insert(entity, node);
}

/// The entity that holds what `entity` held when it was recorded: `entity`
/// while it is alive, otherwise the entity now holding its document node.
/// A dead entity whose node is gone comes back unchanged.
pub fn live_entity(world: &World, entity: Entity) -> Entity {
    if world.get_entity(entity).is_ok() {
        return entity;
    }
    world
        .get_resource::<SceneNodes>()
        .and_then(|nodes| {
            let node = nodes.retired.get(&entity)?;
            nodes.entity(*node)
        })
        .unwrap_or(entity)
}

/// The document patch that names a node `node`.
pub fn node_id_patch(node: SceneNodeId) -> BsnPatch {
    BsnPatch::TupleStruct(BsnTupleStructData {
        type_path: SCENE_NODE_ID_TYPE_PATH.to_string(),
        values: vec![BsnValue::Int(i128::from(node.0))],
    })
}

/// Make `entity` hold `node`, in the document as well as in the world. Does
/// nothing to an entity that is gone or holds no document node.
pub fn rename_node(world: &mut World, entity: Entity, node: SceneNodeId) {
    if world
        .get::<SceneNodeId>(entity)
        .is_none_or(|held| *held == node)
    {
        return;
    }
    let mut ast = world.resource_mut::<SceneBsnAst>();
    let Some(document_node) = ast.ast_for(entity) else {
        return;
    };
    match ast.find_patch_by_type_path(document_node, SCENE_NODE_ID_TYPE_PATH) {
        Some(patch) => ast.set_patch(patch, node_id_patch(node)),
        None => {
            let patch = ast.world.spawn(node_id_patch(node)).id();
            if let Some(patches) = ast.get_patches_mut(document_node) {
                patches.0.push(patch);
            }
        }
    }
    world.entity_mut(entity).insert(node);
}

/// Give every prefab instance root in `ast` that has no node id a fresh one.
///
/// The nodes inside an instance take their ids from the prefab file; the root
/// is the scene's own node, and a scene saved before roots were named has none.
pub fn name_instance_roots(ast: &mut SceneBsnAst) {
    let mut stack = ast.roots.clone();
    let mut seen = HashSet::new();
    let mut unnamed = Vec::new();
    while let Some(node) = stack.pop() {
        if !seen.insert(node) {
            continue;
        }
        if ast
            .find_patch_by_type_path(node, crate::prefab::resolver_bsn::ISA_TYPE)
            .is_some()
            && ast.stable_id_of(node).is_none()
        {
            unnamed.push(node);
        }
        stack.extend(ast.get_children_ast(node));
    }
    for node in unnamed {
        let patch = ast.world.spawn(node_id_patch(SceneNodeId::next())).id();
        if let Some(patches) = ast.get_patches_mut(node) {
            patches.0.push(patch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let mut world = World::new();
        world.init_resource::<SceneNodes>();
        world.add_observer(index_node);
        world.add_observer(retire_node);
        world
    }

    #[test]
    fn a_despawned_entity_resolves_to_the_entity_holding_its_node_now() {
        let mut world = world();
        let node = SceneNodeId::next();
        let first = world.spawn(node).id();
        world.despawn(first);
        let second = world.spawn(node).id();
        world.despawn(second);
        let third = world.spawn(node).id();

        assert_eq!(live_entity(&world, first), third);
        assert_eq!(live_entity(&world, second), third);
        assert_eq!(live_entity(&world, third), third);
    }

    #[test]
    fn a_node_held_by_two_entities_resolves_to_neither_until_one_goes() {
        let mut world = world();
        let node = SceneNodeId::next();
        let recorded = world.spawn(node).id();
        world.despawn(recorded);
        let one = world.spawn(node).id();
        let other = world.spawn(node).id();

        assert_eq!(live_entity(&world, recorded), recorded);
        world.despawn(one);
        assert_eq!(live_entity(&world, recorded), other);
    }

    #[test]
    fn a_dead_entity_whose_node_is_gone_comes_back_unchanged() {
        let mut world = world();
        let entity = world.spawn(SceneNodeId::next()).id();
        world.despawn(entity);
        assert_eq!(live_entity(&world, entity), entity);
    }
}
