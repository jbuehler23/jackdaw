//! What changed in the shape of the scene document between two moments: which nodes
//! a running game must spawn, despawn or move to match it.
//!
//! A prefab instance is one node here, as it is one unit of the history: what sits
//! below its root takes ids from its file, repeated by every other instance of it.
//! An instance whose inside changed shape spawns again from its overrides.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use jackdaw_bsn::SceneBsnAst;

use crate::document_units::is_instance;

/// The shape of a document, by stable node id.
#[derive(Default)]
pub(crate) struct Tree {
    places: HashMap<u64, Place>,
    /// The children of each node in document order, and under `None` the roots.
    children: HashMap<Option<u64>, Vec<u64>>,
}

struct Place {
    parent: Option<u64>,
    /// A digest of what sits below an instance root, zero for any other node.
    inner: u64,
}

/// A node of a [`Tree`], as reached walking the document parents first.
pub(crate) struct WalkedNode {
    pub(crate) id: u64,
    node: Entity,
    parent: Option<u64>,
    follows: Option<u64>,
}

/// One thing a game does to match the document. Nodes appear parents first, with
/// each sibling after those it follows and the despawns last, so a child moved out
/// of a removed parent is already safe and a parent is there when a child arrives.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StructureStep {
    /// `at` is the document node. Its text holds the new nodes below it.
    Spawn {
        node: u64,
        at: Entity,
        parent: Option<u64>,
        follows: Option<u64>,
    },
    Reparent {
        node: u64,
        parent: Option<u64>,
        follows: Option<u64>,
    },
    Despawn {
        node: u64,
    },
}

impl Tree {
    /// The shape of `ast`, and its nodes parents first. A node without an id is left
    /// out with what is below it, and so is a second node with an id already seen.
    pub(crate) fn of(ast: &SceneBsnAst) -> (Self, Vec<WalkedNode>) {
        let _span = info_span!("play.structure_walk").entered();
        let mut tree = Self::default();
        let mut walked = Vec::new();
        let mut claimed: HashSet<u64> = HashSet::new();
        let mut pending: Vec<WalkedNode> = Vec::new();
        tree.queue_siblings(ast, &ast.roots, None, &mut claimed, &mut pending);
        while let Some(next) = pending.pop() {
            let instance = is_instance(ast, next.node);
            tree.places.insert(
                next.id,
                Place {
                    parent: next.parent,
                    inner: if instance {
                        inner_digest(ast, next.node)
                    } else {
                        0
                    },
                },
            );
            if !instance {
                let children = ast.get_children_ast(next.node);
                tree.queue_siblings(ast, &children, Some(next.id), &mut claimed, &mut pending);
            }
            walked.push(next);
        }
        (tree, walked)
    }

    fn queue_siblings(
        &mut self,
        ast: &SceneBsnAst,
        siblings: &[Entity],
        parent: Option<u64>,
        claimed: &mut HashSet<u64>,
        pending: &mut Vec<WalkedNode>,
    ) {
        let mut ids = Vec::with_capacity(siblings.len());
        let start = pending.len();
        for &node in siblings {
            let Some(id) = ast.stable_id_of(node).filter(|&id| claimed.insert(id)) else {
                continue;
            };
            pending.push(WalkedNode {
                id,
                node,
                parent,
                follows: ids.last().copied(),
            });
            ids.push(id);
        }
        pending[start..].reverse();
        self.children.insert(parent, ids);
    }

    pub(crate) fn holds(&self, id: u64) -> bool {
        self.places.contains_key(&id)
    }

    /// What a game holding this shape does to reach `after`, of which `walked` are
    /// the nodes parents first.
    pub(crate) fn steps_to(&self, after: &Tree, walked: &[WalkedNode]) -> Vec<StructureStep> {
        let reordered = self.reordered_in(after);
        let mut steps = Vec::new();
        for next in walked {
            let inner = after.places.get(&next.id).map_or(0, |place| place.inner);
            let spawn = StructureStep::Spawn {
                node: next.id,
                at: next.node,
                parent: next.parent,
                follows: next.follows,
            };
            let Some(place) = self.places.get(&next.id) else {
                let in_parents_text = next.parent.is_some_and(|parent| !self.holds(parent));
                if !in_parents_text {
                    steps.push(spawn);
                }
                continue;
            };
            if place.inner != inner {
                steps.push(StructureStep::Despawn { node: next.id });
                steps.push(spawn);
            } else if place.parent != next.parent || reordered.contains(&next.id) {
                steps.push(StructureStep::Reparent {
                    node: next.id,
                    parent: next.parent,
                    follows: next.follows,
                });
            }
        }
        let mut gone: Vec<u64> = self
            .places
            .iter()
            .filter(|(id, place)| {
                !after.holds(**id) && place.parent.is_none_or(|parent| after.holds(parent))
            })
            .map(|(id, _)| *id)
            .collect();
        gone.sort_unstable();
        steps.extend(gone.into_iter().map(|node| StructureStep::Despawn { node }));
        steps
    }

    /// The nodes that must move for those staying under a parent to read in their
    /// new order: all but the longest run already in it. The rest stay put, and each
    /// that moves lands straight after its new predecessor, parents and earlier
    /// siblings first, so the list ends in document order.
    fn reordered_in(&self, after: &Tree) -> HashSet<u64> {
        let mut reordered = HashSet::new();
        for (parent, now) in &after.children {
            let Some(was) = self.children.get(parent).filter(|was| *was != now) else {
                continue;
            };
            let was_at: HashMap<u64, usize> =
                was.iter().enumerate().map(|(at, id)| (*id, at)).collect();
            let staying: Vec<(u64, usize)> = now
                .iter()
                .filter_map(|id| Some((*id, *was_at.get(id)?)))
                .collect();
            reordered.extend(out_of_order(&staying));
        }
        reordered
    }
}

/// The ids of `staying`, in new order with their old places, outside a longest run
/// of rising old places.
fn out_of_order(staying: &[(u64, usize)]) -> Vec<u64> {
    let mut run_ends: Vec<usize> = Vec::new();
    let mut before: Vec<Option<usize>> = Vec::with_capacity(staying.len());
    for (at, (_, place)) in staying.iter().enumerate() {
        let length = run_ends.partition_point(|&end| staying[end].1 < *place);
        before.push(length.checked_sub(1).map(|shorter| run_ends[shorter]));
        match run_ends.get_mut(length) {
            Some(end) => *end = at,
            None => run_ends.push(at),
        }
    }
    let mut in_run = vec![false; staying.len()];
    let mut at = run_ends.last().copied();
    while let Some(held) = at {
        in_run[held] = true;
        at = before[held];
    }
    staying
        .iter()
        .zip(in_run)
        .filter(|(_, in_run)| !in_run)
        .map(|((id, _), _)| *id)
        .collect()
}

fn inner_digest(ast: &SceneBsnAst, instance: Entity) -> u64 {
    let mut hasher = DefaultHasher::new();
    let mut pending = ast.get_children_ast(instance);
    while let Some(node) = pending.pop() {
        let children = ast.get_children_ast(node);
        (ast.stable_id_of(node), children.len()).hash(&mut hasher);
        pending.extend(children);
    }
    hasher.finish() | 1
}

#[cfg(test)]
mod tests {
    use jackdaw_bsn::BsnPatch;
    use jackdaw_scene_types::SceneNodeId;

    use super::*;

    fn node(ast: &mut SceneBsnAst, id: u64, parent: Option<Entity>) -> Entity {
        let patch = crate::scene_nodes::node_id_patch(SceneNodeId(id));
        let node = ast.create_entity_node(vec![patch]);
        match parent {
            Some(parent) => ast.add_child_to_ast(parent, node),
            None => ast.add_to_roots(node),
        }
        node
    }

    fn steps(before: &SceneBsnAst, after: &SceneBsnAst) -> Vec<StructureStep> {
        let (before, _) = Tree::of(before);
        let (tree, walked) = Tree::of(after);
        before.steps_to(&tree, &walked)
    }

    fn reparent(node: u64, parent: Option<u64>, follows: Option<u64>) -> StructureStep {
        StructureStep::Reparent {
            node,
            parent,
            follows,
        }
    }

    #[test]
    fn an_insertion_moves_no_sibling() {
        let mut before = SceneBsnAst::default();
        node(&mut before, 1, None);
        node(&mut before, 2, None);
        let mut after = SceneBsnAst::default();
        node(&mut after, 1, None);
        let new = node(&mut after, 3, None);
        node(&mut after, 2, None);
        assert_eq!(
            steps(&before, &after),
            vec![StructureStep::Spawn {
                node: 3,
                at: new,
                parent: None,
                follows: Some(1),
            }]
        );
    }

    #[test]
    fn a_reorder_moves_only_what_changed_place() {
        let mut before = SceneBsnAst::default();
        for id in [1, 2, 3] {
            node(&mut before, id, None);
        }
        let mut after = SceneBsnAst::default();
        for id in [3, 1, 2] {
            node(&mut after, id, None);
        }
        assert_eq!(steps(&before, &after), vec![reparent(3, None, None)]);
    }

    /// The steps played on the old list, each straight after what it follows.
    fn played(start: &[u64], steps: &[StructureStep]) -> Vec<u64> {
        let mut list = start.to_vec();
        for step in steps {
            let StructureStep::Reparent { node, follows, .. } = step else {
                panic!("only moves here: {step:?}");
            };
            list.retain(|held| held != node);
            let at = follows.map_or(0, |follows| {
                list.iter()
                    .position(|held| *held == follows)
                    .expect("placed")
                    + 1
            });
            list.insert(at, *node);
        }
        list
    }

    fn roots_of(ids: &[u64]) -> SceneBsnAst {
        let mut ast = SceneBsnAst::default();
        for &id in ids {
            node(&mut ast, id, None);
        }
        ast
    }

    #[test]
    fn a_block_of_siblings_moved_ends_in_document_order() {
        for (from, to) in [
            (vec![1, 2, 3, 4], vec![3, 4, 1, 2]),
            (vec![1, 2, 3, 4, 5], vec![5, 4, 3, 2, 1]),
            (vec![1, 2, 3, 4, 5, 6], vec![2, 3, 1, 6, 4, 5]),
        ] {
            let moves = steps(&roots_of(&from), &roots_of(&to));
            assert_eq!(played(&from, &moves), to, "{moves:?}");
        }
        let moves = steps(&roots_of(&[1, 2, 3, 4]), &roots_of(&[3, 4, 1, 2]));
        assert_eq!(moves.len(), 2, "only as many moves as it takes: {moves:?}");
    }

    /// One add among many siblings must not cost the square of them. The margin is
    /// wide: the quadratic walk takes many times this.
    #[test]
    fn one_add_among_a_flat_document_is_linear() {
        let ids: Vec<u64> = (1..=100_000).collect();
        let before = roots_of(&ids);
        let mut after = roots_of(&ids);
        node(&mut after, 200_000, None);
        let (tree, _) = Tree::of(&before);
        let (next, walked) = Tree::of(&after);
        let start = std::time::Instant::now();
        assert_eq!(tree.steps_to(&next, &walked).len(), 1);
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_swap_of_parent_and_child_moves_the_upper_one_first() {
        let mut before = SceneBsnAst::default();
        let top = node(&mut before, 1, None);
        node(&mut before, 2, Some(top));
        let mut after = SceneBsnAst::default();
        let top = node(&mut after, 2, None);
        node(&mut after, 1, Some(top));
        assert_eq!(
            steps(&before, &after),
            vec![reparent(2, None, None), reparent(1, Some(2), None)]
        );
    }

    #[test]
    fn a_removed_subtree_is_one_despawn_after_what_left_it() {
        let mut before = SceneBsnAst::default();
        let top = node(&mut before, 1, None);
        let middle = node(&mut before, 2, Some(top));
        node(&mut before, 3, Some(middle));
        node(&mut before, 4, Some(middle));
        let mut after = SceneBsnAst::default();
        node(&mut after, 4, None);
        assert_eq!(
            steps(&before, &after),
            vec![reparent(4, None, None), StructureStep::Despawn { node: 1 },]
        );
    }

    #[test]
    fn an_instance_is_one_node_whatever_ids_repeat_below_it() {
        let instance = |ast: &mut SceneBsnAst, id: u64, inner: &[u64]| {
            let root = node(ast, id, None);
            let isa = ast
                .world
                .spawn(BsnPatch::Type(
                    crate::prefab::resolver_bsn::ISA_TYPE.to_string(),
                ))
                .id();
            ast.get_patches_mut(root).expect("patches").0.push(isa);
            for &id in inner {
                node(ast, id, Some(root));
            }
            root
        };
        let mut before = SceneBsnAst::default();
        instance(&mut before, 1, &[7]);
        instance(&mut before, 2, &[7]);
        let (tree, _) = Tree::of(&before);
        assert!(!tree.holds(7), "inner nodes are the instance's");

        let mut after = SceneBsnAst::default();
        instance(&mut after, 1, &[7]);
        let changed = instance(&mut after, 2, &[7, 8]);
        assert_eq!(
            steps(&before, &after),
            vec![
                StructureStep::Despawn { node: 2 },
                StructureStep::Spawn {
                    node: 2,
                    at: changed,
                    parent: None,
                    follows: Some(1),
                },
            ]
        );
    }
}
