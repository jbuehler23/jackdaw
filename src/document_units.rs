//! A scene document cut into units that keep their identity across respawns,
//! so an undo entry holds only the units an edit changed.
//!
//! A unit is a scene node named by its
//! [`SceneNodeId`](jackdaw_scene_types::SceneNodeId), holding its own patches,
//! or a whole prefab instance (its overrides, the way the document stores it),
//! or a named asset the document embeds. Each unit records its parent and the
//! sibling it follows, so a move or an insertion changes only the units next
//! to it.

use bevy::ecs::entity::Entity;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::reflect::TypeRegistry;
use jackdaw_bsn::{BsnPatch, SceneBsnAst};

use crate::prefab::resolver_bsn::ISA_TYPE;

/// What names a unit across respawns.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum UnitKey {
    Node(u64),
    Asset(String),
}

/// One unit as the document held it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Unit {
    pub(crate) parent: Option<UnitKey>,
    pub(crate) follows: Option<UnitKey>,
    /// Whether `text` holds the unit's whole subtree rather than the node's
    /// own patches.
    pub(crate) whole: bool,
    pub(crate) text: Box<str>,
}

/// Every unit of a document.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Units(HashMap<UnitKey, Unit>);

/// A unit before and after an edit; `None` when it did not exist on that side.
#[derive(Clone, Debug)]
pub(crate) struct UnitChange {
    pub(crate) key: UnitKey,
    pub(crate) before: Option<Unit>,
    pub(crate) after: Option<Unit>,
}

/// Which side of a change to put back.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Before,
    After,
}

impl Units {
    /// Cut `ast` into units, or `None` when a node has no id or shares one,
    /// or an asset has no name.
    pub(crate) fn split(ast: &SceneBsnAst, registry: &TypeRegistry) -> Option<Self> {
        let assets: HashSet<Entity> = jackdaw_bsn::asset_roots(ast, registry)
            .into_iter()
            .collect();
        let mut units = HashMap::new();
        let mut pending: Vec<(Entity, Option<UnitKey>, Option<UnitKey>)> = Vec::new();
        push_siblings(&mut pending, &ast.roots, None, |node| {
            key_of(ast, &assets, node)
        })?;
        while let Some((node, parent, follows)) = pending.pop() {
            let key = key_of(ast, &assets, node)?;
            let whole = assets.contains(&node) || is_instance(ast, node);
            let text = match whole {
                true => jackdaw_bsn::emit_entity(ast, node),
                false => jackdaw_bsn::emit_entity_without_children(ast, node),
            };
            if !whole {
                push_siblings(
                    &mut pending,
                    &ast.get_children_ast(node),
                    Some(key.clone()),
                    |child| key_of(ast, &assets, child),
                )?;
            }
            let unit = Unit {
                parent,
                follows,
                whole,
                text: text.into_boxed_str(),
            };
            if units.insert(key, unit).is_some() {
                return None;
            }
        }
        Some(Self(units))
    }

    /// The units that differ between `self` and `after`.
    pub(crate) fn changes_to(&self, after: &Units) -> Vec<UnitChange> {
        let mut changes: Vec<UnitChange> = self
            .0
            .iter()
            .filter(|(key, unit)| after.0.get(*key) != Some(*unit))
            .map(|(key, unit)| UnitChange {
                key: key.clone(),
                before: Some(unit.clone()),
                after: after.0.get(key).cloned(),
            })
            .collect();
        changes.extend(
            after
                .0
                .iter()
                .filter(|(key, _)| !self.0.contains_key(*key))
                .map(|(key, unit)| UnitChange {
                    key: key.clone(),
                    before: None,
                    after: Some(unit.clone()),
                }),
        );
        changes
    }

    /// A document holding every unit.
    pub(crate) fn rebuild(&self) -> Result<SceneBsnAst, String> {
        let mut ast = SceneBsnAst::default();
        let changes: Vec<UnitChange> = self
            .0
            .iter()
            .map(|(key, unit)| UnitChange {
                key: key.clone(),
                before: None,
                after: Some(unit.clone()),
            })
            .collect();
        apply_changes(&mut ast, Located::default(), &changes, Side::After)?;
        Ok(ast)
    }

    /// Bytes the unit texts hold.
    pub(crate) fn heap_bytes(&self) -> usize {
        self.0.values().map(Unit::heap_bytes).sum()
    }
}

impl Unit {
    fn heap_bytes(&self) -> usize {
        self.text.len() + std::mem::size_of::<Self>()
    }
}

impl UnitChange {
    pub(crate) fn heap_bytes(&self) -> usize {
        self.before.as_ref().map_or(0, Unit::heap_bytes)
            + self.after.as_ref().map_or(0, Unit::heap_bytes)
    }

    pub(crate) fn side(&self, side: Side) -> Option<&Unit> {
        match side {
            Side::Before => self.before.as_ref(),
            Side::After => self.after.as_ref(),
        }
    }
}

/// Queue `siblings` under `parent`, each following the one before it.
fn push_siblings(
    pending: &mut Vec<(Entity, Option<UnitKey>, Option<UnitKey>)>,
    siblings: &[Entity],
    parent: Option<UnitKey>,
    key_of: impl Fn(Entity) -> Option<UnitKey>,
) -> Option<()> {
    let mut follows = None;
    for &sibling in siblings {
        let key = key_of(sibling)?;
        pending.push((sibling, parent.clone(), follows.replace(key)));
    }
    Some(())
}

fn key_of(ast: &SceneBsnAst, assets: &HashSet<Entity>, node: Entity) -> Option<UnitKey> {
    match assets.contains(&node) {
        true => ast
            .get_name(node)
            .map(|name| UnitKey::Asset(name.to_string())),
        false => ast.stable_id_of(node).map(UnitKey::Node),
    }
}

fn is_instance(ast: &SceneBsnAst, node: Entity) -> bool {
    ast.find_patch_by_type_path(node, ISA_TYPE).is_some()
}

/// The node each unit of a document is, and the parent of each.
#[derive(Clone, Default)]
pub(crate) struct Located {
    nodes: HashMap<UnitKey, Entity>,
    parents: HashMap<Entity, Entity>,
}

impl Located {
    pub(crate) fn find(ast: &SceneBsnAst, registry: &TypeRegistry) -> Self {
        let assets: HashSet<Entity> = jackdaw_bsn::asset_roots(ast, registry)
            .into_iter()
            .collect();
        let mut located = Self::default();
        let mut stack: Vec<Entity> = ast.roots.clone();
        while let Some(node) = stack.pop() {
            let Some(key) = key_of(ast, &assets, node) else {
                continue;
            };
            located.nodes.insert(key, node);
            if assets.contains(&node) || is_instance(ast, node) {
                continue;
            }
            for child in ast.get_children_ast(node) {
                located.parents.insert(child, node);
                stack.push(child);
            }
        }
        located
    }
}

/// Make the units `changes` name read as their `side` in `ast`, whose units
/// sit where `located` says. Units the changes do not name stay as they are.
pub(crate) fn apply_changes(
    ast: &mut SceneBsnAst,
    located: Located,
    changes: &[UnitChange],
    side: Side,
) -> Result<(), String> {
    let Located {
        nodes: mut located,
        parents,
    } = located;
    for change in changes {
        if let Some(&node) = located.get(&change.key) {
            detach(ast, &parents, node);
        }
    }
    let mut placing: Vec<(&UnitKey, &Unit)> = Vec::new();
    for change in changes {
        let Some(unit) = change.side(side) else {
            located.remove(&change.key);
            continue;
        };
        let node = build(ast, located.get(&change.key).copied(), unit)?;
        located.insert(change.key.clone(), node);
        placing.push((&change.key, unit));
    }
    let moving: HashSet<&UnitKey> = placing.iter().map(|(key, _)| *key).collect();
    let mut placed: HashSet<&UnitKey> = HashSet::new();
    while !placing.is_empty() {
        let ready: Vec<(&UnitKey, &Unit)> = placing
            .iter()
            .copied()
            .filter(|(_, unit)| {
                [&unit.parent, &unit.follows].into_iter().all(|key| {
                    key.as_ref()
                        .is_none_or(|key| !moving.contains(key) || placed.contains(key))
                })
            })
            .collect();
        if ready.is_empty() {
            return Err(format!(
                "{} document units follow one another in a loop",
                placing.len()
            ));
        }
        for (key, unit) in ready {
            let parent = resolve(&located, unit.parent.as_ref())?;
            let follows = resolve(&located, unit.follows.as_ref())?;
            place(ast, located[key], parent, follows);
            placed.insert(key);
        }
        placing.retain(|(key, _)| !placed.contains(key));
    }
    Ok(())
}

/// The node `key` names, if any; an error when it names a unit that is gone.
fn resolve(
    located: &HashMap<UnitKey, Entity>,
    key: Option<&UnitKey>,
) -> Result<Option<Entity>, String> {
    key.map(|key| {
        located
            .get(key)
            .copied()
            .ok_or_else(|| format!("a document unit names {key:?}, which is not there"))
    })
    .transpose()
}

/// The node `unit` describes: `existing` with its own patches replaced, or a
/// new node.
fn build(ast: &mut SceneBsnAst, existing: Option<Entity>, unit: &Unit) -> Result<Entity, String> {
    let parsed = match unit.text.is_empty() {
        true => SceneBsnAst::default(),
        false => jackdaw_bsn::parse_bsn_text(&unit.text).map_err(|err| err.to_string())?,
    };
    let root = parsed.roots.first().copied();
    if unit.whole {
        let root = root.ok_or("a document unit holds no node")?;
        let node = jackdaw_bsn::clone_subtree_into(ast, &parsed, root, None);
        ast.remove_from_roots(node);
        return Ok(node);
    }
    let own: Vec<BsnPatch> = root
        .map(|root| parsed.cloned_component_patches(root))
        .unwrap_or_default();
    let Some(node) = existing else {
        return Ok(ast.create_entity_node(own));
    };
    let children: Vec<Entity> = ast
        .get_patches(node)
        .map(|patches| {
            patches
                .0
                .iter()
                .copied()
                .filter(|&patch| matches!(ast.get_patch(patch), Some(BsnPatch::Children(_))))
                .collect()
        })
        .unwrap_or_default();
    let mut patches: Vec<Entity> = own
        .into_iter()
        .map(|patch| ast.world.spawn(patch).id())
        .collect();
    patches.extend(children);
    if let Some(list) = ast.get_patches_mut(node) {
        list.0 = patches;
    }
    Ok(node)
}

/// Take `node` out of its parent's children or the roots, dropping a
/// children list it leaves empty.
fn detach(ast: &mut SceneBsnAst, parents: &HashMap<Entity, Entity>, node: Entity) {
    let Some(&parent) = parents.get(&node) else {
        ast.remove_from_roots(node);
        return;
    };
    ast.remove_child_from_ast(parent, node);
    let emptied: Vec<Entity> = ast
        .get_patches(parent)
        .map(|patches| {
            patches
                .0
                .iter()
                .copied()
                .filter(|&patch| {
                    matches!(ast.get_patch(patch), Some(BsnPatch::Children(children)) if children.is_empty())
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(patches) = ast.get_patches_mut(parent) {
        patches.0.retain(|patch| !emptied.contains(patch));
    }
}

/// Put `node` under `parent`, or among the roots, just after `follows`.
fn place(ast: &mut SceneBsnAst, node: Entity, parent: Option<Entity>, follows: Option<Entity>) {
    match parent {
        None => {
            let at = follows
                .and_then(|sibling| ast.roots.iter().position(|&root| root == sibling))
                .map_or(0, |index| index + 1);
            ast.roots.insert(at.min(ast.roots.len()), node);
        }
        Some(parent) => {
            let at = follows
                .and_then(|sibling| {
                    ast.get_children_ast(parent)
                        .iter()
                        .position(|&child| child == sibling)
                })
                .map_or(0, |index| index + 1);
            ast.insert_child_in_ast(parent, node, at);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64, name: &str, x: f32, children: &[String]) -> String {
        let mut text = format!(
            "#{name}\njackdaw_scene_types::node_id::SceneNodeId({id})\nbevy_transform::components::transform::Transform {{\n    translation: glam::Vec3 {{ x: {x:?}, y: 0.0, z: 0.0 }},\n}}\n"
        );
        if !children.is_empty() {
            text.push_str("bevy_ecs::hierarchy::Children [\n");
            text.push_str(&children.join(",\n"));
            text.push_str("]\n");
        }
        text
    }

    fn scene(roots: &[String]) -> SceneBsnAst {
        let text = format!("bevy_ecs::hierarchy::Children [\n{}]\n", roots.join(",\n"));
        jackdaw_bsn::parse_bsn_text(&text)
            .expect("the scene parses")
            .deep_clone()
    }

    fn before() -> SceneBsnAst {
        scene(&[
            node(1, "Ground", 0.0, &[node(11, "Rock", 1.0, &[])]),
            node(2, "Tree", 2.0, &[]),
            node(3, "Shed", 3.0, &[]),
        ])
    }

    fn after() -> SceneBsnAst {
        scene(&[
            node(
                1,
                "Ground",
                0.0,
                &[node(11, "Rock", 1.0, &[]), node(2, "Tree", 5.0, &[])],
            ),
            node(4, "Well", 4.0, &[]),
        ])
    }

    fn units(ast: &SceneBsnAst) -> Units {
        Units::split(ast, &TypeRegistry::new()).expect("every node has an id")
    }

    fn applied(mut ast: SceneBsnAst, changes: &[UnitChange], side: Side) -> String {
        let located = Located::find(&ast, &TypeRegistry::new());
        apply_changes(&mut ast, located, changes, side).expect("the changes apply");
        jackdaw_bsn::emit_scene(&ast)
    }

    #[test]
    fn an_edit_keeps_only_the_units_it_changed() {
        let changes = units(&before()).changes_to(&units(&after()));
        let mut keys: Vec<&UnitKey> = changes.iter().map(|change| &change.key).collect();
        keys.sort_by_key(|key| format!("{key:?}"));
        assert_eq!(
            keys,
            [&UnitKey::Node(2), &UnitKey::Node(3), &UnitKey::Node(4)]
        );
    }

    #[test]
    fn changes_put_either_side_of_an_edit_back_exactly() {
        let changes = units(&before()).changes_to(&units(&after()));
        assert_eq!(
            applied(before(), &changes, Side::After),
            jackdaw_bsn::emit_scene(&after())
        );
        assert_eq!(
            applied(after(), &changes, Side::Before),
            jackdaw_bsn::emit_scene(&before())
        );
    }

    #[test]
    fn units_rebuild_the_document_they_were_cut_from() {
        let rebuilt = units(&after()).rebuild().expect("the units rebuild");
        assert_eq!(
            jackdaw_bsn::emit_scene(&rebuilt),
            jackdaw_bsn::emit_scene(&after())
        );
    }

    #[test]
    fn a_node_without_an_id_leaves_the_document_whole() {
        let ast = scene(&["#Loose\n".to_string(), node(1, "Ground", 0.0, &[])]);
        assert!(Units::split(&ast, &TypeRegistry::new()).is_none());
    }
}
