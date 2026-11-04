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

use std::any::TypeId;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use bevy::asset::{ReflectHandle, UntypedAssetId, UntypedHandle};
use bevy::ecs::change_detection::Tick;
use bevy::ecs::reflect::ReflectComponent;
use bevy::ecs::world::WorldId;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bevy::reflect::serde::{ReflectSerializerProcessor, TypedReflectSerializer};
use bevy::reflect::{PartialReflect, ReflectRef, TypePath, TypeRegistry};
use jackdaw_api::pie::PlayState;
use jackdaw_bsn::{BsnPatch, BsnPatches, SceneBsnAst, patch_type_path};
use jackdaw_pie_protocol::ControlEvent;
use jackdaw_scene_types::SCENE_NODE_ID_TYPE_PATH;
use serde::Serializer;

use crate::pie::SyncTarget;
use crate::pie_mirror::PieViewMode;
use crate::pie_projection::PieProjection;
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

/// Send the document's node changes to every live instance. A launching
/// instance is not sent to: it loads the scene when it starts. With no
/// instance live, the first launch records the document as the baseline.
pub(crate) fn sync_document_to_game(world: &mut World) {
    let target = world
        .get_non_send::<crate::pie::PieSession>()
        .map_or(SyncTarget::None, crate::pie::PieSession::sync_target);
    match target {
        SyncTarget::None => {}
        SyncTarget::Launching => {
            if world
                .get_resource::<DocumentSync>()
                .is_some_and(|sync| sync.seen.is_none())
            {
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
    if !world.contains_resource::<SceneBsnAst>() || !world.contains_resource::<DocumentSync>() {
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
    if !document_changed {
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
    seen.retry.clear();

    let projected = projected_entities(world);
    let mut references: Option<AssetReferences> = None;
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
        let references = references.get_or_insert_with(|| AssetReferences::of(world));
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
        for path in set_paths {
            match component_value(world, &registry, references, entity, node, path) {
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
                continue;
            }
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
            events.push(ControlEvent::NodeComponents {
                node: id,
                set,
                removed,
            });
        }
    }
    events
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
/// asset path, the `#Name` the loaded scene embeds it as, or the catalog's
/// `@Name`, the same references a scene save writes.
struct AssetReferences(HashMap<UntypedAssetId, String>);

impl AssetReferences {
    fn of(world: &World) -> Self {
        Self(crate::scene_io::save::asset_reference_seed(world))
    }

    fn reference(&self, handle: &UntypedHandle) -> Option<String> {
        handle
            .path()
            .map(ToString::to_string)
            .or_else(|| self.0.get(&handle.id()).cloned())
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
/// the editor registers, else the document's value for a project type.
fn component_value(
    world: &World,
    registry: &TypeRegistry,
    references: &AssetReferences,
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
        return game_value(value.as_partial_reflect(), registry, references);
    }
    let schema = world
        .get_resource::<ProjectTypes>()
        .and_then(|types| types.component(type_path))
        .ok_or_else(|| Unsent::whole("the type is not registered"))?;
    Ok(crate::inspector::project_component_display::project_component_json(world, node, schema))
}

/// Serialize a component for the game. An asset handle goes as the reference
/// the game's scene loader resolves (see [`AssetReferences`]), or `null` for
/// the type's default handle. A value holding a handle with no such
/// reference, or an entity reference, has no meaning in the game and is
/// refused, naming the field.
fn game_value(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    references: &AssetReferences,
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
    references: &AssetReferences,
    field: &mut String,
) -> Option<&'static str> {
    if let Some(value) = value.try_as_reflect() {
        let type_id = value.reflect_type_info().type_id();
        if let Some(reflect_handle) = registry.get_type_data::<ReflectHandle>(type_id) {
            let handle = reflect_handle.downcast_handle_untyped(value.as_any());
            let default = UntypedHandle::default_for_type(reflect_handle.asset_type_id());
            let sendable = handle.is_some_and(|handle| {
                handle.id() == default.id() || references.reference(&handle).is_some()
            });
            return (!sendable).then_some("holds an asset the game's scene does not name");
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
    references: &'a AssetReferences,
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
        let mut world = World::new();
        let registry = AppTypeRegistry::default();
        {
            let mut registry = registry.write();
            registry.register::<Transform>();
            registry.register::<SceneNodeId>();
        }
        world.insert_resource(registry.clone());
        world.init_resource::<DocumentSync>();
        let entity = world.spawn((SceneNodeId(NODE), Transform::default())).id();
        let patches = {
            let registry = registry.read();
            vec![
                component_to_bsn_patch(&SceneNodeId(NODE), &registry),
                component_to_bsn_patch(&Transform::default(), &registry),
            ]
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

    /// The translations the game end received, in order.
    fn received_x(game: &mut IpcChannelTransport) -> Vec<f64> {
        game.drain_received()
            .into_iter()
            .filter_map(|(_, bytes)| match from_bytes(&bytes) {
                Ok(ControlEvent::NodeComponents { node, set, .. }) if node == NODE => set
                    .into_iter()
                    .find(|(path, _)| path == TRANSFORM)
                    .and_then(|(_, value)| value["translation"][0].as_f64()),
                _ => None,
            })
            .collect()
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
        assert_eq!(received_x(&mut game_end), vec![6.0]);

        sync_document_to_game(&mut world);
        assert!(received_x(&mut game_end).is_empty(), "an edit goes once");
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
        assert_eq!(received_x(&mut game_end), vec![2.0]);
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
        assert!(received_x(&mut game_end).is_empty());

        move_to(&mut world, entity, 1.0);
        sync_document_to_game(&mut world);
        assert_eq!(received_x(&mut game_end), vec![1.0]);
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
