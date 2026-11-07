//! Applies the editor's structural document changes to the running game: a node
//! added, removed or moved reaches every live entity spawned from it.
//!
//! A node's entities are found through [`SceneNodeIndex`]. An entity below a
//! prefab instance's root takes its id from the prefab file, so it can repeat the
//! id of a node of the scene and never counts as that node. With a scene spawned
//! twice, a parent and a child pair up within one root.
//!
//! While a scene root has yet to spawn, a change is also held and replayed once the
//! root has spawned, ahead of the held component edits, in the order it arrived.
//! The document the root spawned from may hold the change already, so each change
//! leaves what is already in place alone.

use std::path::Path;

use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use jackdaw_scene_types::{SceneNodeId, UiSceneRoot};

use crate::node_sync::{PendingNodeChanges, SceneNodeIndex};
use crate::{
    JackdawSceneMember, JackdawSceneRoot, SceneInstanceMembers, SceneSpawned,
    scene_root_awaiting_spawn,
};

/// A structural change of the document a game plays.
#[derive(Clone, Debug)]
pub enum StructureChange {
    /// Spawn `bsn`, the subtree of `node`, under every entity of `parent`, or for
    /// `None` under every root playing the asset path `scene`, after the sibling
    /// `follows`. `dir` is the folder the text counts as being in.
    Spawn {
        node: u64,
        parent: Option<u64>,
        follows: Option<u64>,
        scene: String,
        dir: String,
        bsn: String,
    },
    /// Despawn every entity of `node`, with everything below it.
    Despawn { node: u64 },
    /// Move every entity of `node` under `parent`, or for `None` to the top of its
    /// scene, after the sibling `follows`.
    Reparent {
        node: u64,
        parent: Option<u64>,
        follows: Option<u64>,
    },
}

/// The asset path a root asked the editor for, which outlives its handle being
/// sent to another file.
#[derive(Component)]
pub(crate) struct PlayedAs(pub(crate) String);

#[derive(Resource, Default)]
pub(crate) struct HeldStructure(Vec<Held>);

/// A change that arrived while a root had yet to spawn.
struct Held {
    change: StructureChange,
    /// The roots that have taken it.
    delivered: HashSet<Entity>,
}

/// Apply `change` to what has spawned, and hold it for any root yet to spawn.
pub fn apply_structure_change(world: &mut World, change: StructureChange) {
    apply_within(world, &change, None);
    if scene_root_awaiting_spawn(world) {
        let delivered = spawned_roots(world).into_iter().collect();
        world
            .get_resource_or_init::<HeldStructure>()
            .0
            .push(Held { change, delivered });
    }
}

/// Replay held changes for the roots that spawned since they were held, and drop
/// them once no root is waiting.
pub(crate) fn replay_held_structure(world: &mut World) {
    if world.resource::<HeldStructure>().0.is_empty() {
        return;
    }
    let mut held = std::mem::take(&mut world.resource_mut::<HeldStructure>().0);
    let spawned = spawned_roots(world);
    for held in &mut held {
        for &root in &spawned {
            if held.delivered.insert(root) {
                apply_within(world, &held.change, Some(root));
            }
        }
    }
    if scene_root_awaiting_spawn(world) {
        world.resource_mut::<HeldStructure>().0 = held;
    }
}

/// Apply `change`, only inside the root `only` if given.
fn apply_within(world: &mut World, change: &StructureChange, only: Option<Entity>) {
    match change {
        StructureChange::Spawn {
            node,
            parent,
            follows,
            scene,
            dir,
            bsn,
        } => spawn_node(
            world,
            &Spawning {
                node: *node,
                parent: *parent,
                follows: *follows,
                scene,
                dir,
                bsn,
            },
            only,
        ),
        StructureChange::Despawn { node } => despawn_node(world, *node, only),
        StructureChange::Reparent {
            node,
            parent,
            follows,
        } => reparent_node(world, *node, *parent, *follows, only),
    }
}

struct Spawning<'a> {
    node: u64,
    parent: Option<u64>,
    follows: Option<u64>,
    scene: &'a str,
    dir: &'a str,
    bsn: &'a str,
}

fn spawn_node(world: &mut World, spawn: &Spawning, only: Option<Entity>) {
    let parsed = match jackdaw_bsn::parse_bsn_text(spawn.bsn) {
        Ok(parsed) => parsed,
        Err(err) => {
            warn!(
                "PIE: could not read the node {} the editor sent: {err}",
                spawn.node
            );
            return;
        }
    };
    let dir = Path::new(spawn.dir);
    let (ast, expanded) = crate::resolve_prefab_references(world, parsed, "a live edit", dir);
    if only.is_none() {
        forget_older_edits(world, &ast);
    }
    if let Err(err) = jackdaw_bsn::reject_retired_ui_components(&ast) {
        warn!("PIE: cannot spawn the node {}: {err}", spawn.node);
        return;
    }
    let targets: Vec<(Entity, Option<Entity>)> = match spawn.parent {
        None => roots_playing(world, spawn.scene)
            .into_iter()
            .map(|root| (root, None))
            .collect(),
        Some(parent) => {
            let holders = unit_holders(world, parent);
            if holders.is_empty() && !scene_root_awaiting_spawn(world) {
                warn!(
                    "PIE: the node {} goes under {parent}, which this game does not have",
                    spawn.node
                );
            }
            holders
                .into_iter()
                .map(|holder| (instance_of(world, holder).unwrap_or(holder), Some(holder)))
                .collect()
        }
    };
    for (root, under) in targets {
        if only.is_some_and(|only| only != root)
            || unit_holders(world, spawn.node)
                .into_iter()
                .any(|held| instance_of(world, held) == Some(root))
        {
            continue;
        }
        let members = crate::spawn_document_nodes(world, root, under, &ast, dir, &expanded);
        let Some(&entity) = members.first() else {
            continue;
        };
        if under.is_none()
            && let Some(mut held) = world.get_mut::<SceneInstanceMembers>(root)
        {
            held.0.push(entity);
        }
        place_after(world, entity, spawn.follows);
    }
}

/// An edit held from before this text was written is older than it.
fn forget_older_edits(world: &mut World, ast: &jackdaw_bsn::SceneBsnAst) {
    let Some(mut pending) = world.get_resource_mut::<PendingNodeChanges>() else {
        return;
    };
    for &root in &ast.roots {
        for node in std::iter::once(root).chain(ast.descendants_of(root)) {
            if let Some(id) = ast.stable_id_of(node) {
                pending.forget(id);
            }
        }
    }
}

fn despawn_node(world: &mut World, node: u64, only: Option<Entity>) {
    for entity in unit_holders(world, node) {
        if only.is_some_and(|only| instance_of(world, entity) != Some(only)) {
            continue;
        }
        leave_scene_top(world, entity);
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }
}

fn reparent_node(
    world: &mut World,
    node: u64,
    parent: Option<u64>,
    follows: Option<u64>,
    only: Option<Entity>,
) {
    for entity in unit_holders(world, node) {
        let root = instance_of(world, entity);
        if only.is_some_and(|only| root != Some(only)) {
            continue;
        }
        match parent {
            Some(parent) => {
                let Some(target) = parent_in_instance(world, parent, root) else {
                    if !scene_root_awaiting_spawn(world) {
                        warn!(
                            "PIE: the node {node} moves under {parent}, which this game does not \
                             have, so it stays where it is"
                        );
                    }
                    continue;
                };
                if is_at_or_below(world, target, entity) {
                    warn!("PIE: the node {node} cannot move below itself");
                    continue;
                }
                leave_scene_top(world, entity);
                world.entity_mut(entity).insert(ChildOf(target));
            }
            None => {
                let Some(root) = root else {
                    continue;
                };
                move_to_scene_top(world, entity, root);
            }
        }
        place_after(world, entity, follows);
    }
}

/// The entity of `parent` to move a child of the root `root` under: the one in that
/// root, or the only one there is.
fn parent_in_instance(world: &World, parent: u64, root: Option<Entity>) -> Option<Entity> {
    let holders = unit_holders(world, parent);
    holders
        .iter()
        .copied()
        .find(|&holder| instance_of(world, holder) == root)
        .or(match holders.as_slice() {
            [only] => Some(*only),
            _ => None,
        })
}

fn is_at_or_below(world: &World, entity: Entity, ancestor: Entity) -> bool {
    let mut at = entity;
    loop {
        if at == ancestor {
            return true;
        }
        let Some(child_of) = world.get::<ChildOf>(at) else {
            return false;
        };
        at = child_of.parent();
    }
}

/// `entity` is no longer a top-level entity of its scene.
fn leave_scene_top(world: &mut World, entity: Entity) {
    let Some(&JackdawSceneMember { root }) = world.get::<JackdawSceneMember>(entity) else {
        return;
    };
    world.entity_mut(entity).remove::<JackdawSceneMember>();
    if let Some(mut members) = world.get_mut::<SceneInstanceMembers>(root) {
        members.0.retain(|&member| member != entity);
    }
}

fn move_to_scene_top(world: &mut World, entity: Entity, root: Entity) {
    if world.get::<JackdawSceneMember>(entity).is_none() {
        world.entity_mut(entity).insert(JackdawSceneMember { root });
        if let Some(mut members) = world.get_mut::<SceneInstanceMembers>(root) {
            members.0.push(entity);
        }
    }
    if world.get::<UiSceneRoot>(entity).is_some() {
        world.entity_mut(entity).remove::<ChildOf>();
    } else {
        world.entity_mut(entity).insert(ChildOf(root));
    }
}

/// Put `entity` straight after the child of its parent that holds `follows`, or
/// first for `None`. A sibling the parent does not have leaves it where it is.
fn place_after(world: &mut World, entity: Entity, follows: Option<u64>) {
    let Some(parent) = world.get::<ChildOf>(entity).map(ChildOf::parent) else {
        return;
    };
    let Some(children) = world.get::<Children>(parent) else {
        return;
    };
    let holds_follows = |child: &Entity, follows: u64| {
        world
            .get::<SceneNodeId>(*child)
            .is_some_and(|held| held.0 == follows)
    };
    let Some(at) = children.iter().rposition(|child| child == entity) else {
        return;
    };
    let index = match follows {
        None if at == 0 => return,
        None => 0,
        Some(follows) if at > 0 && holds_follows(&children[at - 1], follows) => return,
        Some(follows) => {
            let Some(sibling) = children.iter().position(|c| holds_follows(&c, follows)) else {
                return;
            };
            if sibling < at { sibling + 1 } else { sibling }
        }
    };
    world.entity_mut(parent).insert_child(index, entity);
}

/// The entities holding `node` as a node of the scene rather than as part of a
/// prefab instance.
fn unit_holders(world: &World, node: u64) -> Vec<Entity> {
    let Some(index) = world.get_resource::<SceneNodeIndex>() else {
        return Vec::new();
    };
    index
        .entities(SceneNodeId(node))
        .iter()
        .copied()
        .filter(|&entity| !below_instance_root(world, entity))
        .collect()
}

fn below_instance_root(world: &World, entity: Entity) -> bool {
    let mut at = world.get::<ChildOf>(entity);
    while let Some(child_of) = at {
        let parent = child_of.parent();
        if world.get::<jackdaw_prefab::IsA>(parent).is_some() {
            return true;
        }
        at = world.get::<ChildOf>(parent);
    }
    false
}

/// The scene root `entity` belongs to.
fn instance_of(world: &World, entity: Entity) -> Option<Entity> {
    let mut at = entity;
    loop {
        if world.get::<JackdawSceneRoot>(at).is_some() {
            return Some(at);
        }
        if let Some(member) = world.get::<JackdawSceneMember>(at) {
            return Some(member.root);
        }
        at = world.get::<ChildOf>(at)?.parent();
    }
}

fn spawned_roots(world: &mut World) -> Vec<Entity> {
    world
        .query_filtered::<Entity, (With<JackdawSceneRoot>, With<SceneSpawned>)>()
        .iter(world)
        .collect()
}

/// The spawned roots playing the asset path `scene`.
fn roots_playing(world: &mut World, scene: &str) -> Vec<Entity> {
    world
        .query_filtered::<(Entity, &JackdawSceneRoot, Option<&PlayedAs>), With<SceneSpawned>>()
        .iter(world)
        .filter(|(_, root, played)| {
            let path = match played {
                Some(played) => Some(played.0.clone()),
                None => root
                    .0
                    .path()
                    .map(|path| path.path().to_string_lossy().into_owned()),
            };
            path.is_some_and(|path| path.replace('\\', "/") == scene)
        })
        .map(|(entity, ..)| entity)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::{JackdawPlugin, JackdawScene};

    const TRANSFORM: &str = "bevy_transform::components::transform::Transform";

    fn node_text(node: u64) -> String {
        format!("jackdaw_scene_types::node_id::SceneNodeId({node})\n{TRANSFORM}\n")
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::transform::TransformPlugin);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
        crate::node_sync::install_node_index(&mut app);
        app.add_plugins(JackdawPlugin);
        app.register_type::<Name>();
        app
    }

    /// A root spawned from `text`, playing the asset path `scene`.
    fn root_with(app: &mut App, scene: &str, text: &str) -> Entity {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<JackdawScene>>()
            .add(JackdawScene::new(text.to_string(), PathBuf::new()));
        let root = app
            .world_mut()
            .spawn((JackdawSceneRoot(handle), PlayedAs(scene.to_string())))
            .id();
        app.update();
        root
    }

    fn spawn_change(node: u64, parent: Option<u64>, follows: Option<u64>) -> StructureChange {
        StructureChange::Spawn {
            node,
            parent,
            follows,
            scene: "level.bsn".to_string(),
            dir: String::new(),
            bsn: node_text(node),
        }
    }

    fn holders(app: &mut App, node: u64) -> Vec<Entity> {
        let world = app.world_mut();
        world
            .query::<(Entity, &SceneNodeId)>()
            .iter(world)
            .filter(|(_, held)| held.0 == node)
            .map(|(entity, _)| entity)
            .collect()
    }

    fn parent_of(app: &App, entity: Entity) -> Option<Entity> {
        app.world().get::<ChildOf>(entity).map(ChildOf::parent)
    }

    #[test]
    fn a_change_reaches_both_roots_of_a_scene_spawned_twice_and_pairs_within_each() {
        let mut app = app();
        let first = root_with(&mut app, "level.bsn", &node_text(1));
        let second = root_with(&mut app, "level.bsn", &node_text(1));

        apply_structure_change(app.world_mut(), spawn_change(2, None, None));
        let added = holders(&mut app, 2);
        assert_eq!(added.len(), 2, "one under each root");

        apply_structure_change(
            app.world_mut(),
            StructureChange::Reparent {
                node: 2,
                parent: Some(1),
                follows: None,
            },
        );
        for &entity in &added {
            let parent = parent_of(&app, entity).expect("a parent");
            let root = parent_of(&app, parent).expect("the node's root");
            assert!(root == first || root == second);
            assert_eq!(
                app.world().get::<SceneNodeId>(parent),
                Some(&SceneNodeId(1))
            );
            assert!(
                added
                    .iter()
                    .filter(|&&other| other != entity)
                    .all(|&other| parent_of(&app, other) != Some(parent)),
                "each stays inside its own root"
            );
        }

        apply_structure_change(app.world_mut(), StructureChange::Despawn { node: 1 });
        assert!(holders(&mut app, 1).is_empty());
        assert!(holders(&mut app, 2).is_empty());
        app.update();
    }

    #[test]
    fn a_scene_that_is_not_the_one_named_takes_no_spawn() {
        let mut app = app();
        let other = root_with(&mut app, "other.bsn", &node_text(1));
        apply_structure_change(app.world_mut(), spawn_change(2, None, None));
        assert!(holders(&mut app, 2).is_empty());
        assert_eq!(holders(&mut app, 1).len(), 1);
        assert!(app.world().get::<SceneSpawned>(other).is_some());
    }

    #[test]
    fn a_parent_the_game_does_not_have_leaves_everything_as_it_is() {
        let mut app = app();
        let root = root_with(&mut app, "level.bsn", &node_text(1));
        apply_structure_change(app.world_mut(), spawn_change(2, Some(99), None));
        assert!(holders(&mut app, 2).is_empty());

        apply_structure_change(
            app.world_mut(),
            StructureChange::Reparent {
                node: 1,
                parent: Some(99),
                follows: None,
            },
        );
        let entity = holders(&mut app, 1)[0];
        assert_eq!(parent_of(&app, entity), Some(root));
    }

    #[test]
    fn a_node_id_repeated_below_an_instance_root_is_not_that_node() {
        let mut app = app();
        root_with(&mut app, "level.bsn", &node_text(1));
        let unit = holders(&mut app, 1)[0];
        let instance = app
            .world_mut()
            .spawn(jackdaw_prefab::IsA {
                source: PathBuf::from("rock.bsn"),
                deleted: Vec::new(),
            })
            .id();
        let inner = app
            .world_mut()
            .spawn((SceneNodeId(1), ChildOf(instance)))
            .id();
        app.update();

        apply_structure_change(app.world_mut(), spawn_change(2, Some(1), None));
        let added = holders(&mut app, 2);
        assert_eq!(added.len(), 1, "only the node of the scene takes the child");
        assert_eq!(parent_of(&app, added[0]), Some(unit));

        apply_structure_change(app.world_mut(), StructureChange::Despawn { node: 1 });
        assert!(app.world().get_entity(unit).is_err());
        assert!(
            app.world().get_entity(inner).is_ok(),
            "the instance keeps it"
        );
    }

    #[test]
    fn siblings_land_in_document_order_wherever_they_are_added() {
        let mut app = app();
        let root = root_with(&mut app, "level.bsn", &node_text(1));
        apply_structure_change(app.world_mut(), spawn_change(3, None, Some(1)));
        apply_structure_change(app.world_mut(), spawn_change(2, None, Some(1)));
        apply_structure_change(app.world_mut(), spawn_change(4, None, None));
        let order = |app: &App| -> Vec<u64> {
            app.world()
                .get::<Children>(root)
                .expect("children")
                .iter()
                .filter_map(|child| app.world().get::<SceneNodeId>(child))
                .map(|node| node.0)
                .collect()
        };
        assert_eq!(order(&app), vec![4, 1, 2, 3]);

        apply_structure_change(
            app.world_mut(),
            StructureChange::Reparent {
                node: 4,
                parent: None,
                follows: Some(3),
            },
        );
        assert_eq!(order(&app), vec![1, 2, 3, 4]);
    }

    #[test]
    fn a_root_on_top_of_its_scene_counts_as_a_member_once_it_moves_there() {
        let mut app = app();
        let root = root_with(&mut app, "level.bsn", &node_text(1));
        apply_structure_change(app.world_mut(), spawn_change(2, Some(1), None));
        let child = holders(&mut app, 2)[0];
        assert!(app.world().get::<JackdawSceneMember>(child).is_none());

        apply_structure_change(
            app.world_mut(),
            StructureChange::Reparent {
                node: 2,
                parent: None,
                follows: None,
            },
        );
        assert_eq!(
            app.world().get::<JackdawSceneMember>(child),
            Some(&JackdawSceneMember { root })
        );
        assert!(
            app.world()
                .get::<SceneInstanceMembers>(root)
                .expect("members")
                .0
                .contains(&child)
        );

        apply_structure_change(app.world_mut(), StructureChange::Despawn { node: 2 });
        assert!(
            !app.world()
                .get::<SceneInstanceMembers>(root)
                .expect("members")
                .0
                .contains(&child)
        );
    }
}
