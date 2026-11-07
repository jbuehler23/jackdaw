//! Pushes the scene document's component changes to the running game.
//!
//! While play is running, each frame the document changed finds the nodes
//! whose patches changed (the document world's change ticks, or a full pass
//! when the document was replaced) and compares each node's components
//! against what was last seen. Every component whose patch differs goes to
//! every live instance as its full current value in a
//! [`ControlEvent::NodeComponents`], together with the components the node
//! no longer has. A component that leaves the node's patches but stays on
//! the editor entity (one the entity derives, such as a required component)
//! goes as its current value instead of a removal. Commands, undo, redo and
//! remote edits all change the document, so no command needs to report its
//! own edit.
//!
//! A node added, removed or moved reaches a game playing the document as a
//! [`ControlEvent::SpawnNode`], [`ControlEvent::DespawnNode`] or
//! [`ControlEvent::ReparentNode`], worked out in `play_structure`. They go ahead of
//! the frame's component edits, which never cross them in the outbox. Each frame
//! the document's structure changed costs one walk of its tree.
//!
//! An edit naming a runtime asset a game has not been given goes behind a
//! [`ControlEvent::SceneAssets`] holding the asset's definition, the way served
//! text embeds it, and one naming an asset file behind a
//! [`ControlEvent::AssetFiles`]. Both go through the instance's one queue, in that
//! order. A changed material goes on its own, under the path or session name a game
//! holds it by, and the game writes it into the asset it holds.

use std::any::TypeId;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bevy::asset::{ReflectAsset, ReflectHandle, UntypedAssetId, UntypedHandle};
use bevy::ecs::change_detection::Tick;
use bevy::ecs::message::{MessageCursor, Messages};
use bevy::ecs::reflect::ReflectComponent;
use bevy::ecs::world::WorldId;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::serde::{ReflectSerializerProcessor, TypedReflectSerializer};
use bevy::reflect::{PartialReflect, ReflectRef, TypePath, TypeRegistry};
use jackdaw_api::pie::PlayState;
use jackdaw_bsn::{BsnPatch, BsnPatches, CatalogAssetRef, SceneBsnAst, patch_type_path};
use jackdaw_pie_protocol::ControlEvent;
use jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH;
use serde::Serializer;

use crate::pie::{InstanceKey, SyncTarget};
use crate::pie_mirror::PieViewMode;
use crate::pie_projection::PieProjection;
use crate::play_scene::{AskedScene, AssetFile, asset_files_missing_from};
use crate::play_structure::{StructureStep, Tree};
use crate::prefab::resolver_bsn::ISA_TYPE;
use crate::project_types::ProjectTypes;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<DocumentSync>()
        .add_systems(
            Last,
            sync_document_to_game.run_if(in_state(crate::AppState::Editor)),
        )
        .add_systems(OnEnter(PlayState::Stopped), forget_on_stop);
}

/// What the game was last sent of the document, per scene node.
#[derive(Resource, Default)]
pub struct DocumentSync {
    seen: Option<Seen>,
    warned: HashSet<String>,
    material_events: MessageCursor<AssetEvent<StandardMaterial>>,
    /// An asset took a new session name, so the next collection looks at every
    /// node, whether or not the document changed.
    rescan: bool,
    /// Document nodes that were runtime entities of a game, by node. That game takes
    /// the id, and every other game takes the node as any new one.
    adoptions: HashMap<u64, Adoption>,
}

/// The game holding a runtime entity, and the entity as bits.
struct Adoption {
    game: InstanceKey,
    entity: u64,
}

impl DocumentSync {
    /// Forget what was seen, so the next collection starts from the document
    /// as it is then and sends nothing for it.
    pub fn forget(&mut self) {
        self.seen = None;
        self.rescan = false;
        self.adoptions.clear();
    }

    /// The entity `entity`, as bits, of the game `game` is the document's new node
    /// `node` from here on.
    pub(crate) fn note_adopted(&mut self, game: InstanceKey, entity: u64, node: u64) {
        self.adoptions.insert(node, Adoption { game, entity });
    }
}

struct Seen {
    document: WorldId,
    document_tick: Tick,
    world_tick: Tick,
    nodes: HashMap<u64, Components>,
    /// The tree as last sent, and the tab it belongs to.
    tree: Tree,
    tab: Option<u64>,
    roots: Vec<Entity>,
    /// Changed nodes that had no editor entity, looked at again the next time
    /// the document changes.
    retry: HashSet<u64>,
}

/// A digest of each authored component on a node, by component type path.
type Components = HashMap<String, u64>;

/// A node whose patches changed, with its components as they stand.
struct Touched {
    id: u64,
    node: Entity,
    components: Components,
}

fn forget_on_stop(mut sync: ResMut<DocumentSync>) {
    sync.forget();
}

/// A node event, and the assets its values name.
struct NodeChange {
    event: ControlEvent,
    assets: UsedAssets,
}

/// What one collection found.
#[derive(Default)]
struct Collected {
    structure: Vec<StructureStep>,
    /// The nodes that exist on both sides of `structure`.
    existing: HashSet<u64>,
    edits: Vec<NodeChange>,
}

/// The assets a value names that a game may not hold.
#[derive(Default)]
struct UsedAssets {
    /// Runtime assets, going by their session names.
    runtime: Vec<CatalogAssetRef>,
    /// Asset files, going by their paths.
    files: Vec<AssetFile>,
}

/// Send the document's node changes to every live instance. A launching
/// instance is not sent to: it asks for the document when it starts. With no
/// instance live, the first launch records the document as the baseline,
/// and with nothing live or launching (play stopped, or a launch whose build
/// failed) the baseline is dropped.
pub(crate) fn sync_document_to_game(world: &mut World) {
    let target = world
        .get_non_send::<crate::pie::PieSession>()
        .map_or(SyncTarget::None, crate::pie::PieSession::sync_target);
    match target {
        SyncTarget::None => {
            if let Some(mut sync) = world.get_resource_mut::<DocumentSync>()
                && sync.seen.is_some()
            {
                sync.forget();
            }
        }
        SyncTarget::Launching => {
            if world
                .get_resource::<DocumentSync>()
                .is_some_and(|sync| sync.seen.is_none())
            {
                collect_node_changes(world);
            }
        }
        SyncTarget::Live => {
            let keys = world.non_send::<crate::pie::PieSession>().live_keys();
            let materials = collect_material_changes(world, &keys);
            let changes = collect_changes_for(world, &keys);
            for (key, event) in materials.into_iter().chain(changes) {
                crate::pie::send_control_to_instance(world, &key, event);
            }
        }
    }
}

/// Count the document as it stands now as sent, without sending it. For a
/// write whose values came from the running game, which already holds them.
/// Does nothing while no baseline is recorded.
pub(crate) fn absorb_document_changes(world: &mut World) {
    if world
        .get_resource::<DocumentSync>()
        .is_some_and(|sync| sync.seen.is_some())
    {
        collect_node_changes(world);
    }
}

/// The node edits made to the document since the last call, one event per
/// changed node. The first call after [`DocumentSync::forget`] records the
/// document and returns nothing.
pub fn collect_node_changes(world: &mut World) -> Vec<ControlEvent> {
    let collected = collect_changes(world);
    if let Some(mut sync) = world.get_resource_mut::<DocumentSync>() {
        sync.adoptions.clear();
    }
    collected
        .edits
        .into_iter()
        .map(|change| change.event)
        .collect()
}

/// [`collect_node_changes`] as each of the games `keys` takes it, in send order:
/// an edit naming an asset a game was not given, or was given another definition
/// of, comes behind the definition. Structural changes come first, to the games
/// playing the document.
pub fn collect_changes_for(
    world: &mut World,
    keys: &[InstanceKey],
) -> Vec<(InstanceKey, ControlEvent)> {
    let collected = collect_changes(world);
    let adoptions = world
        .get_resource_mut::<DocumentSync>()
        .map(|mut sync| std::mem::take(&mut sync.adoptions))
        .unwrap_or_default();
    let mut sends = structure_sends(
        world,
        keys,
        &collected.structure,
        &collected.existing,
        &adoptions,
    );
    for change in collected.edits {
        for key in keys {
            if let Some(event) = assets_missing_from(world, key, &change.assets.runtime) {
                sends.push((key.clone(), event));
            }
            if let Some(event) = asset_files_missing_from(world, key, &change.assets.files) {
                sends.push((key.clone(), event));
            }
            sends.push((key.clone(), change.event.clone()));
        }
    }
    sends
}

/// What each of the games `keys` playing the document takes for `structure`: what
/// the new subtrees name that it does not hold, then the steps in order, once per
/// path it plays the document by. A node that was a runtime entity of a game goes to
/// that game as an adoption, once, carrying what it needs to spawn if the entity has
/// gone.
fn structure_sends(
    world: &mut World,
    keys: &[InstanceKey],
    structure: &[StructureStep],
    existing: &HashSet<u64>,
    adoptions: &HashMap<u64, Adoption>,
) -> Vec<(InstanceKey, ControlEvent)> {
    if structure.is_empty() {
        return Vec::new();
    }
    let playing: Vec<(&InstanceKey, Vec<AskedScene>)> = keys
        .iter()
        .map(|key| {
            (
                key,
                crate::play_scene::scenes_playing_active_tab(world, key),
            )
        })
        .filter(|(_, asked)| !asked.is_empty())
        .collect();
    let Some((_, first)) = playing.first() else {
        return Vec::new();
    };
    let spawns: Vec<(u64, Entity)> = structure
        .iter()
        .filter_map(|step| match step {
            StructureStep::Spawn { node, at, .. } => Some((*node, *at)),
            _ => None,
        })
        .collect();
    let subtrees = match spawns.is_empty() {
        true => None,
        false => crate::play_scene::subtrees_for_game(world, &spawns, existing, &first[0]),
    };
    let mut sends = Vec::new();
    for (key, asked) in &playing {
        let key = (*key).clone();
        if let Some(subtrees) = &subtrees {
            for event in subtree_needs_for(world, &key, subtrees) {
                sends.push((key.clone(), event));
            }
        }
        for (place, scene) in asked.iter().enumerate() {
            for step in structure {
                let event = match step {
                    StructureStep::Spawn {
                        node,
                        parent,
                        follows,
                        ..
                    } => {
                        let Some(bsn) = subtrees.as_ref().and_then(|held| held.texts.get(node))
                        else {
                            warn!("Play: the new node {node} could not be written for the game");
                            continue;
                        };
                        let adopting = adoptions
                            .get(node)
                            .filter(|adoption| place == 0 && adoption.game == key);
                        match adopting {
                            Some(adoption) => ControlEvent::AdoptNode {
                                entity: adoption.entity,
                                node: *node,
                                parent: *parent,
                                follows: *follows,
                                scene: scene.requested.clone(),
                                dir: scene.dir.clone(),
                                bsn: bsn.clone(),
                            },
                            None => ControlEvent::SpawnNode {
                                node: *node,
                                parent: *parent,
                                follows: *follows,
                                scene: scene.requested.clone(),
                                dir: scene.dir.clone(),
                                bsn: bsn.clone(),
                            },
                        }
                    }
                    StructureStep::Reparent {
                        node,
                        parent,
                        follows,
                    } => ControlEvent::ReparentNode {
                        node: *node,
                        parent: *parent,
                        follows: *follows,
                        scene: scene.requested.clone(),
                    },
                    StructureStep::Despawn { node } => ControlEvent::DespawnNode {
                        node: *node,
                        scene: scene.requested.clone(),
                    },
                };
                sends.push((key.clone(), event));
            }
        }
    }
    sends
}

/// What the game `key` must hold before it reads `subtrees`, in send order, now
/// counted as held.
fn subtree_needs_for(
    world: &mut World,
    key: &InstanceKey,
    subtrees: &crate::play_scene::SubtreesForGame,
) -> Vec<ControlEvent> {
    let mut events = Vec::new();
    events.extend(asset_files_missing_from(world, key, &subtrees.files));
    let prefabs = subtrees
        .prefabs
        .iter()
        .map(|prefab| crate::play_overlay::UnsavedPrefab {
            path: prefab.path.clone(),
            text: prefab.text.clone(),
        })
        .collect();
    events.extend(crate::play_overlay::prefab_files_missing_from(
        world,
        key,
        prefabs,
        Vec::new(),
    ));
    let mut names = world.get_resource_or_init::<crate::play_scene::PlayAssetNames>();
    for (name, definition) in &subtrees.prefab_assets {
        names.note_given(key, name.clone(), *definition);
    }
    for (name, definition, text) in &subtrees.assets {
        if !names.has_given(key, name, *definition) {
            names.note_given(key, name.clone(), *definition);
            events.push(ControlEvent::SceneAssets { text: text.clone() });
        }
    }
    events
}

/// The materials changed since the last call, as each of the games `keys` takes
/// them: covers a value set in the editor, an undo of one, and an edit to a file. A
/// game holds the definition under the path or session name, so everything using
/// the material updates with no edit to any component. A definition a game already
/// holds goes nowhere.
pub fn collect_material_changes(
    world: &mut World,
    keys: &[InstanceKey],
) -> Vec<(InstanceKey, ControlEvent)> {
    if !world.contains_resource::<DocumentSync>() {
        return Vec::new();
    }
    let changed: Vec<UntypedAssetId> =
        world.resource_scope(|world, mut sync: Mut<DocumentSync>| {
            let Some(messages) = world.get_resource::<Messages<AssetEvent<StandardMaterial>>>()
            else {
                return Vec::new();
            };
            sync.material_events
                .read(messages)
                .filter_map(|event| match event {
                    AssetEvent::Modified { id } => Some(id.untyped()),
                    _ => None,
                })
                .collect()
        });
    let paths = world.get_resource::<jackdaw_bsn::BsnAssetPaths>();
    let mut files: Vec<AssetFile> = Vec::new();
    let mut runtime: Vec<UntypedAssetId> = Vec::new();
    for id in changed {
        match paths.and_then(|paths| paths.0.get(&id)) {
            Some(path) if jackdaw_bsn::is_document_path(std::path::Path::new(path)) => {
                files.push(AssetFile {
                    path: path.clone(),
                    type_id: id.type_id(),
                    id,
                });
            }
            Some(_) => {}
            None if runtime.contains(&id) => {}
            None => runtime.push(id),
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.dedup_by(|a, b| a.path == b.path);
    let mut sends = Vec::new();
    for key in keys {
        if let Some(event) = asset_files_missing_from(world, key, &files) {
            sends.push((key.clone(), event));
        }
    }
    sends.extend(session_material_changes(world, keys, runtime));
    sends
}

/// The new definitions of `changed`, for each of the games `keys` given them under
/// a session name. An asset sharing its name with another asset keeps that one's
/// definition from changing by taking a new name, which only an edit of what holds
/// it can tell a game, so the next collection looks for those.
fn session_material_changes(
    world: &mut World,
    keys: &[InstanceKey],
    changed: Vec<UntypedAssetId>,
) -> Vec<(InstanceKey, ControlEvent)> {
    let mut sends = Vec::new();
    world.init_resource::<crate::play_scene::PlayAssetNames>();
    for id in changed {
        let type_id = id.type_id();
        let named =
            world.resource_scope(|world, mut names: Mut<crate::play_scene::PlayAssetNames>| {
                let before = names.name_of(id)?;
                Some((before, names.resolve(world, id, type_id)?))
            });
        let Some((before, name)) = named else {
            continue;
        };
        if before != name {
            world.resource_mut::<DocumentSync>().rescan = true;
            continue;
        }
        let asset = CatalogAssetRef {
            name: name.trim_start_matches('#').to_string(),
            type_id,
            asset_id: id,
        };
        for key in keys {
            let given = world
                .resource::<crate::play_scene::PlayAssetNames>()
                .was_given(key, &name);
            if given
                && let Some(event) = assets_missing_from(world, key, std::slice::from_ref(&asset))
            {
                sends.push((key.clone(), event));
            }
        }
    }
    sends
}

/// The definitions of those of `assets` the game `key` does not hold, now counted
/// as given.
fn assets_missing_from(
    world: &mut World,
    key: &InstanceKey,
    assets: &[CatalogAssetRef],
) -> Option<ControlEvent> {
    let mut names = world.get_resource_mut::<crate::play_scene::PlayAssetNames>()?;
    let mut missing: Vec<CatalogAssetRef> = Vec::new();
    for asset in assets {
        let name = format!("#{}", asset.name);
        let definition = names.definition(&name)?;
        if !names.has_given(key, &name, definition) {
            names.note_given(key, name, definition);
            missing.push(CatalogAssetRef {
                name: asset.name.clone(),
                type_id: asset.type_id,
                asset_id: asset.asset_id,
            });
        }
    }
    (!missing.is_empty()).then(|| ControlEvent::SceneAssets {
        text: jackdaw_bsn::serialize_assets_to_bsn(world, &missing),
    })
}

fn collect_changes(world: &mut World) -> Collected {
    if !world.contains_resource::<SceneBsnAst>() || !world.contains_resource::<DocumentSync>() {
        return Collected::default();
    }
    world.init_resource::<crate::play_scene::PlayAssetNames>();
    world.resource_scope(|world, mut sync: Mut<DocumentSync>| {
        world.resource_scope(|world, mut names: Mut<crate::play_scene::PlayAssetNames>| {
            collect(world, &mut sync, &mut names)
        })
    })
}

fn collect(
    world: &mut World,
    sync: &mut DocumentSync,
    names: &mut crate::play_scene::PlayAssetNames,
) -> Collected {
    let world_now = world.change_tick();
    world.increment_change_tick();
    let rescan = std::mem::take(&mut sync.rescan);
    let document_changed = rescan
        || world
            .get_resource_ref::<SceneBsnAst>()
            .is_some_and(|ast| match &sync.seen {
                Some(seen) => ast.last_changed().is_newer_than(seen.world_tick, world_now),
                None => true,
            });
    if !document_changed {
        if let Some(seen) = sync.seen.as_mut() {
            seen.world_tick = world_now;
        }
        return Collected::default();
    }

    let tab = world
        .get_resource::<crate::scenes::Scenes>()
        .and_then(|scenes| scenes.tabs.get(scenes.active))
        .map(|tab| tab.id);
    let (document, document_tick, authored, present, structure_after, roots) = {
        let mut ast = world.resource_mut::<SceneBsnAst>();
        let ast = ast.bypass_change_detection();
        let document = ast.world.id();
        let now = ast.world.change_tick();
        let since = sync
            .seen
            .as_ref()
            .filter(|seen| !rescan && seen.document == document)
            .map(|seen| (seen.document_tick, &seen.retry));
        let (authored, present, structural) = touched_nodes(ast, since, now);
        let moved = sync
            .seen
            .as_ref()
            .is_none_or(|seen| seen.tab != tab || seen.roots != ast.roots);
        let structure_after = (structural || moved).then(|| Tree::of(ast));
        let roots = ast.roots.clone();
        ast.world.increment_change_tick();
        (document, now, authored, present, structure_after, roots)
    };
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let project = world.get_resource::<ProjectTypes>();
    let mut references = PlayReferences::with(world, names);
    let touched: Vec<Touched> = authored
        .into_iter()
        .map(|(id, node, patches)| {
            let mut components = by_component(patches, &registry, project);
            let entity = world
                .resource::<SceneBsnAst>()
                .ecs_for_ast(node)
                .filter(|&entity| world.get_entity(entity).is_ok());
            if let Some(entity) = entity {
                fold_in_references(
                    world,
                    &registry,
                    &mut references,
                    names,
                    entity,
                    node,
                    &mut components,
                );
            }
            Touched {
                id,
                node,
                components,
            }
        })
        .collect();

    let Some(seen) = sync.seen.as_mut() else {
        sync.seen = Some(Seen {
            document,
            document_tick,
            world_tick: world_now,
            nodes: touched
                .into_iter()
                .map(|node| (node.id, node.components))
                .collect(),
            tree: structure_after.map(|(tree, _)| tree).unwrap_or_default(),
            tab,
            roots,
            retry: HashSet::new(),
        });
        return Collected::default();
    };
    seen.document = document;
    seen.document_tick = document_tick;
    seen.world_tick = world_now;
    seen.nodes.retain(|id, _| present.contains(id));
    seen.retry.clear();
    seen.roots = roots;
    let same_tab = seen.tab == tab;
    seen.tab = tab;
    let mut collected = Collected::default();
    if let Some((after, walked)) = structure_after {
        if same_tab {
            let steps = seen.tree.steps_to(&after, &walked);
            if steps
                .iter()
                .any(|step| matches!(step, StructureStep::Spawn { .. }))
            {
                collected.existing = walked
                    .iter()
                    .map(|node| node.id)
                    .filter(|id| seen.tree.holds(*id))
                    .collect();
            }
            collected.structure = steps;
        }
        seen.tree = after;
    }

    let projected = projected_entities(world);
    let events = &mut collected.edits;
    for Touched {
        id,
        node,
        components,
    } in touched
    {
        let Some(before) = seen.nodes.get(&id) else {
            seen.nodes.insert(id, components);
            continue;
        };
        let mut set_paths: Vec<&String> = components
            .iter()
            .filter(|(path, digest)| before.get(*path) != Some(*digest))
            .map(|(path, _)| path)
            .collect();
        let mut left: Vec<&String> = before
            .keys()
            .filter(|path| !components.contains_key(*path))
            .collect();
        if set_paths.is_empty() && left.is_empty() {
            continue;
        }
        let Some(entity) = world
            .resource::<SceneBsnAst>()
            .ecs_for_ast(node)
            .filter(|&entity| world.get_entity(entity).is_ok())
        else {
            seen.retry.insert(id);
            continue;
        };
        set_paths.sort();
        left.sort();
        let references = &mut references;
        let mut recorded = components.clone();
        let mut keep_previous = |path: &String| match before.get(path) {
            Some(digest) => {
                recorded.insert(path.clone(), *digest);
            }
            None => {
                recorded.remove(path);
            }
        };
        let mut set = Vec::new();
        let mut removed = Vec::new();
        let mut assets = UsedAssets::default();
        for path in set_paths {
            match component_value(
                world,
                &registry,
                references,
                names,
                &mut assets,
                entity,
                node,
                path,
            ) {
                Ok(value) => set.push((path.clone(), value)),
                Err(unsent) => {
                    keep_previous(path);
                    unsent.warn(&mut sync.warned, id, entity, path);
                }
            }
        }
        for path in left {
            let Some(held) = held_component(world, &registry, entity, path) else {
                removed.push(path.clone());
                continue;
            };
            if projected.contains(&entity) {
                keep_previous(path);
                seen.retry.insert(id);
                continue;
            }
            references.take_up(
                world,
                &registry,
                held.as_partial_reflect(),
                names,
                &mut assets,
            );
            match game_value(held.as_partial_reflect(), &registry, references) {
                Ok(value) => set.push((path.clone(), value)),
                Err(unsent) => {
                    keep_previous(path);
                    unsent.warn(&mut sync.warned, id, entity, path);
                }
            }
        }
        seen.nodes.insert(id, recorded);
        if !set.is_empty() || !removed.is_empty() {
            set.sort_by(|a, b| a.0.cmp(&b.0));
            events.push(NodeChange {
                event: ControlEvent::NodeComponents {
                    node: id,
                    set,
                    removed,
                },
                assets,
            });
        }
    }
    collected
}

/// The editor entities Live view fills with a running instance's values. A
/// component one of them keeps after its patch is gone holds that instance's
/// state, not the document's, so it is not sent.
fn projected_entities(world: &World) -> HashSet<Entity> {
    let live = world
        .get_resource::<PieViewMode>()
        .is_some_and(|mode| *mode == PieViewMode::Live);
    if !live {
        return HashSet::new();
    }
    world
        .get_resource::<PieProjection>()
        .map(|projection| projection.by_bits.values().copied().collect())
        .unwrap_or_default()
}

/// A node's authored patches as `(type path, digest)` pairs, in order.
type Authored = Vec<(String, u64)>;

/// Every node with a stable id, as the ids present, the nodes among them whose
/// patches changed since `since` (all of them when `since` is `None`), and whether
/// a node's list of patches or children changed, which is all that moves a node
/// other than the document's list of roots.
fn touched_nodes(
    ast: &mut SceneBsnAst,
    since: Option<(Tick, &HashSet<u64>)>,
    now: Tick,
) -> (Vec<(u64, Entity, Authored)>, HashSet<u64>, bool) {
    let mut query = ast.world.query::<(Entity, Ref<BsnPatches>)>();
    let mut structural = since.is_none();
    let nodes: Vec<(Entity, bool)> = query
        .iter(&ast.world)
        .map(|(node, patches)| {
            let changed = since.is_none_or(|(tick, _)| {
                let newer = |changed: Tick| changed.is_newer_than(tick, now);
                let mut changed = newer(patches.last_changed());
                structural |= changed;
                for &patch in &patches.0 {
                    let patch = ast
                        .world
                        .get_entity(patch)
                        .ok()
                        .and_then(|patch| patch.get_ref::<BsnPatch>());
                    let Some(patch) = patch else {
                        changed = true;
                        structural = true;
                        continue;
                    };
                    if newer(patch.last_changed()) {
                        changed = true;
                        structural |= matches!(*patch, BsnPatch::Children(_));
                    }
                }
                changed
            });
            (node, changed)
        })
        .collect();

    let mut touched = Vec::new();
    let mut present = HashSet::new();
    for (node, changed) in nodes {
        let Some(id) = ast.stable_id_of(node) else {
            continue;
        };
        present.insert(id);
        let retry = since.is_some_and(|(_, retry)| retry.contains(&id));
        if changed || retry {
            touched.push((id, node, authored_patches(ast, node)));
        }
    }
    (touched, present, structural)
}

/// The patches `node` authors as components, with a digest of each. The
/// node id, prefab links, templates, base scenes and children are not
/// components the game takes from a node.
fn authored_patches(ast: &SceneBsnAst, node: Entity) -> Authored {
    let Some(patches) = ast.get_patches(node) else {
        return Authored::new();
    };
    patches
        .0
        .iter()
        .filter_map(|&patch| ast.get_patch(patch))
        .filter_map(|patch| {
            let path = match patch {
                BsnPatch::Name(_) => Name::type_path(),
                BsnPatch::Type(_) | BsnPatch::Struct(_) | BsnPatch::TupleStruct(_) => {
                    patch_type_path(patch)?
                }
                BsnPatch::Base(_) | BsnPatch::Template(..) | BsnPatch::Children(_) => {
                    return None;
                }
            };
            if path == SCENE_NODE_ID_TYPE_PATH || path == ISA_TYPE {
                return None;
            }
            let mut hasher = DefaultHasher::new();
            format!("{patch:?}").hash(&mut hasher);
            Some((path.to_string(), hasher.finish()))
        })
        .collect()
}

/// Fold the asset references `entity`'s components hold into their digests. The
/// document mirrors a handle field without an asset context, so its patch reads
/// the same whichever asset the field points at. Hashing the reference rather
/// than the handle's id keeps a reparse, which changes ids, from reading as an
/// edit. A project component's patch names its assets as the document does, so its
/// session names go in the same way.
fn fold_in_references(
    world: &World,
    registry: &TypeRegistry,
    references: &mut PlayReferences,
    names: &mut crate::play_scene::PlayAssetNames,
    entity: Entity,
    node: Entity,
    components: &mut Components,
) {
    fn fold(digest: &mut u64, held: impl Hash) {
        let mut hasher = DefaultHasher::new();
        digest.hash(&mut hasher);
        held.hash(&mut hasher);
        *digest = hasher.finish();
    }
    for (path, digest) in components.iter_mut() {
        if let Some(value) = held_component(world, registry, entity, path) {
            let held = references.references_in(world, registry, value.as_partial_reflect(), names);
            if !held.is_empty() {
                fold(digest, held);
            }
        } else if let Some(schema) = world
            .get_resource::<ProjectTypes>()
            .and_then(|types| types.component(path))
        {
            let (_, session) = project_value(
                world,
                registry,
                names,
                &mut UsedAssets::default(),
                node,
                schema,
            );
            if !session.is_empty() {
                fold(digest, session);
            }
        }
    }
}

/// A project component's value as the document holds it, with every reference to
/// an asset the document embeds under its session name, as served text writes it,
/// and those names. The assets it names that a game may not hold go in `used`.
fn project_value(
    world: &World,
    registry: &TypeRegistry,
    names: &mut crate::play_scene::PlayAssetNames,
    used: &mut UsedAssets,
    node: Entity,
    schema: &jackdaw_schema::TypeSchema,
) -> (serde_json::Value, Vec<String>) {
    let mut value =
        crate::inspector::project_component_display::project_component_json(world, node, schema);
    let mut session = Vec::new();
    name_references_in_json(world, registry, names, used, &mut value, &mut session);
    (value, session)
}

fn name_references_in_json(
    world: &World,
    registry: &TypeRegistry,
    names: &mut crate::play_scene::PlayAssetNames,
    used: &mut UsedAssets,
    value: &mut serde_json::Value,
    session: &mut Vec<String>,
) {
    use serde_json::Value;
    match value {
        Value::String(text) => {
            if let Some((name, asset)) = names.session_reference(world, registry, text) {
                text.clone_from(&name);
                session.push(name);
                if let Some(asset) = asset
                    && !used
                        .runtime
                        .iter()
                        .any(|held| held.asset_id == asset.asset_id)
                {
                    used.runtime.push(asset);
                }
            } else if let Some(handle) = world
                .get_resource::<jackdaw_bsn::BsnProjectAssets>()
                .and_then(|project| project.0.get(text.as_str()))
                .filter(|_| jackdaw_bsn::is_document_path(std::path::Path::new(text.as_str())))
                && !used.files.iter().any(|file| file.id == handle.id())
            {
                used.files.push(AssetFile {
                    path: text.clone(),
                    type_id: handle.type_id(),
                    id: handle.id(),
                });
            }
        }
        Value::Array(items) => {
            for item in items {
                name_references_in_json(world, registry, names, used, item, session);
            }
        }
        Value::Object(fields) => {
            for (_, field) in fields.iter_mut() {
                name_references_in_json(world, registry, names, used, field, session);
            }
        }
        _ => {}
    }
}

/// Every asset the live document uses: those it embeds and those a component of
/// one of its entities holds.
pub(crate) fn assets_in_document(world: &World) -> HashSet<UntypedAssetId> {
    let mut used: HashSet<UntypedAssetId> = world
        .get_resource::<jackdaw_bsn::BsnSceneAssets>()
        .into_iter()
        .flat_map(|assets| assets.0.values().map(UntypedHandle::id))
        .collect();
    let Some(ast) = world.get_resource::<SceneBsnAst>() else {
        return used;
    };
    let registry = world.resource::<AppTypeRegistry>().read();
    let mut handles = Vec::new();
    for entity in crate::scene_io::save::doc_entities_in_order(ast) {
        let Ok(entity) = world.get_entity(entity) else {
            continue;
        };
        for &component in entity.archetype().components() {
            let Some(value) = world
                .components()
                .get_info(component)
                .and_then(|info| registry.get(info.type_id()?))
                .and_then(|registration| registration.data::<ReflectComponent>())
                .and_then(|reflect| reflect.reflect(entity))
            else {
                continue;
            };
            handles_in(value.as_partial_reflect(), &registry, &mut handles);
        }
    }
    used.extend(handles.iter().map(UntypedHandle::id));
    used
}

/// Fold a node's patch digests into one digest per component type path.
fn by_component(
    patches: Authored,
    registry: &TypeRegistry,
    project: Option<&ProjectTypes>,
) -> Components {
    let mut components: HashMap<String, DefaultHasher> = HashMap::new();
    for (path, digest) in patches {
        let key = component_path(&path, registry, project);
        digest.hash(components.entry(key).or_default());
    }
    components
        .into_iter()
        .map(|(key, hasher)| (key, hasher.finish()))
        .collect()
}

/// The component a patch's type path names: the path itself, or the enum a
/// `Enum::Variant` path is a variant of.
fn component_path(path: &str, registry: &TypeRegistry, project: Option<&ProjectTypes>) -> String {
    let known = |candidate: &str| {
        registry.get_with_type_path(candidate).is_some()
            || project.is_some_and(|types| types.type_schema(candidate).is_some())
    };
    if known(path) {
        return path.to_string();
    }
    match path.rsplit_once("::") {
        Some((base, _)) if known(base) => base.to_string(),
        _ => path.to_string(),
    }
}

/// Why a component was not sent, and the field that stopped it (empty for the
/// component as a whole).
struct Unsent {
    field: String,
    reason: String,
}

impl Unsent {
    fn whole(reason: impl Into<String>) -> Self {
        Self {
            field: String::new(),
            reason: reason.into(),
        }
    }

    /// Warn once per node, component and field.
    fn warn(&self, warned: &mut HashSet<String>, node: u64, entity: Entity, path: &str) {
        if !warned.insert(format!("{node}/{path}/{}", self.field)) {
            return;
        }
        if self.field.is_empty() {
            warn!(
                "Play: {path} on node {node} (entity {entity}) was not sent to the game: {}",
                self.reason
            );
        } else {
            warn!(
                "Play: {path} on node {node} (entity {entity}) was not sent to the game: \
                 field `{}` {}; a later edit tries again",
                self.field, self.reason
            );
        }
    }
}

/// How the game's scene loader names each asset a handle can point at: the
/// asset path, the catalog's `@Name`, or the one `#Name` the Play session gave a
/// runtime asset (see [`crate::play_scene::PlayAssetNames`]). Served text and
/// edits read names from the same place.
pub(crate) struct PlayReferences {
    fixed: HashMap<UntypedAssetId, String>,
    session: HashMap<UntypedAssetId, String>,
    /// The assets named since this was made, none of which need looking at again.
    current: HashSet<UntypedAssetId>,
}

impl PlayReferences {
    pub(crate) fn with(world: &World, names: &crate::play_scene::PlayAssetNames) -> Self {
        let mut fixed = crate::scene_io::save::asset_reference_seed(world);
        let unresolved = world.get_resource::<jackdaw_bsn::UnresolvedReferences>();
        fixed.retain(|id, reference| {
            !reference.starts_with('#')
                && !jackdaw_bsn::is_placeholder_id(*id)
                && !unresolved.is_some_and(|unresolved| unresolved.is_retired(*id))
        });
        let session = names.iter().map(|(id, name)| (*id, name.clone())).collect();
        Self {
            fixed,
            session,
            current: HashSet::new(),
        }
    }

    /// The reference of each handle `value` holds, in order, `None` for a default
    /// handle or an asset with no reference.
    fn references_in(
        &mut self,
        world: &World,
        registry: &TypeRegistry,
        value: &dyn PartialReflect,
        names: &mut crate::play_scene::PlayAssetNames,
    ) -> Vec<Option<String>> {
        let mut handles = Vec::new();
        handles_in(value, registry, &mut handles);
        if handles
            .iter()
            .any(|handle| !self.current.contains(&handle.id()))
        {
            self.take_up(world, registry, value, names, &mut UsedAssets::default());
            self.current.extend(handles.iter().map(UntypedHandle::id));
        }
        handles
            .iter()
            .map(|handle| self.reference(handle))
            .collect()
    }

    /// Name the runtime assets `value` holds handles to, listing them and the
    /// asset files it names in `used`.
    fn take_up(
        &mut self,
        world: &World,
        registry: &TypeRegistry,
        value: &dyn PartialReflect,
        names: &mut crate::play_scene::PlayAssetNames,
        used: &mut UsedAssets,
    ) {
        let mut handles = Vec::new();
        handles_in(value, registry, &mut handles);
        for handle in handles {
            let id = handle.id();
            let type_id = handle.type_id();
            if matches!(handle, UntypedHandle::Uuid { .. })
                || registry
                    .get(type_id)
                    .and_then(|registration| registration.data::<ReflectAsset>())
                    .is_none()
            {
                continue;
            }
            if handle.path().is_none() && !self.fixed.contains_key(&id) {
                if let Some(name) = names.resolve(world, id, type_id) {
                    if !used.runtime.iter().any(|asset| asset.asset_id == id) {
                        used.runtime.push(CatalogAssetRef {
                            name: name.trim_start_matches('#').to_string(),
                            type_id,
                            asset_id: id,
                        });
                    }
                    self.session.insert(id, name);
                }
                continue;
            }
            if let Some(path) = self
                .reference(&handle)
                .filter(|reference| jackdaw_bsn::is_document_path(std::path::Path::new(reference)))
                && !used.files.iter().any(|file| file.id == id)
            {
                used.files.push(AssetFile { path, type_id, id });
            }
        }
    }

    pub(crate) fn reference(&self, handle: &UntypedHandle) -> Option<String> {
        let id = handle.id();
        handle
            .path()
            .map(ToString::to_string)
            .or_else(|| self.fixed.get(&id).cloned())
            .or_else(|| self.session.get(&id).cloned())
    }
}

/// Every asset handle in `value`, in the order reflection visits them.
fn handles_in(value: &dyn PartialReflect, registry: &TypeRegistry, out: &mut Vec<UntypedHandle>) {
    if let Some(concrete) = value.try_as_reflect() {
        let type_id = concrete.reflect_type_info().type_id();
        if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(type_id) {
            out.extend(reflect_handle.downcast_handle_untyped(concrete.as_any()));
            return;
        }
    }
    match value.reflect_ref() {
        ReflectRef::Struct(value) => {
            for index in 0..value.field_len() {
                if let Some(field) = value.field_at(index) {
                    handles_in(field, registry, out);
                }
            }
        }
        ReflectRef::TupleStruct(value) => {
            for index in 0..value.field_len() {
                if let Some(field) = value.field(index) {
                    handles_in(field, registry, out);
                }
            }
        }
        ReflectRef::Tuple(value) => {
            for index in 0..value.field_len() {
                if let Some(field) = value.field(index) {
                    handles_in(field, registry, out);
                }
            }
        }
        ReflectRef::List(value) => value
            .iter()
            .for_each(|item| handles_in(item, registry, out)),
        ReflectRef::Array(value) => value
            .iter()
            .for_each(|item| handles_in(item, registry, out)),
        ReflectRef::Map(value) => {
            value
                .iter()
                .for_each(|(_, item)| handles_in(item, registry, out));
        }
        ReflectRef::Set(value) => {
            value
                .iter()
                .for_each(|item| handles_in(item, registry, out));
        }
        ReflectRef::Enum(value) => {
            for index in 0..value.field_len() {
                if let Some(field) = value.field_at(index) {
                    handles_in(field, registry, out);
                }
            }
        }
        _ => {}
    }
}

/// The component `type_path` names on `entity`, when the editor registers it
/// and the entity holds it.
fn held_component<'w>(
    world: &'w World,
    registry: &TypeRegistry,
    entity: Entity,
    type_path: &str,
) -> Option<&'w dyn Reflect> {
    registry
        .get_with_type_path(type_path)?
        .data::<ReflectComponent>()?
        .reflect(world.entity(entity))
}

/// The full current value of `type_path` on the node, in the form the game's
/// reflection deserializer reads: the editor entity's component for a type
/// the editor registers, else the document's value for a project type, naming
/// assets as the game holds them.
#[expect(
    clippy::too_many_arguments,
    reason = "the naming state and the assets used travel alongside the value's address"
)]
fn component_value(
    world: &World,
    registry: &TypeRegistry,
    references: &mut PlayReferences,
    names: &mut crate::play_scene::PlayAssetNames,
    used: &mut UsedAssets,
    entity: Entity,
    node: Entity,
    type_path: &str,
) -> Result<serde_json::Value, Unsent> {
    if let Some(registration) = registry.get_with_type_path(type_path) {
        let reflect = registration
            .data::<ReflectComponent>()
            .ok_or_else(|| Unsent::whole("it is not a reflected component"))?;
        let value = reflect
            .reflect(world.entity(entity))
            .ok_or_else(|| Unsent::whole("the editor entity does not hold it"))?;
        references.take_up(world, registry, value.as_partial_reflect(), names, used);
        return game_value(value.as_partial_reflect(), registry, references);
    }
    let schema = world
        .get_resource::<ProjectTypes>()
        .and_then(|types| types.component(type_path))
        .ok_or_else(|| Unsent::whole("the type is not registered"))?;
    Ok(project_value(world, registry, names, used, node, schema).0)
}

/// Serialize a component for the game. An asset handle goes as the reference
/// the game's scene loader resolves (see [`PlayReferences`]), or `null` for
/// the type's default handle. A value holding a handle with no such
/// reference, or an entity reference, has no meaning in the game and is
/// refused, naming the field.
fn game_value(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    references: &PlayReferences,
) -> Result<serde_json::Value, Unsent> {
    let mut field = String::new();
    if let Some(reason) = first_unsendable(value, registry, references, &mut field) {
        return Err(Unsent {
            field: field.trim_start_matches('.').to_string(),
            reason: reason.to_string(),
        });
    }
    let processor = GameValueProcessor { references };
    let serializer = TypedReflectSerializer::with_processor(value, registry, &processor);
    serde_json::to_value(&serializer).map_err(|err| Unsent::whole(err.to_string()))
}

/// The first part of `value` the game cannot be sent, with its field path
/// left in `field`.
fn first_unsendable(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    references: &PlayReferences,
    field: &mut String,
) -> Option<&'static str> {
    if let Some(value) = value.try_as_reflect() {
        let type_id = value.reflect_type_info().type_id();
        if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(type_id) {
            let handle = reflect_handle.downcast_handle_untyped(value.as_any());
            let default = UntypedHandle::default_for_type(reflect_handle.asset_type_id());
            let sendable = handle.is_some_and(|handle| {
                handle.id() == default.id()
                    || jackdaw_bsn::is_placeholder_id(handle.id())
                    || references.reference(&handle).is_some()
            });
            return (!sendable).then_some("holds an asset that cannot be embedded for the game");
        }
        if type_id == TypeId::of::<Entity>() {
            return Some("holds an entity reference");
        }
    }
    let mut visit = |name: String, value: &dyn PartialReflect| {
        let len = field.len();
        field.push_str(&name);
        let found = first_unsendable(value, registry, references, field);
        if found.is_none() {
            field.truncate(len);
        }
        found
    };
    match value.reflect_ref() {
        ReflectRef::Struct(value) => (0..value.field_len()).find_map(|index| {
            let name = value.name_at(index).unwrap_or_default();
            visit(format!(".{name}"), value.field_at(index)?)
        }),
        ReflectRef::TupleStruct(value) => {
            (0..value.field_len()).find_map(|index| visit(format!(".{index}"), value.field(index)?))
        }
        ReflectRef::Tuple(value) => {
            (0..value.field_len()).find_map(|index| visit(format!(".{index}"), value.field(index)?))
        }
        ReflectRef::List(value) => value
            .iter()
            .enumerate()
            .find_map(|(index, item)| visit(format!("[{index}]"), item)),
        ReflectRef::Array(value) => value
            .iter()
            .enumerate()
            .find_map(|(index, item)| visit(format!("[{index}]"), item)),
        ReflectRef::Map(value) => value
            .iter()
            .enumerate()
            .find_map(|(index, (_, item))| visit(format!("[{index}]"), item)),
        ReflectRef::Set(value) => value
            .iter()
            .enumerate()
            .find_map(|(index, item)| visit(format!("[{index}]"), item)),
        ReflectRef::Enum(value) => (0..value.field_len()).find_map(|index| {
            let name = value
                .name_at(index)
                .map_or_else(|| index.to_string(), str::to_string);
            visit(format!(".{name}"), value.field_at(index)?)
        }),
        _ => None,
    }
}

struct GameValueProcessor<'a> {
    references: &'a PlayReferences,
}

impl ReflectSerializerProcessor for GameValueProcessor<'_> {
    fn try_serialize<S>(
        &self,
        value: &dyn PartialReflect,
        registry: &TypeRegistry,
        serializer: S,
    ) -> Result<Result<S::Ok, S>, S::Error>
    where
        S: Serializer,
    {
        let Some(value) = value.try_as_reflect() else {
            return Ok(Err(serializer));
        };
        let type_id = value.reflect_type_info().type_id();
        if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(type_id) {
            let reference = reflect_handle
                .downcast_handle_untyped(value.as_any())
                .and_then(|handle| self.references.reference(&handle));
            return Ok(Ok(match reference {
                Some(reference) => serializer.serialize_str(&reference)?,
                None => serializer.serialize_unit()?,
            }));
        }
        if type_id == TypeId::of::<Entity>() {
            return Ok(Ok(serializer.serialize_unit()?));
        }
        Ok(Err(serializer))
    }
}

#[cfg(test)]
mod tests {
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    use jackdaw_bsn::{BsnValue, component_to_bsn_patch, set_bsn_field};
    use jackdaw_pie_protocol::event::from_bytes;
    use jackdaw_pie_protocol::{IpcChannelTransport, PieTransport};
    use jackdaw_scene_types::SceneNodeId;

    use super::*;
    use crate::pie::{InstanceKey, PieSession};

    const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
    const NODE: u64 = 42;

    /// A process standing in for a game instance; it exits on its own.
    fn child() -> Child {
        Command::new(std::env::current_exe().expect("the test binary"))
            .arg("--list")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("a child process")
    }

    /// The editor's and the game's ends of a connected link.
    fn link() -> (IpcChannelTransport, IpcChannelTransport) {
        let (server, name) = jackdaw_pie_protocol::serve().expect("a rendezvous");
        let accept = std::thread::spawn(move || server.accept());
        let game = jackdaw_pie_protocol::connect(&name).expect("the game connects");
        let editor = accept
            .join()
            .expect("accept ran")
            .expect("the editor accepts");
        (editor, game)
    }

    fn key(instance: u32) -> InstanceKey {
        InstanceKey {
            config: "Game".to_string(),
            instance,
        }
    }

    /// A document with one node bound to an editor entity, and no session.
    fn editor_world() -> (World, Entity) {
        editor_world_with(None)
    }

    /// [`editor_world`], the node also authoring `visibility` when given.
    fn editor_world_with(visibility: Option<Visibility>) -> (World, Entity) {
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        {
            let mut registry = registry.write();
            registry.register::<Transform>();
            registry.register::<SceneNodeId>();
            registry.register::<Visibility>();
        }
        world.insert_resource(registry.clone());
        world.init_resource::<DocumentSync>();
        let entity = world.spawn((SceneNodeId(NODE), Transform::default())).id();
        let patches = {
            let registry = registry.read();
            let mut patches = vec![
                component_to_bsn_patch(&SceneNodeId(NODE), &registry),
                component_to_bsn_patch(&Transform::default(), &registry),
            ];
            if let Some(visibility) = visibility {
                world.entity_mut(entity).insert(visibility);
                patches.push(component_to_bsn_patch(&visibility, &registry));
            }
            patches
        };
        let mut ast = SceneBsnAst::default();
        let node = ast.create_entity_node(patches);
        ast.add_to_roots(node);
        ast.link(entity, node);
        world.insert_resource(ast);
        (world, entity)
    }

    /// Move the node to `x` in the document and on the editor entity.
    fn move_to(world: &mut World, entity: Entity, x: f32) {
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let value = BsnValue::from_reflect(&Transform::from_xyz(x, 0.0, 0.0), &registry);
        world.resource_scope(|_, mut ast: Mut<SceneBsnAst>| {
            let node = ast.ast_for(entity).expect("a node");
            set_bsn_field(&mut ast, node, TRANSFORM, "", value, &registry);
        });
        world
            .get_mut::<Transform>(entity)
            .expect("a transform")
            .translation
            .x = x;
    }

    /// The translations the game end received, in order. Sends leave on a
    /// writer thread, so this queues a marker behind them and reads up to it,
    /// which covers everything sent so far.
    fn received_x(world: &mut World, game: &mut IpcChannelTransport) -> Vec<f64> {
        crate::pie::broadcast_control(world, ControlEvent::Resume);
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut translations = Vec::new();
        while Instant::now() < deadline {
            for (_, bytes) in game.drain_received() {
                match from_bytes(&bytes) {
                    Ok(ControlEvent::Resume) => return translations,
                    Ok(ControlEvent::NodeComponents { node, set, .. }) if node == NODE => {
                        translations.extend(
                            set.into_iter()
                                .find(|(path, _)| path == TRANSFORM)
                                .and_then(|(_, value)| value["translation"][0].as_f64()),
                        );
                    }
                    _ => {}
                }
            }
            std::thread::yield_now();
        }
        panic!("the marker did not arrive");
    }

    /// The document as a tab holding `zone.bsn`, in an open project, with what
    /// the event drain needs.
    fn serving(world: &mut World, dir: &std::path::Path) {
        world.insert_resource(crate::project::ProjectRoot::new(
            dir.to_path_buf(),
            crate::project::ProjectConfig::default(),
        ));
        let mut tab = crate::scenes::SceneTab::new_untitled(1);
        tab.path = Some(dir.join("assets/zone.bsn"));
        world.insert_resource(crate::scenes::Scenes {
            tabs: vec![tab],
            active: 0,
        });
        world.insert_resource(crate::pie_mirror::PieInstances {
            buffers: Default::default(),
            focused: Some(key(1)),
        });
        world.insert_resource(PieViewMode::Scene);
        world.init_resource::<PieProjection>();
    }

    /// A game asks over `game` for the scene, and the editor's drain replies.
    fn answer_over_link(world: &mut World, game: &mut IpcChannelTransport) -> String {
        let request = jackdaw_pie_protocol::StateEvent::RequestScene {
            request: 7,
            path: "zone.bsn".to_string(),
        };
        game.send(
            jackdaw_pie_protocol::PieChannel::Reliable,
            &jackdaw_pie_protocol::event::to_bytes(&request).expect("encode"),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            crate::pie::drain_game_events(world);
            for (_, bytes) in game.drain_received() {
                if let Ok(ControlEvent::SceneDocument { request, answer }) = from_bytes(&bytes) {
                    assert_eq!(request, 7);
                    let jackdaw_pie_protocol::SceneAnswer::Document { text, .. } = answer else {
                        panic!("the open scene answers with its document: {answer:?}");
                    };
                    return text;
                }
            }
            std::thread::yield_now();
        }
        panic!("no answer arrived");
    }

    /// Asked for a scene, the editor's main thread parses nothing and goes on: the
    /// text is written on a worker, and an edit made meanwhile reaches the game
    /// behind the answer, however late that is.
    #[test]
    fn an_answer_comes_from_a_worker_ahead_of_edits_made_while_it_is_written() {
        let dir = tempfile::tempdir().expect("a temp dir");
        bevy::tasks::AsyncComputeTaskPool::get_or_init(bevy::tasks::TaskPool::new);
        let (mut world, entity) = editor_world();
        serving(&mut world, dir.path());
        let (editor_end, mut game) = link();
        let mut session = PieSession::default();
        session.insert_test_live(key(1), child(), editor_end);
        world.insert_non_send(session);
        sync_document_to_game(&mut world);

        let parses = jackdaw_bsn::parses_on_this_thread();
        let request = jackdaw_pie_protocol::StateEvent::RequestScene {
            request: 7,
            path: "zone.bsn".to_string(),
        };
        game.send(
            jackdaw_pie_protocol::PieChannel::Reliable,
            &jackdaw_pie_protocol::event::to_bytes(&request).expect("encode"),
        );
        let deadline = Instant::now() + Duration::from_secs(30);
        while world.non_send::<PieSession>().answers_being_written() == 0 {
            assert!(Instant::now() < deadline, "the request was never taken up");
            crate::pie::drain_game_events(&mut world);
            std::thread::yield_now();
        }
        move_to(&mut world, entity, 6.0);
        sync_document_to_game(&mut world);

        let mut order = Vec::new();
        while order.len() < 2 {
            assert!(Instant::now() < deadline, "got only {order:?}");
            crate::pie::drain_game_events(&mut world);
            for (_, bytes) in game.drain_received() {
                match from_bytes(&bytes) {
                    Ok(ControlEvent::SceneDocument { .. }) => order.push("answer"),
                    Ok(ControlEvent::NodeComponents { .. }) => order.push("edit"),
                    _ => {}
                }
            }
            std::thread::yield_now();
        }
        assert_eq!(order, ["answer", "edit"]);
        assert_eq!(
            jackdaw_bsn::parses_on_this_thread(),
            parses,
            "nothing is parsed on the main thread"
        );
    }

    /// An instance launched while another runs gets the edits made while it
    /// built and connected from the document it spawns, and those after its
    /// reply over the link, with the instance already live.
    #[test]
    fn a_second_instance_gets_the_edits_made_while_it_built_and_connected() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let (mut world, entity) = editor_world();
        serving(&mut world, dir.path());
        let (first_editor, mut first_game) = link();
        let mut session = PieSession::default();
        session.insert_test_live(key(1), child(), first_editor);
        session.insert_test_pending_build(key(2));
        world.insert_non_send(session);
        sync_document_to_game(&mut world);

        move_to(&mut world, entity, 6.0);
        sync_document_to_game(&mut world);
        assert_eq!(received_x(&mut world, &mut first_game), vec![6.0]);

        let (second_editor, mut second_game) = link();
        world
            .non_send_mut::<PieSession>()
            .insert_test_live(key(2), child(), second_editor);
        let text = answer_over_link(&mut world, &mut second_game);
        assert!(
            text.contains("x: 6.0"),
            "the edit is in the document:\n{text}"
        );

        move_to(&mut world, entity, 9.0);
        sync_document_to_game(&mut world);
        assert_eq!(received_x(&mut world, &mut first_game), vec![9.0]);
        assert_eq!(received_x(&mut world, &mut second_game), vec![9.0]);
    }

    #[test]
    fn nothing_is_sent_or_recorded_while_stopped() {
        let (mut world, entity) = editor_world();
        sync_document_to_game(&mut world);
        world.insert_non_send(PieSession::default());
        move_to(&mut world, entity, 3.0);
        sync_document_to_game(&mut world);
        assert!(
            world.resource::<DocumentSync>().seen.is_none(),
            "no baseline is taken with no instance launched"
        );
    }

    #[test]
    fn an_edit_reaches_a_live_instance_while_another_launches() {
        let (mut world, entity) = editor_world();
        let mut session = PieSession::default();
        session.insert_test_connecting(key(1), child());
        world.insert_non_send(session);
        sync_document_to_game(&mut world);
        assert!(
            world.resource::<DocumentSync>().seen.is_some(),
            "the launch records the baseline"
        );

        let (editor_end, mut game_end) = link();
        {
            let mut session = world.non_send_mut::<PieSession>();
            session.insert_test_live(key(2), child(), editor_end);
            session.insert_test_pending_build(key(3));
        }
        move_to(&mut world, entity, 6.0);
        sync_document_to_game(&mut world);
        assert_eq!(received_x(&mut world, &mut game_end), vec![6.0]);

        sync_document_to_game(&mut world);
        assert!(
            received_x(&mut world, &mut game_end).is_empty(),
            "an edit goes once"
        );
    }

    #[test]
    fn an_edit_made_while_launching_goes_once_an_instance_is_live() {
        let (mut world, entity) = editor_world();
        let mut session = PieSession::default();
        session.insert_test_pending_build(key(1));
        world.insert_non_send(session);
        sync_document_to_game(&mut world);
        move_to(&mut world, entity, 2.0);
        sync_document_to_game(&mut world);

        let (editor_end, mut game_end) = link();
        world
            .non_send_mut::<PieSession>()
            .insert_test_live(key(2), child(), editor_end);
        sync_document_to_game(&mut world);
        assert_eq!(received_x(&mut world, &mut game_end), vec![2.0]);
    }

    #[test]
    fn a_save_to_scene_write_is_not_sent_back() {
        let (mut world, entity) = editor_world();
        let (editor_end, mut game_end) = link();
        let mut session = PieSession::default();
        session.insert_test_live(key(1), child(), editor_end);
        world.insert_non_send(session);
        sync_document_to_game(&mut world);

        move_to(&mut world, entity, 8.0);
        absorb_document_changes(&mut world);
        sync_document_to_game(&mut world);
        assert!(received_x(&mut world, &mut game_end).is_empty());

        move_to(&mut world, entity, 1.0);
        sync_document_to_game(&mut world);
        assert_eq!(received_x(&mut world, &mut game_end), vec![1.0]);
    }

    #[test]
    fn a_failed_build_drops_the_baseline() {
        let (mut world, entity) = editor_world();
        let mut session = PieSession::default();
        session.insert_test_pending_build(key(1));
        world.insert_non_send(session);
        sync_document_to_game(&mut world);
        assert!(world.resource::<DocumentSync>().seen.is_some());

        world.non_send_mut::<PieSession>().fail_test_builds();
        sync_document_to_game(&mut world);
        assert!(
            world.resource::<DocumentSync>().seen.is_none(),
            "nothing is live or launching, so the baseline goes"
        );

        move_to(&mut world, entity, 5.0);
        world
            .non_send_mut::<PieSession>()
            .insert_test_pending_build(key(2));
        sync_document_to_game(&mut world);
        let (editor_end, mut game_end) = link();
        world
            .non_send_mut::<PieSession>()
            .insert_test_live(key(2), child(), editor_end);
        sync_document_to_game(&mut world);
        assert!(
            received_x(&mut world, &mut game_end).is_empty(),
            "the next launch starts from the document as it is then"
        );
    }

    /// A derived component whose patch goes while Live view projects the
    /// entity is held back, and goes with the node's next edit.
    #[test]
    fn a_derived_component_left_while_projected_is_sent_later() {
        const VISIBILITY: &str = "bevy_camera::visibility::Visibility";
        let (mut world, entity) = editor_world_with(Some(Visibility::Hidden));
        collect_node_changes(&mut world);

        world.insert_resource(PieViewMode::Live);
        let mut projection = PieProjection::default();
        projection.by_bits.insert(1, entity);
        world.insert_resource(projection);
        world.resource_scope(|_, mut ast: Mut<SceneBsnAst>| {
            let node = ast.ast_for(entity).expect("a node");
            ast.remove_component_patch(node, VISIBILITY);
        });
        *world.get_mut::<Visibility>(entity).expect("a visibility") = Visibility::Inherited;
        assert!(collect_node_changes(&mut world).is_empty());

        world.insert_resource(PieViewMode::Scene);
        move_to(&mut world, entity, 1.0);
        let events = collect_node_changes(&mut world);
        let [ControlEvent::NodeComponents { set, removed, .. }] = events.as_slice() else {
            panic!("one node event, got {events:?}");
        };
        assert!(removed.is_empty());
        let sent: Vec<&str> = set.iter().map(|(path, _)| path.as_str()).collect();
        assert_eq!(sent, vec![VISIBILITY, TRANSFORM]);
    }

    #[test]
    fn a_node_without_an_editor_entity_is_retried_only_when_the_document_changes() {
        let (mut world, entity) = editor_world();
        collect_node_changes(&mut world);
        move_to(&mut world, entity, 4.0);
        world.resource_mut::<SceneBsnAst>().unlink(entity);
        assert!(collect_node_changes(&mut world).is_empty());
        assert!(
            !world
                .resource::<DocumentSync>()
                .seen
                .as_ref()
                .expect("a baseline")
                .retry
                .is_empty()
        );

        let world_tick = |world: &World| {
            world
                .resource::<DocumentSync>()
                .seen
                .as_ref()
                .expect("a baseline")
                .document_tick
        };
        let before = world_tick(&world);
        assert!(collect_node_changes(&mut world).is_empty());
        assert_eq!(
            world_tick(&world),
            before,
            "an unchanged document is not scanned again"
        );

        let node = world
            .resource::<SceneBsnAst>()
            .node_by_stable_id(NODE)
            .expect("the node");
        world.resource_mut::<SceneBsnAst>().link(entity, node);
        let events = collect_node_changes(&mut world);
        assert_eq!(events.len(), 1, "the retry sends once the node is bound");
    }
}
