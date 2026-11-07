//! Which entity holds each document node now, and which node each despawned
//! entity held.
//!
//! A history entry names the entities it changed, and a respawn (a snapshot
//! undo, a tab switch, a prefab reload) mints new entities for the same nodes.
//! [`live_entity`] finds the entity that holds a recorded entity's node now.

use std::collections::VecDeque;

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
/// so two instances of one prefab share them, and a file an older pack wrote can
/// repeat the id of a node in the scene.
#[derive(Resource, Default)]
pub struct SceneNodes {
    live: HashMap<SceneNodeId, Entity>,
    shared: HashMap<SceneNodeId, Vec<Entity>>,
    retired: HashMap<Entity, SceneNodeId>,
    /// `retired` in the order entities died, so the oldest go first once it
    /// outgrows [`Self::retired_cap`].
    retired_order: VecDeque<Entity>,
}

impl SceneNodes {
    /// The entity holding `node` as a unit of its own in the document, however
    /// many entities below instance roots repeat the id. An instance's inner
    /// node is never returned, even when it is the only holder.
    pub(crate) fn unit(&self, world: &World, node: SceneNodeId) -> Option<Entity> {
        self.holders(node).find(|&entity| is_unit(world, entity))
    }

    fn only_holder(&self, node: SceneNodeId) -> Option<Entity> {
        if self.shared.contains_key(&node) {
            return None;
        }
        self.live.get(&node).copied()
    }

    fn holders(&self, node: SceneNodeId) -> impl Iterator<Item = Entity> + '_ {
        let held = match self.shared.get(&node) {
            Some(holders) => holders.as_slice(),
            None => self.live.get(&node).map_or(&[][..], std::slice::from_ref),
        };
        held.iter().copied()
    }

    /// How many despawned entities are remembered: a few times the live
    /// nodes, so a history entry recorded a few respawns ago still finds its
    /// node, with a floor for small scenes.
    fn retired_cap(&self) -> usize {
        (4 * self.live.len()).max(65_536)
    }

    fn retire(&mut self, node: SceneNodeId, entity: Entity) {
        if self.retired.insert(entity, node).is_none() {
            self.retired_order.push_back(entity);
        }
        while self.retired_order.len() > self.retired_cap() {
            if let Some(oldest) = self.retired_order.pop_front() {
                self.retired.remove(&oldest);
            }
        }
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

/// Whether `entity` is a node of the document that no instance root sits above.
/// Everything below an instance root belongs to that instance's unit, whether its
/// file or the scene added it.
fn is_unit(world: &World, entity: Entity) -> bool {
    world
        .get_resource::<SceneBsnAst>()
        .is_some_and(|document| document.ast_for(entity).is_some())
        && !below_instance_root(world, entity)
}

fn below_instance_root(world: &World, entity: Entity) -> bool {
    let mut at = world.get::<ChildOf>(entity);
    while let Some(child_of) = at {
        let parent = child_of.parent();
        if world.get::<crate::prefab::IsA>(parent).is_some() {
            return true;
        }
        at = world.get::<ChildOf>(parent);
    }
    false
}

/// The entity `node` means to someone holding only the id: the unit holding it,
/// else the only holder, so one instance's inner node still resolves after a
/// respawn. Inner nodes that several instances share resolve to none.
pub fn unit_holder(world: &World, node: SceneNodeId) -> Option<Entity> {
    let nodes = world.get_resource::<SceneNodes>()?;
    nodes.unit(world, node).or_else(|| nodes.only_holder(node))
}

fn index_node(
    added: On<Insert<SceneNodeId>>,
    ids: Query<&SceneNodeId>,
    mut nodes: ResMut<SceneNodes>,
) {
    let entity = added.event_target();
    if let Ok(&node) = ids.get(entity) {
        nodes.hold(node, entity);
    }
}

fn retire_node(
    discarded: On<Discard<SceneNodeId>>,
    ids: Query<&SceneNodeId>,
    mut nodes: ResMut<SceneNodes>,
) {
    let entity = discarded.event_target();
    let Ok(&node) = ids.get(entity) else {
        return;
    };
    nodes.release(node, entity);
    nodes.retire(node, entity);
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
            unit_holder(world, *node)
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
    match ast.stable_id_patch(document_node) {
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

/// One entity node of a document, as a walk from the roots finds it.
struct WalkedNode {
    node: Entity,
    instance_root: bool,
    /// Below an instance root and numbered by its file, so the file names it.
    /// A child the scene added below an instance is not inherited.
    inherited: bool,
}

/// Every entity node of `ast`, in document order. Named asset roots spawn no
/// entity and are left out.
fn walk_nodes(ast: &SceneBsnAst, registry: &bevy::reflect::TypeRegistry) -> Vec<WalkedNode> {
    let mut stack: Vec<(Entity, bool)> = jackdaw_bsn::catalog::entity_roots(ast, registry)
        .into_iter()
        .rev()
        .map(|root| (root, false))
        .collect();
    let mut seen = HashSet::new();
    let mut walked = Vec::new();
    while let Some((node, below)) = stack.pop() {
        if !seen.insert(node) {
            continue;
        }
        let instance_root = ast
            .find_patch_by_type_path(node, crate::prefab::resolver_bsn::ISA_TYPE)
            .is_some();
        let inherited = below
            && ast
                .find_patch_by_type_path(node, crate::prefab::resolver_bsn::PREFAB_ENTITY_ID_TYPE)
                .is_some();
        walked.push(WalkedNode {
            node,
            instance_root,
            inherited,
        });
        let children_below = below || instance_root;
        stack.extend(
            ast.get_children_ast(node)
                .into_iter()
                .rev()
                .map(|child| (child, children_below)),
        );
    }
    walked
}

fn give_fresh_ids(ast: &mut SceneBsnAst, repeating: &[Entity]) {
    if repeating.is_empty() {
        return;
    }
    warn!(
        "{} scene nodes repeated a node id and got fresh ones",
        repeating.len()
    );
    for &node in repeating {
        if let Some(patch) = ast.stable_id_patch(node) {
            ast.set_patch(patch, node_id_patch(SceneNodeId::next()));
        }
    }
}

/// Give every entity node in `ast` that the scene owns and that has no node id
/// a fresh one, so a hand-written or older scene names each node the way the
/// editor and a running game address it. A node repeating an id the scene
/// already gave another of its own nodes gets a fresh one too, keeping both
/// nodes; the first in the document keeps the id. Named asset roots spawn no
/// entity and are left alone.
///
/// The nodes inside a prefab instance carry a `PrefabEntityId` and take their
/// ids from the prefab file, so they are left as they are. An instance root
/// carries one too, but it is the scene's own node.
pub fn name_scene_nodes(ast: &mut SceneBsnAst, registry: &bevy::reflect::TypeRegistry) {
    let mut ids = HashSet::new();
    let mut unnamed = Vec::new();
    let mut repeating = Vec::new();
    for walked in walk_nodes(ast, registry) {
        match ast.stable_id_of(walked.node) {
            None if walked.instance_root || !walked.inherited => unnamed.push(walked.node),
            Some(id) if !walked.inherited && !ids.insert(id) => repeating.push(walked.node),
            _ => {}
        }
    }
    for node in unnamed {
        let patch = ast.world.spawn(node_id_patch(SceneNodeId::next())).id();
        if let Some(patches) = ast.get_patches_mut(node) {
            patches.0.push(patch);
        }
    }
    give_fresh_ids(ast, &repeating);
}

/// Give a fresh id to every node the scene owns in `resolved` that repeats an id
/// an instance inherited from its file, which only shows once the instances are
/// expanded. The inherited node keeps the id, since the file says so.
///
/// Renames what a history entry would name, so a caller runs this only on a
/// document with no history.
pub fn separate_from_inherited_ids(
    resolved: &mut SceneBsnAst,
    registry: &bevy::reflect::TypeRegistry,
) {
    let walked = walk_nodes(resolved, registry);
    let inherited: HashSet<u64> = walked
        .iter()
        .filter(|walked| walked.inherited)
        .filter_map(|walked| resolved.stable_id_of(walked.node))
        .collect();
    let repeating: Vec<Entity> = walked
        .iter()
        .filter(|walked| !walked.inherited)
        .filter(|walked| {
            resolved
                .stable_id_of(walked.node)
                .is_some_and(|id| inherited.contains(&id))
        })
        .map(|walked| walked.node)
        .collect();
    give_fresh_ids(resolved, &repeating);
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
    fn the_oldest_despawned_entities_are_forgotten_first() {
        let mut nodes = SceneNodes::default();
        let cap = nodes.retired_cap();
        let mut world = World::new();
        let dead: Vec<Entity> = (0..=cap).map(|_| world.spawn_empty().id()).collect();
        for &entity in &dead {
            nodes.retire(SceneNodeId::next(), entity);
        }
        assert_eq!(nodes.retired.len(), cap);
        assert!(!nodes.retired.contains_key(&dead[0]));
        assert!(nodes.retired.contains_key(&dead[cap]));
    }

    #[test]
    fn a_dead_entity_whose_node_is_gone_comes_back_unchanged() {
        let mut world = world();
        let entity = world.spawn(SceneNodeId::next()).id();
        world.despawn(entity);
        assert_eq!(live_entity(&world, entity), entity);
    }
}
