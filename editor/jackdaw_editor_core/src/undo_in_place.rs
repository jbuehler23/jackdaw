//! Putting a document edit back by touching only the nodes it changed, so
//! every other entity keeps its id.
//!
//! A node whose own patches changed has them replaced and its components
//! rebuilt from the document; a prefab instance that changed is spawned again
//! from its overrides; a node that only moved is moved. An edit this cannot
//! carry out (an embedded asset changed, a reference the scene cannot
//! resolve) is refused before anything is touched, and the caller respawns
//! the scene instead.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use jackdaw_bsn::{BsnPatch, BsnSceneAssets, SceneBsnAst};
use jackdaw_scene_types::SceneNodeId;

use crate::commands::{HierarchyLocation, WorldTransform, place_entity};
use crate::document_units::{Side, Unit, UnitChange, UnitKey};
use crate::scene_nodes::SceneNodes;

/// Make the nodes `changes` name read as their `side`, or refuse with the
/// reason when that cannot be done in place.
pub(crate) fn put_back(
    world: &mut World,
    changes: &[UnitChange],
    side: Side,
) -> Result<(), String> {
    let steps = plan(world, changes, side)?;
    carry_out(world, steps);
    Ok(())
}

/// One node an edit changed.
struct Step<'a> {
    node: SceneNodeId,
    from: Option<&'a Unit>,
    to: Option<&'a Unit>,
    content: Option<Content>,
}

/// What a changed node becomes.
enum Content {
    /// The node's own patches; its children are units of their own.
    Own(Vec<BsnPatch>),
    /// A prefab instance, resolved, with the root to spawn.
    Whole(Box<SceneBsnAst>, Entity),
}

fn plan<'a>(world: &World, changes: &'a [UnitChange], side: Side) -> Result<Vec<Step<'a>>, String> {
    let other = match side {
        Side::Before => Side::After,
        Side::After => Side::Before,
    };
    let resolvable = world.get_resource::<BsnSceneAssets>();
    changes
        .iter()
        .map(|change| {
            let UnitKey::Node(id) = change.key else {
                return Err("an embedded asset changed".to_string());
            };
            let from = change.side(other);
            let to = change.side(side);
            let rewritten = to.filter(|to| {
                from.is_none_or(|from| from.text != to.text || from.whole != to.whole)
            });
            let content = rewritten
                .map(|unit| content_of(world, unit, resolvable))
                .transpose()?;
            Ok(Step {
                node: SceneNodeId(id),
                from,
                to,
                content,
            })
        })
        .collect()
}

fn content_of(
    world: &World,
    unit: &Unit,
    resolvable: Option<&BsnSceneAssets>,
) -> Result<Content, String> {
    if let Some(name) = named_references(&unit.text)
        .find(|name| resolvable.is_none_or(|assets| !assets.0.contains_key(*name)))
    {
        return Err(format!("{name} is not an asset the scene holds"));
    }
    let parsed = match unit.text.is_empty() {
        true => SceneBsnAst::default(),
        false => jackdaw_bsn::parse_bsn_text(&unit.text).map_err(|err| err.to_string())?,
    };
    let root = parsed.roots.first().copied();
    if !unit.whole {
        return Ok(Content::Own(
            root.map(|root| parsed.cloned_component_patches(root))
                .unwrap_or_default(),
        ));
    }
    let resolved = match world.get_resource::<crate::prefab::PrefabAstCache>() {
        Some(cache) => crate::prefab::resolver_bsn::resolve_scene(&parsed, &|path| cache.get(path))
            .map_err(|err| err.to_string())?,
        None => parsed,
    };
    let root = resolved
        .roots
        .first()
        .copied()
        .ok_or("a prefab instance resolved to nothing")?;
    Ok(Content::Whole(Box::new(resolved), root))
}

/// The `#Name` and `@Name` asset references in `text`.
fn named_references(text: &str) -> impl Iterator<Item = &str> {
    text.match_indices('"').filter_map(|(at, _)| {
        let rest = &text[at + 1..];
        let end = rest.find('"')?;
        let quoted = &rest[..end];
        (quoted.starts_with('#') || quoted.starts_with('@')).then_some(quoted)
    })
}

fn carry_out(world: &mut World, steps: Vec<Step>) {
    let mut placed: HashMap<SceneNodeId, Entity> = HashMap::new();
    let mut pending: Vec<Step> = Vec::new();
    let mut removing: Vec<SceneNodeId> = Vec::new();
    for step in steps {
        match step.to {
            Some(_) => pending.push(step),
            None => removing.push(step.node),
        }
    }
    let moving: Vec<SceneNodeId> = pending.iter().map(|step| step.node).collect();
    let waits_on = |key: &Option<UnitKey>, placed: &HashMap<SceneNodeId, Entity>| match key {
        Some(UnitKey::Node(id)) => {
            let node = SceneNodeId(*id);
            moving.contains(&node) && !placed.contains_key(&node)
        }
        _ => false,
    };
    while !pending.is_empty() {
        let (ready, waiting): (Vec<Step>, Vec<Step>) = pending.into_iter().partition(|step| {
            let to = step.to.expect("pending steps have a target");
            !waits_on(&to.parent, &placed) && !waits_on(&to.follows, &placed)
        });
        if ready.is_empty() {
            warn!(
                "undo: {} nodes wait on one another and were left out",
                waiting.len()
            );
            break;
        }
        for step in ready {
            let node = step.node;
            match carry_out_step(world, &placed, step) {
                Some(entity) => {
                    placed.insert(node, entity);
                }
                None => warn!("undo: node {node:?} has no parent to go back under"),
            }
        }
        pending = waiting;
    }
    for node in removing {
        let Some(entity) = live(world, &placed, node) else {
            continue;
        };
        crate::commands::deselect_entities(world, &[entity]);
        crate::commands::despawn_scene_entity(world, entity);
    }
}

/// Put one node where its step says, returning it.
fn carry_out_step(
    world: &mut World,
    placed: &HashMap<SceneNodeId, Entity>,
    step: Step,
) -> Option<Entity> {
    let to = step.to?;
    let parent = match &to.parent {
        Some(UnitKey::Node(id)) => Some(live(world, placed, SceneNodeId(*id))?),
        _ => None,
    };
    let follows = match &to.follows {
        Some(UnitKey::Node(id)) => live(world, placed, SceneNodeId(*id)),
        _ => None,
    };
    let now = live(world, placed, step.node);
    let (entity, spawned) = match (now, step.content) {
        (Some(entity), Some(Content::Own(patches))) => {
            replace_own_patches(world, entity, patches);
            (entity, false)
        }
        (Some(entity), None) => (entity, false),
        (now, Some(Content::Whole(resolved, root))) => {
            if let Some(entity) = now {
                crate::commands::despawn_scene_entity(world, entity);
            }
            (spawn_subtree(world, &resolved, root, parent)?, true)
        }
        (None, Some(Content::Own(patches))) => {
            let mut document = SceneBsnAst::default();
            let root = document.create_entity_node(patches);
            (spawn_subtree(world, &document, root, parent)?, true)
        }
        (None, None) => return None,
    };
    let moved = step
        .from
        .is_none_or(|from| from.parent != to.parent || from.follows != to.follows);
    if spawned || moved {
        let location = HierarchyLocation {
            parent,
            index: index_after(world, entity, parent, follows),
        };
        place_entity(world, entity, location, WorldTransform::Unplaced);
        crate::hierarchy::sync_outliner_row_order(world, parent);
    }
    Some(entity)
}

/// The entity holding `node`: the one this edit just placed, or the live one.
fn live(world: &World, placed: &HashMap<SceneNodeId, Entity>, node: SceneNodeId) -> Option<Entity> {
    placed
        .get(&node)
        .copied()
        .or_else(|| world.get_resource::<SceneNodes>()?.entity(node))
}

/// Swap `entity`'s own patches in the document for `patches`, keeping its
/// children, and rebuild its components from them.
fn replace_own_patches(world: &mut World, entity: Entity, patches: Vec<BsnPatch>) {
    {
        let mut live = world.resource_mut::<SceneBsnAst>();
        let Some(node) = live.ast_for(entity) else {
            return;
        };
        let old: Vec<Entity> = live
            .get_patches(node)
            .map(|list| list.0.clone())
            .unwrap_or_default();
        let (children, replaced): (Vec<Entity>, Vec<Entity>) = old
            .into_iter()
            .partition(|&patch| matches!(live.get_patch(patch), Some(BsnPatch::Children(_))));
        let mut list: Vec<Entity> = patches
            .into_iter()
            .map(|patch| live.world.spawn(patch).id())
            .collect();
        list.extend(children);
        if let Some(patches) = live.get_patches_mut(node) {
            patches.0 = list;
        }
        for patch in replaced {
            live.world.despawn(patch);
        }
    }
    crate::scene_io::resync_entity_from_ast(world, entity);
}

/// Graft `root` of `document` into the live document under `parent` and spawn
/// it, returning the entity spawned for the root.
fn spawn_subtree(
    world: &mut World,
    document: &SceneBsnAst,
    root: Entity,
    parent: Option<Entity>,
) -> Option<Entity> {
    let node = {
        let mut live = world.resource_mut::<SceneBsnAst>();
        let parent_node = parent.and_then(|parent| live.ast_for(parent));
        jackdaw_bsn::clone_subtree_into(&mut live, document, root, parent_node)
    };
    let mut spawned = Vec::new();
    jackdaw_bsn::spawn_ast_node(world, node, parent, &mut spawned);
    jackdaw_bsn::apply_dirty_ast_patches(world);
    spawned.first().copied()
}

/// The sibling index just after `follows` under `parent`, counted without
/// `entity` itself.
fn index_after(
    world: &World,
    entity: Entity,
    parent: Option<Entity>,
    follows: Option<Entity>,
) -> usize {
    let Some(follows) = follows else {
        return 0;
    };
    let position = match parent {
        Some(parent) => world.get::<Children>(parent).and_then(|children| {
            children
                .iter()
                .filter(|&child| child != entity)
                .position(|child| child == follows)
        }),
        None => {
            let live = world.resource::<SceneBsnAst>();
            let own = live.ast_for(entity);
            let follows = live.ast_for(follows);
            live.roots
                .iter()
                .filter(|&&root| Some(root) != own)
                .position(|&root| Some(root) == follows)
        }
    };
    position.map_or(0, |index| index + 1)
}
