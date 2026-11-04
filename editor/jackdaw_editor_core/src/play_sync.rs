//! Pushes the scene document's component changes to the running game.
//!
//! While play is running, each frame finds the document nodes whose patches
//! changed (the document world's change ticks, or a full pass when the
//! document was replaced) and compares each node's components against what
//! was last seen. Every component whose patch differs goes to every instance
//! as its full current value in a [`ControlEvent::NodeComponents`], together
//! with the components the node no longer has. Commands, undo, redo and
//! remote edits all change the document, so no command needs to report its
//! own edit.

use std::any::TypeId;
use std::cell::Cell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bevy::asset::{ReflectHandle, UntypedHandle};
use bevy::ecs::change_detection::Tick;
use bevy::ecs::reflect::ReflectComponent;
use bevy::ecs::world::WorldId;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::serde::{ReflectSerializerProcessor, TypedReflectSerializer};
use bevy::reflect::{PartialReflect, TypePath, TypeRegistry};
use jackdaw_api::pie::PlayState;
use jackdaw_bsn::{BsnPatch, BsnPatches, SceneBsnAst, patch_type_path};
use jackdaw_pie_protocol::ControlEvent;
use jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH;
use serde::Serializer;

use crate::pie::SyncTarget;
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
}

impl DocumentSync {
    /// Forget what was seen, so the next collection starts from the document
    /// as it is then and sends nothing for it.
    pub fn forget(&mut self) {
        self.seen = None;
    }
}

struct Seen {
    document: WorldId,
    document_tick: Tick,
    world_tick: Tick,
    nodes: HashMap<u64, Components>,
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

fn sync_document_to_game(world: &mut World) {
    let target = world
        .get_non_send::<crate::pie::PieSession>()
        .map_or(SyncTarget::None, crate::pie::PieSession::sync_target);
    match target {
        SyncTarget::None => {}
        SyncTarget::Launching => {
            if world.resource::<DocumentSync>().seen.is_none() {
                collect_node_changes(world);
            }
        }
        SyncTarget::Live => {
            for event in collect_node_changes(world) {
                crate::pie::broadcast_control(world, event);
            }
        }
    }
}

/// The node edits made to the document since the last call, one event per
/// changed node. The first call after [`DocumentSync::forget`] records the
/// document and returns nothing.
pub fn collect_node_changes(world: &mut World) -> Vec<ControlEvent> {
    if !world.contains_resource::<SceneBsnAst>() {
        return Vec::new();
    }
    world.resource_scope(|world, mut sync: Mut<DocumentSync>| collect(world, &mut sync))
}

fn collect(world: &mut World, sync: &mut DocumentSync) -> Vec<ControlEvent> {
    let world_now = world.change_tick();
    world.increment_change_tick();
    let document_changed =
        world
            .get_resource_ref::<SceneBsnAst>()
            .is_some_and(|ast| match &sync.seen {
                Some(seen) => ast.last_changed().is_newer_than(seen.world_tick, world_now),
                None => true,
            });
    let idle = sync.seen.as_ref().is_some_and(|seen| seen.retry.is_empty());
    if idle && !document_changed {
        if let Some(seen) = sync.seen.as_mut() {
            seen.world_tick = world_now;
        }
        return Vec::new();
    }

    let (document, document_tick, authored, present) = {
        let mut ast = world.resource_mut::<SceneBsnAst>();
        let ast = ast.bypass_change_detection();
        let document = ast.world.id();
        let now = ast.world.change_tick();
        let since = sync
            .seen
            .as_ref()
            .filter(|seen| seen.document == document)
            .map(|seen| (seen.document_tick, &seen.retry));
        let (authored, present) = touched_nodes(ast, since, now);
        ast.world.increment_change_tick();
        (document, now, authored, present)
    };
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let project = world.get_resource::<ProjectTypes>();
    let touched: Vec<Touched> = authored
        .into_iter()
        .map(|(id, node, patches)| Touched {
            id,
            node,
            components: by_component(patches, &registry, project),
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
            retry: HashSet::new(),
        });
        return Vec::new();
    };
    seen.document = document;
    seen.document_tick = document_tick;
    seen.world_tick = world_now;
    seen.nodes.retain(|id, _| present.contains(id));
    seen.retry.retain(|id| present.contains(id));

    let mut events = Vec::new();
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
        let mut removed: Vec<String> = before
            .keys()
            .filter(|path| !components.contains_key(*path))
            .cloned()
            .collect();
        if set_paths.is_empty() && removed.is_empty() {
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
        removed.sort();
        let mut set = Vec::new();
        for path in set_paths {
            match component_value(world, &registry, entity, node, path) {
                Ok(value) => set.push((path.clone(), value)),
                Err(reason) => {
                    if sync.warned.insert(path.clone()) {
                        warn!("Play: {path} was not sent to the game: {reason}");
                    }
                }
            }
        }
        seen.nodes.insert(id, components);
        seen.retry.remove(&id);
        if !set.is_empty() || !removed.is_empty() {
            events.push(ControlEvent::NodeComponents {
                node: id,
                set,
                removed,
            });
        }
    }
    events
}

/// A node's authored patches as `(type path, digest)` pairs, in order.
type Authored = Vec<(String, u64)>;

/// Every node with a stable id, as the ids present, and the nodes among them
/// whose patches changed since `since` (all of them when `since` is `None`).
fn touched_nodes(
    ast: &mut SceneBsnAst,
    since: Option<(Tick, &HashSet<u64>)>,
    now: Tick,
) -> (Vec<(u64, Entity, Authored)>, HashSet<u64>) {
    let mut query = ast.world.query::<(Entity, Ref<BsnPatches>)>();
    let nodes: Vec<(Entity, bool)> = query
        .iter(&ast.world)
        .map(|(node, patches)| {
            let changed = since.is_none_or(|(tick, _)| {
                let newer = |changed: Tick| changed.is_newer_than(tick, now);
                newer(patches.last_changed())
                    || patches.0.iter().any(|&patch| {
                        ast.world
                            .get_entity(patch)
                            .ok()
                            .and_then(|patch| patch.get_ref::<BsnPatch>())
                            .is_none_or(|patch| newer(patch.last_changed()))
                    })
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
    (touched, present)
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

/// The full current value of `type_path` on the node, in the form the game's
/// reflection deserializer reads: the editor entity's component for a type
/// the editor registers, else the document's value for a project type.
fn component_value(
    world: &World,
    registry: &TypeRegistry,
    entity: Entity,
    node: Entity,
    type_path: &str,
) -> Result<serde_json::Value, String> {
    if let Some(registration) = registry.get_with_type_path(type_path) {
        let reflect = registration
            .data::<ReflectComponent>()
            .ok_or("it is not a reflected component")?;
        let value = reflect
            .reflect(world.entity(entity))
            .ok_or("the editor entity does not hold it")?;
        return serialize_for_game(value.as_partial_reflect(), registry);
    }
    let schema = world
        .get_resource::<ProjectTypes>()
        .and_then(|types| types.component(type_path))
        .ok_or("the type is not registered")?;
    Ok(crate::inspector::project_component_display::project_component_json(world, node, schema))
}

/// Serialize a component for the game. An asset handle goes as its asset
/// path (or `null` for the type's default handle); a value holding a handle
/// with no path, or an entity reference, has no meaning in the game and is
/// refused.
pub(crate) fn serialize_for_game(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
) -> Result<serde_json::Value, String> {
    let processor = GameValueProcessor {
        refused: Cell::new(None),
    };
    let serializer = TypedReflectSerializer::with_processor(value, registry, &processor);
    let json = serde_json::to_value(&serializer).map_err(|err| err.to_string())?;
    match processor.refused.take() {
        Some(reason) => Err(reason.to_string()),
        None => Ok(json),
    }
}

struct GameValueProcessor {
    refused: Cell<Option<&'static str>>,
}

impl ReflectSerializerProcessor for GameValueProcessor {
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
            let handle = reflect_handle.downcast_handle_untyped(value.as_any());
            if let Some(path) = handle.as_ref().and_then(UntypedHandle::path) {
                return Ok(Ok(serializer.serialize_str(&path.to_string())?));
            }
            let default = UntypedHandle::default_for_type(reflect_handle.asset_type_id());
            if handle.is_none_or(|handle| handle.id() != default.id()) {
                self.refused
                    .set(Some("it holds an asset handle with no asset path"));
            }
            return Ok(Ok(serializer.serialize_unit()?));
        }
        if type_id == TypeId::of::<Entity>() {
            self.refused.set(Some("it holds an entity reference"));
            return Ok(Ok(serializer.serialize_unit()?));
        }
        Ok(Err(serializer))
    }
}
