//! Projects the focused PIE instance's streamed state into the editor preview
//! ECS, so the Scene outliner and inspector show the live game's entities.
//! (Those entities also appear in the viewport because they are real preview
//! ECS entities; there is no separate viewport renderer for the running
//! game.)

use std::collections::{HashMap, HashSet};

use bevy::ecs::change_detection::Tick;
use bevy::ecs::component::ComponentId;
use bevy::prelude::*;
use bevy::reflect::serde::TypedReflectDeserializer;
use jackdaw_bsn::SceneBsnAst;
use jackdaw_pie_protocol::StateEvent;
use serde::de::{DeserializeSeed, IntoDeserializer};

/// Marker on a preview entity that exists only because the running game spawned
/// it (no authored AST node). Removed when play stops or the focus instance
/// changes; never serialized.
#[derive(Component, Debug, Clone, Copy)]
pub struct PieEphemeral;

/// Maps a streamed entity (game-side bits) to the preview entity representing
/// it: an authored entity resolved via `SceneNodeId`, or an ephemeral preview
/// entity this projector spawned. Cleared when play stops, focus changes, or
/// the active tab switches (tab activation respawns authored entities with
/// fresh ids, so the old map is stale).
#[derive(Resource, Default)]
pub struct PieProjection {
    pub by_bits: HashMap<u64, Entity>,
    /// Preview entities whose streamed parent has not been projected yet,
    /// keyed by the parent's game-side bits. Reparented when the parent's
    /// `EntitySpawned` arrives.
    pub pending_children: HashMap<u64, Vec<Entity>>,
    /// The parent each queued child last asked for. The game can move a child on
    /// while it waits, which leaves it queued under a parent it no longer names.
    waiting_on: HashMap<Entity, u64>,
    /// The game-side bits last mapped to each preview entity, so a respawn under
    /// new bits finds the old alias without scanning `by_bits`.
    bits_of: HashMap<Entity, u64>,
    /// Authored entities holding a streamed value their document does not, and
    /// not respawned since. They hold the game's state, so what an emit writes
    /// for them comes from the document alone.
    pub overlaid: HashSet<Entity>,
    /// The components of authored entities the stream wrote, by type path, with
    /// the change tick of that write. A later change is the editor's.
    streamed: HashMap<Entity, HashMap<String, Option<Tick>>>,
    /// References that arrived while an entity naming them held the game's values.
    relink_after: bevy::platform::collections::HashSet<String>,
    /// What authored entities held before the stream first overwrote a component,
    /// by type path, which a revert puts back.
    priors: HashMap<Entity, HashMap<String, KeptPrior>>,
    /// Authored roots the stream put under a parent, as the game parents a scene's
    /// roots under its own container. A revert makes them roots again.
    moved_roots: HashSet<Entity>,
    /// The stream did something only a respawn undoes: it moved an authored entity
    /// that had a parent, added a component and so whatever that component requires,
    /// or overwrote a value that cannot be copied.
    pub needs_respawn: bool,
}

impl PieProjection {
    /// Whether the change `entity`'s `type_path` holds at `last_changed` is the
    /// stream's own write.
    pub fn stream_wrote(&self, entity: Entity, type_path: &str, last_changed: Tick) -> bool {
        self.streamed
            .get(&entity)
            .and_then(|types| types.get(type_path))
            .is_some_and(|written| *written == Some(last_changed))
    }

    /// Link fields naming `names` once no entity holds the game's values.
    pub(crate) fn relink_after_overlays(
        &mut self,
        names: &bevy::platform::collections::HashSet<String>,
    ) {
        self.relink_after.extend(names.iter().cloned());
    }
}

/// Note that the stream just wrote `type_path` on `entity`. Call it straight after
/// the write: the component's change tick now is what identifies that write.
pub fn mark_streamed(world: &mut World, entity: Entity, type_path: &str) {
    let written = component_changed_tick(world, entity, type_path);
    if let Some(mut projection) = world.get_resource_mut::<PieProjection>() {
        projection.overlaid.insert(entity);
        projection
            .streamed
            .entry(entity)
            .or_default()
            .insert(type_path.to_string(), written);
    }
}

fn component_changed_tick(world: &World, entity: Entity, type_path: &str) -> Option<Tick> {
    let type_id = world
        .resource::<AppTypeRegistry>()
        .read()
        .get_with_type_path(type_path)?
        .type_id();
    let component = world.components().get_id(type_id)?;
    let ticks = world
        .get_entity(entity)
        .ok()?
        .get_change_ticks_by_id(component)?;
    Some(ticks.changed)
}

/// A value an authored entity held before the stream overwrote it.
struct KeptPrior {
    value: Box<dyn Reflect>,
    /// [`document_digest`] when the value was kept. The value only stands for the
    /// document while this still matches.
    document: Option<u64>,
}

/// What [`apply_component_value`] did to the entity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Nothing: the value did not apply, or the entity already held it.
    Nothing,
    /// The component's value changed.
    Changed,
    /// The entity gained the component.
    Added,
}

/// Type path of the hierarchy component naming an entity's parent. Streamed
/// values carry the parent as raw game-side entity bits, which mean nothing in
/// the editor world, so the projector remaps them instead of applying them.
const CHILD_OF_PATH: &str = "bevy_ecs::hierarchy::ChildOf";
/// Type path of the derived child-list component. Never applied: the editor
/// derives it from the remapped `ChildOf` inserts.
const CHILDREN_PATH: &str = "bevy_ecs::hierarchy::Children";

/// Component type-path prefixes never applied to preview entities. A streamed
/// camera would become an ACTIVE render camera in the editor world: the game
/// rig's camera fights the UI camera at the same order, and a render target
/// whose nulled asset handle resolves to the default image aborts the render
/// pass (the default image is not renderable). Current game builds do not send
/// these, but the projector refuses them regardless so an older game binary
/// cannot crash the editor.
const PROJECTION_SKIP_PREFIXES: &[&str] = &[
    "bevy_camera::camera::",
    "bevy_camera::components::",
    // A windowless game marks its own camera. Applied to an editor entity it
    // makes a second default UI camera, and the tie is broken by entity id, so
    // the editor's UI can move to an inactive camera and stop drawing.
    "bevy_ui::ui_node::IsDefaultUiCamera",
];

/// Streamed but never saved: these must reach the preview even though the
/// save filter rejects them. The graph playback is what the Graph window
/// lights its running state from while the game is the one playing.
const PROJECTION_ALLOW_PATHS: &[&str] =
    &["jackdaw_animation_runtime::graph::AnimationGraphPlayback"];

/// True when a streamed component must not be applied to a preview entity.
fn projection_skips(type_path: &str) -> bool {
    if PROJECTION_ALLOW_PATHS.contains(&type_path) {
        return false;
    }
    type_path == CHILDREN_PATH
        || crate::scene_io::should_skip_component(type_path)
        || PROJECTION_SKIP_PREFIXES
            .iter()
            .any(|prefix| type_path.starts_with(prefix))
}

/// `ChildOf` is a single-field struct over `Entity`, which the snapshot
/// serializer writes as the parent's `{index}v{generation}` text.
fn parent_bits_from_value(value: &serde_json::Value) -> Option<u64> {
    jackdaw_pie_protocol::snapshot::entity_bits(value)
}

fn reparent_preview(world: &mut World, child: Entity, parent: Entity) {
    if child == parent
        || world
            .get::<ChildOf>(child)
            .is_some_and(|held| held.0 == parent)
        || would_cycle(world, child, parent)
    {
        return;
    }
    let held_parent = world.get::<ChildOf>(child).is_some();
    if world.get::<PieEphemeral>(child).is_none()
        && let Some(mut projection) = world.get_resource_mut::<PieProjection>()
        && !projection.moved_roots.contains(&child)
    {
        if held_parent {
            projection.needs_respawn = true;
        } else {
            projection.moved_roots.insert(child);
        }
    }
    if let Ok(mut entity_mut) = world.get_entity_mut(child) {
        entity_mut.insert(ChildOf(parent));
    }
}

/// True when parenting `child` under `parent` would form a `ChildOf` cycle,
/// i.e. `parent` already sits under `child`. A streamed projection can briefly
/// request this while entities respawn and reparent; a cycle would hang every
/// per-frame hierarchy walk (transform propagation included), so the offending
/// edge is dropped and a later consistent update sets the real parent. The walk
/// terminates because the existing hierarchy is kept acyclic by this very
/// guard; the bound is a backstop against an already-corrupt tree.
fn would_cycle(world: &World, child: Entity, parent: Entity) -> bool {
    let mut ancestor = parent;
    for _ in 0..100_000 {
        if ancestor == child {
            return true;
        }
        match world.get::<ChildOf>(ancestor) {
            Some(child_of) => ancestor = child_of.0,
            None => return false,
        }
    }
    true
}

/// Resolve a streamed `ChildOf` value to the parent's preview entity and
/// insert the remapped component on `child`. If the parent has not been
/// projected yet, the child is queued and reparented when the parent spawns.
fn apply_child_of(world: &mut World, child: Entity, value: &serde_json::Value) {
    let Some(parent_bits) = parent_bits_from_value(value) else {
        debug!("apply_child_of: unrecognized ChildOf value: {value}");
        return;
    };
    let parent = world
        .resource::<PieProjection>()
        .by_bits
        .get(&parent_bits)
        .copied();
    let mut projection = world.resource_mut::<PieProjection>();
    match parent {
        Some(parent) => {
            projection.waiting_on.remove(&child);
            reparent_preview(world, child, parent);
        }
        None => {
            projection.waiting_on.insert(child, parent_bits);
            projection
                .pending_children
                .entry(parent_bits)
                .or_default()
                .push(child);
        }
    }
}

/// Parent to `parent` the children queued for `parent_bits` that still name it.
fn adopt_waiting_children(world: &mut World, parent_bits: u64, parent: Entity) {
    let mut projection = world.resource_mut::<PieProjection>();
    let Some(queued) = projection.pending_children.remove(&parent_bits) else {
        return;
    };
    let children: Vec<Entity> = queued
        .into_iter()
        .filter(|child| {
            let waiting = projection.waiting_on.get(child) == Some(&parent_bits);
            if waiting {
                projection.waiting_on.remove(child);
            }
            waiting
        })
        .collect();
    for child in children {
        reparent_preview(world, child, parent);
    }
}

/// Tear down all projected live state and restore the authored preview.
///
/// Despawns every ephemeral entity and clears the projection map. Each authored
/// entity the game changed takes back the values it held before those components
/// changed, which are what its document stands for while the document is unedited.
/// A large scene the game left alone then needs no respawn. What
/// [`PieProjection::needs_respawn`] names, an edit made live in the panels, a
/// document edit to a component or name the game also changed, a root the user
/// moved under another entity, or a value that could not be put back respawns the
/// scene the way a tab switch does.
///
/// The respawn captures the document first, so entities stay marked as holding
/// game state until it is done: the capture must not take their values for
/// authored ones. A capture that fails leaves them marked and sets
/// [`PieProjection::needs_respawn`], which the next revert retries and a switch of
/// the view to Live leaves alone.
pub fn revert_preview(world: &mut World) {
    let revert = world
        .get_resource_mut::<PieProjection>()
        .map(|mut projection| Revert {
            priors: std::mem::take(&mut projection.priors),
            moved_roots: std::mem::take(&mut projection.moved_roots),
            needs_respawn: std::mem::take(&mut projection.needs_respawn),
        })
        .unwrap_or_default();
    let root_reparented = revert
        .moved_roots
        .iter()
        .any(|&root| !is_document_root(world, root));
    for &root in &revert.moved_roots {
        if let Ok(mut entity_mut) = world.get_entity_mut(root) {
            entity_mut.remove::<ChildOf>();
        }
    }
    clear_projection(world);
    let live_edits = world
        .get_resource::<crate::live_edits::LiveEditLog>()
        .is_some_and(|log| !log.entries.is_empty());
    let document_edited = revert.priors.iter().any(|(&entity, values)| {
        values.iter().any(|(type_path, prior)| {
            world.get_entity(entity).is_ok()
                && document_digest(world, entity, type_path) != prior.document
        })
    });
    let mut respawn = revert.needs_respawn || live_edits || document_edited || root_reparented;
    for (entity, values) in revert.priors {
        for (type_path, prior) in values {
            respawn |= !restore_component(world, entity, &type_path, prior.value);
        }
    }
    if respawn && !crate::scenes::swap::respawn_scene_from_ast(world) {
        if let Some(mut projection) = world.get_resource_mut::<PieProjection>() {
            projection.needs_respawn = true;
        }
        return;
    }
    forget_overlays(world);
}

/// True when the document lists `entity`'s node as a scene root. An entity with no
/// node, or gone, is not one the user could have moved.
fn is_document_root(world: &World, entity: Entity) -> bool {
    let Some(ast) = world.get_resource::<SceneBsnAst>() else {
        return true;
    };
    ast.ast_for(entity)
        .is_none_or(|node| ast.roots.contains(&node))
}

/// What a revert has to undo.
#[derive(Default)]
struct Revert {
    priors: HashMap<Entity, HashMap<String, KeptPrior>>,
    moved_roots: HashSet<Entity>,
    needs_respawn: bool,
}

/// Identifies what the document says for `entity`'s component, if anything.
fn document_digest(world: &World, entity: Entity, type_path: &str) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let ast = world.get_resource::<SceneBsnAst>()?;
    let node = ast.ast_for(entity)?;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if type_path == <Name as bevy::reflect::TypePath>::type_path() {
        ast.get_name(node)?.hash(&mut hasher);
    } else {
        let patch = ast.find_patch_by_type_path(node, type_path)?;
        format!("{:?}", ast.get_patch(patch)?).hash(&mut hasher);
    }
    Some(hasher.finish())
}

/// Put back a value [`apply_streamed_value`] kept, whole, so a list the game grew
/// comes back at its old length. A component the entity has lost since, to an
/// undo say, stays gone. False when the entity holds the component but the value
/// could not be set on it.
fn restore_component(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    value: Box<dyn Reflect>,
) -> bool {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let Some(registration) = registry.get_with_type_path(type_path) else {
        return true;
    };
    let Some(reflect_component) = registration.data::<ReflectComponent>() else {
        return true;
    };
    let is_mutable = component_is_mutable(world, registration.type_id());
    let Ok(entity_ref) = world.get_entity(entity) else {
        return true;
    };
    if !reflect_component.contains(entity_ref) {
        return true;
    }
    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        return true;
    };
    if !is_mutable {
        reflect_component.insert(&mut entity_mut, value.as_partial_reflect(), &registry);
        return true;
    }
    if let Some(mut held) = reflect_component.reflect_mut(&mut entity_mut)
        && held.set(value).is_err()
    {
        debug!("revert_preview: the value kept for {type_path} is of another type");
        return false;
    }
    true
}

fn component_is_mutable(world: &World, type_id: std::any::TypeId) -> bool {
    world
        .components()
        .get_id(type_id)
        .and_then(|id: ComponentId| world.components().get_info(id))
        .map(bevy::ecs::component::ComponentInfo::mutable)
        .unwrap_or(true)
}

/// The authored entities were respawned, so none holds a streamed value.
fn forget_overlays(world: &mut World) {
    let Some(mut projection) = world.get_resource_mut::<PieProjection>() else {
        return;
    };
    projection.overlaid.clear();
    projection.streamed.clear();
    projection.priors.clear();
    projection.moved_roots.clear();
    projection.needs_respawn = false;
    let names = std::mem::take(&mut projection.relink_after);
    if !names.is_empty() {
        crate::scene_io::asset_names::relink_references(world, &names);
    }
}

/// Despawn every ephemeral preview entity and clear the projection maps,
/// without touching the authored scene.
fn clear_projection(world: &mut World) {
    let ephemerals: Vec<Entity> = {
        let mut q = world.query_filtered::<Entity, With<PieEphemeral>>();
        q.iter(world).collect()
    };
    for e in ephemerals {
        // The game routinely parents authored previews under ephemerals (zone
        // roots, network actors). `despawn` takes descendants with it, so detach
        // the authored ones first or they die with the container and their node
        // ids resolve to dead entities on the next replay.
        detach_authored_descendants(world, e);
        if let Ok(em) = world.get_entity_mut(e) {
            em.despawn();
        }
    }
    // Absent when the PIE plugin isn't registered (headless harnesses,
    // editors built without PIE); there is no projection to clear then.
    let Some(mut projection) = world.get_resource_mut::<PieProjection>() else {
        return;
    };
    projection.by_bits.clear();
    projection.pending_children.clear();
    projection.waiting_on.clear();
    projection.bits_of.clear();
    forget_dead_selection(world);
}

/// Drop despawned entities from the selection. A respawn clears it anyway, but a
/// stream despawning an ephemeral or a revert without one does not.
fn forget_dead_selection(world: &mut World) {
    let Some(selection) = world.get_resource::<crate::selection::Selection>() else {
        return;
    };
    let alive: Vec<Entity> = selection
        .entities
        .iter()
        .copied()
        .filter(|&entity| world.get_entity(entity).is_ok())
        .collect();
    if alive.len() != selection.entities.len() {
        world.resource_mut::<crate::selection::Selection>().entities = alive;
    }
}

/// Replay an instance's full buffered snapshot into the preview world.
fn replay_buffer(world: &mut World, key: &crate::pie::InstanceKey) {
    let snapshot: Vec<jackdaw_pie_protocol::RemoteEntity> = world
        .resource::<crate::pie_mirror::PieInstances>()
        .buffers
        .get(key)
        .map(|b| {
            b.entities
                .iter()
                .map(|(bits, entry)| jackdaw_pie_protocol::RemoteEntity {
                    entity: *bits,
                    components: entry.components.clone(),
                    scene_node_id: entry.scene_node_id,
                })
                .collect()
        })
        .unwrap_or_default();
    for entity in snapshot {
        project_event(
            world,
            jackdaw_pie_protocol::StateEvent::EntitySpawned { entity },
        );
    }
}

/// Rebuild the live projection after a tab activation. Activating a tab
/// respawns the authored scene with fresh entity ids, so the projection map
/// and any surviving ephemerals from the previous tab are stale; without this,
/// every switch leaks the previous projection into the new tab. Clears the
/// stale state, then (in Live view, with a focused running instance) replays
/// the focused buffer against the new tab's AST bindings. Safe no-op when no
/// game is running.
pub fn reproject_focused(world: &mut World) {
    clear_projection(world);
    forget_overlays(world);
    replay_focused(world);
}

/// Project the focused instance again without a respawn, as when the view
/// switches to Live. Authored entities keep the streamed values and the prior
/// values kept for them, so what a revert owes is not lost.
pub fn reproject_view(world: &mut World) {
    clear_projection(world);
    replay_focused(world);
}

fn replay_focused(world: &mut World) {
    let live = world
        .get_resource::<crate::pie_mirror::PieViewMode>()
        .is_some_and(|mode| *mode == crate::pie_mirror::PieViewMode::Live);
    if !live {
        return;
    }
    let focused = world
        .get_resource::<crate::pie_mirror::PieInstances>()
        .and_then(|instances| instances.focused.clone());
    if let Some(key) = focused {
        replay_buffer(world, &key);
    }
}

/// Switch the focused instance: revert the current projection, set the new
/// focus, and re-project the new instance's full buffered snapshot into the
/// preview world. A no-op if `key` is already focused.
pub fn set_focused_instance(world: &mut World, key: crate::pie::InstanceKey) {
    if world
        .resource::<crate::pie_mirror::PieInstances>()
        .focused
        .as_ref()
        == Some(&key)
    {
        return;
    }
    // The old focus keeps streaming frames nobody will consume; stop it
    // before the focus moves.
    crate::pie::send_control_to_focused(world, jackdaw_pie_protocol::ControlEvent::StopFrameStream);
    revert_preview(world);
    crate::live_frame::clear_stream(world);
    // The stop prompt owns the log while it is open.
    let prompt_open = world
        .get_resource::<crate::live_edits_ui::StopPrompt>()
        .is_some_and(|prompt| prompt.0);
    if !prompt_open
        && let Some(mut log) = world.get_resource_mut::<crate::live_edits::LiveEditLog>()
    {
        log.entries.clear();
        log.pending_action = None;
    }
    world
        .resource_mut::<crate::pie_mirror::PieInstances>()
        .focused = Some(key.clone());
    replay_buffer(world, &key);
}

/// Apply one streamed event to the preview world.
pub fn project_event(world: &mut World, event: StateEvent) {
    match event {
        StateEvent::EntitySpawned { entity } => {
            let bits = entity.entity;
            let authored = entity
                .scene_node_id
                .and_then(|id| world.resource::<SceneBsnAst>().entity_for_stable_id(id));
            let preview = authored.unwrap_or_else(|| {
                // Seed Transform/Visibility so children inherit before
                // streamed values overwrite the defaults.
                world
                    .spawn((PieEphemeral, Transform::default(), Visibility::default()))
                    .id()
            });
            // A zone hot-reload respawns an authored node under new bits and
            // sends the new spawn before the old despawn, so drop the bits that
            // still alias this preview entity. Otherwise a reverse lookup could
            // resolve dead bits.
            {
                let mut projection = world.resource_mut::<PieProjection>();
                projection.by_bits.insert(bits, preview);
                if let Some(old) = projection.bits_of.insert(preview, bits)
                    && old != bits
                    && projection.by_bits.get(&old) == Some(&preview)
                {
                    projection.by_bits.remove(&old);
                }
            }
            // Hierarchy is remapped, not applied: stash ChildOf for after the
            // value components land.
            let mut child_of: Option<serde_json::Value> = None;
            for (type_path, value) in entity.components {
                if type_path == CHILD_OF_PATH {
                    child_of = Some(value);
                    continue;
                }
                if projection_skips(&type_path) {
                    continue;
                }
                apply_streamed_value(world, preview, type_path, &value);
            }
            if let Some(value) = child_of {
                apply_child_of(world, preview, &value);
            }
            // This entity may be the parent earlier children were waiting for.
            adopt_waiting_children(world, bits, preview);
        }
        StateEvent::ComponentChanged {
            entity,
            type_path,
            value,
        } => {
            if projection_skips(&type_path) {
                return;
            }
            if let Some(preview) = world
                .resource::<PieProjection>()
                .by_bits
                .get(&entity)
                .copied()
            {
                if type_path == CHILD_OF_PATH {
                    apply_child_of(world, preview, &value);
                } else {
                    apply_streamed_value(world, preview, type_path, &value);
                }
            }
        }
        StateEvent::EntityDespawned { entity } => {
            let preview = world
                .resource_mut::<PieProjection>()
                .by_bits
                .remove(&entity);
            // Children queued for a parent that will never spawn again.
            {
                let mut projection = world.resource_mut::<PieProjection>();
                let queued = projection.pending_children.remove(&entity);
                for child in queued.into_iter().flatten() {
                    if projection.waiting_on.get(&child) == Some(&entity) {
                        projection.waiting_on.remove(&child);
                    }
                }
            }
            if let Some(preview) = preview {
                let mut projection = world.resource_mut::<PieProjection>();
                if projection.bits_of.get(&preview) == Some(&entity) {
                    projection.bits_of.remove(&preview);
                }
                // Only despawn entities this projector created; authored
                // entities are reverted on stop, not despawned here.
                if world.get::<PieEphemeral>(preview).is_some() {
                    detach_authored_descendants(world, preview);
                    if let Ok(e) = world.get_entity_mut(preview) {
                        e.despawn();
                    }
                    forget_dead_selection(world);
                }
            }
        }
        StateEvent::CursorState { .. }
        | StateEvent::RequestScene { .. }
        | StateEvent::SceneRequestAbandoned { .. }
        | StateEvent::OwnWindow => {}
    }
}

/// Apply a value from the stream, and note on an authored entity what a revert
/// has to undo.
fn apply_streamed_value(
    world: &mut World,
    preview: Entity,
    type_path: String,
    value: &serde_json::Value,
) {
    let authored = world.get::<PieEphemeral>(preview).is_none();
    let keep_prior = authored
        && !world
            .resource::<PieProjection>()
            .priors
            .get(&preview)
            .is_some_and(|values| values.contains_key(&type_path));
    let (applied, prior) = apply_value(world, preview, &type_path, value, keep_prior);
    if applied == Applied::Nothing || !authored {
        return;
    }
    let kept = matches!(prior, Prior::Kept(_));
    let document = if kept {
        document_digest(world, preview, &type_path)
    } else {
        None
    };
    let mid_edit = kept && edit_in_flight(world);
    mark_streamed(world, preview, &type_path);
    let mut projection = world.resource_mut::<PieProjection>();
    if applied == Applied::Added {
        projection.needs_respawn = true;
        return;
    }
    match prior {
        Prior::CannotCopy => projection.needs_respawn = true,
        Prior::Kept(value) => {
            projection.needs_respawn |= mid_edit;
            projection
                .priors
                .entry(preview)
                .or_default()
                .insert(type_path, KeptPrior { value, document });
        }
        Prior::NotKept => {}
    }
}

/// True while a modal operator runs or document patches wait to be applied, when
/// an entity can hold a value its document does not.
fn edit_in_flight(world: &mut World) -> bool {
    use jackdaw_api_internal::lifecycle::ActiveModalOperator;
    world
        .query_filtered::<(), With<ActiveModalOperator>>()
        .iter(world)
        .next()
        .is_some()
        || world
            .query_filtered::<(), With<jackdaw_bsn::AstDirty>>()
            .iter(world)
            .next()
            .is_some()
}

/// Rescue authored previews from an ephemeral subtree about to be despawned.
///
/// `despawn` takes descendants with it, and the game routinely reparents
/// authored previews under ephemerals (zone roots, carried items). Detaching
/// each non-ephemeral child keeps the authored entity alive (its own subtree
/// goes with it); ephemeral children die with the parent, but their authored
/// descendants are rescued first.
fn detach_authored_descendants(world: &mut World, parent: Entity) {
    detach_authored_descendants_guarded(world, parent, &mut std::collections::HashSet::new());
}

/// Walk the ephemeral subtree, detaching authored descendants. The `seen` set
/// guards against a parent cycle, which a streamed projection can momentarily
/// form while entities respawn and reparent; without it the recursion would
/// run forever.
fn detach_authored_descendants_guarded(
    world: &mut World,
    parent: Entity,
    seen: &mut std::collections::HashSet<Entity>,
) {
    if !seen.insert(parent) {
        return;
    }
    let children: Vec<Entity> = world
        .get::<Children>(parent)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    for child in children {
        if world.get::<PieEphemeral>(child).is_some() {
            detach_authored_descendants_guarded(world, child, seen);
        } else if let Ok(mut entity_mut) = world.get_entity_mut(child) {
            entity_mut.remove::<ChildOf>();
        }
    }
}

/// Apply a streamed component value to a preview entity, choosing the operation
/// by the component's mutability. A mutable component is mutated in place (no
/// `on_add`/`on_insert`; `Changed` fires). An immutable component is re-inserted
/// so its value changes and its hooks run. No-op (debug log) if the type is not
/// registered, has no `ReflectComponent`, or the value does not deserialize, and
/// silently when the entity already holds this value, so a game streaming the
/// scene it was given dirties nothing in the editor.
pub fn apply_component_value(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    value: &serde_json::Value,
) -> Applied {
    apply_value(world, entity, type_path, value, false).0
}

/// The value a write replaced.
enum Prior {
    NotKept,
    Kept(Box<dyn Reflect>),
    CannotCopy,
}

/// [`apply_component_value`], also returning the value it overwrote when asked
/// to keep it.
fn apply_value(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    value: &serde_json::Value,
    keep_prior: bool,
) -> (Applied, Prior) {
    let nothing = (Applied::Nothing, Prior::NotKept);
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry_guard = registry.read();

    let Some(registration) = registry_guard.get_with_type_path(type_path) else {
        debug!("apply_component_value: type not registered: {type_path}");
        return nothing;
    };

    let Some(reflect_component) = registration.data::<ReflectComponent>().cloned() else {
        debug!("apply_component_value: no ReflectComponent for: {type_path}");
        return nothing;
    };

    let mut processor = jackdaw_pie_protocol::RemoteDeserializerProcessor;
    let reflect_deserializer =
        TypedReflectDeserializer::with_processor(registration, &registry_guard, &mut processor);
    let reflected = match reflect_deserializer.deserialize(value.clone().into_deserializer()) {
        Ok(v) => v,
        Err(e) => {
            debug!("apply_component_value: deserialization failed for {type_path}: {e}");
            return nothing;
        }
    };

    let type_id = registration.type_id();

    let is_mutable = component_is_mutable(world, type_id);

    // Check presence while we still have shared world access (before the &mut borrow).
    let present = match world.get_entity(entity) {
        Ok(entity_ref) => reflect_component.contains(entity_ref),
        Err(_) => {
            debug!("apply_component_value: entity {entity:?} does not exist");
            return nothing;
        }
    };

    // A stream never carries a handle, so the value's are all defaults; the
    // entity's own stay.
    let mut reflected = reflected;
    let mut prior = Prior::NotKept;
    if present
        && let Ok(entity_ref) = world.get_entity(entity)
        && let Some(held) = reflect_component.reflect(entity_ref)
    {
        keep_held_handles(
            reflected.as_mut(),
            held.as_partial_reflect(),
            &registry_guard,
            KeepHandles::All,
        );
        if held.reflect_partial_eq(reflected.as_partial_reflect()) == Some(true) {
            return nothing;
        }
        if keep_prior {
            prior = match held.reflect_clone() {
                Ok(value) => Prior::Kept(value),
                Err(_) => Prior::CannotCopy,
            };
        }
    }

    drop(registry_guard);

    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        debug!("apply_component_value: entity {entity:?} was despawned concurrently");
        return nothing;
    };

    if !is_mutable || !present {
        let registry_guard = registry.read();
        reflect_component.insert(
            &mut entity_mut,
            reflected.as_partial_reflect(),
            &registry_guard,
        );
    } else {
        reflect_component.apply(entity_mut, reflected.as_partial_reflect());
    }
    let applied = if present {
        Applied::Changed
    } else {
        Applied::Added
    };
    (applied, prior)
}

/// Put back on `entity`'s component the handles of `held` where it now holds a
/// cleared one. Whether any changed.
pub(crate) fn restore_cleared_handles(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    held: &dyn Reflect,
) -> bool {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let Some(reflect_component) = registry
        .get_with_type_path(type_path)
        .and_then(|registration| registration.data::<ReflectComponent>())
    else {
        return false;
    };
    let Some(mut restored) = world
        .get_entity(entity)
        .ok()
        .and_then(|now| reflect_component.reflect(now))
        .and_then(|now| now.reflect_clone().ok())
    else {
        return false;
    };
    let changed = keep_held_handles(
        restored.as_partial_reflect_mut(),
        held.as_partial_reflect(),
        &registry,
        KeepHandles::OverCleared,
    );
    if changed && let Ok(mut entity_mut) = world.get_entity_mut(entity) {
        reflect_component.insert(&mut entity_mut, restored.as_partial_reflect(), &registry);
    }
    changed
}

/// Which handles of `value` [`keep_held_handles`] writes over.
#[derive(Clone, Copy, PartialEq)]
enum KeepHandles {
    All,
    OverCleared,
}

/// Copy every asset handle in `held` over the same field of `value`. Fields
/// that do not line up, such as a list of another length, are left. Whether any
/// handle was written.
fn keep_held_handles(
    value: &mut dyn PartialReflect,
    held: &dyn PartialReflect,
    registry: &bevy::reflect::TypeRegistry,
    keep: KeepHandles,
) -> bool {
    use bevy::reflect::{ReflectMut, ReflectRef};
    let handle_type = held.try_as_reflect().and_then(|held| {
        registry.get_type_data::<bevy::asset::ReflectHandle>(held.reflect_type_info().type_id())
    });
    if let Some(handle_type) = handle_type {
        if keep == KeepHandles::OverCleared {
            let cleared = bevy::asset::UntypedHandle::default_for_type(handle_type.asset_type_id());
            let id_of = |of: &dyn PartialReflect| {
                of.try_as_reflect()
                    .and_then(|of| handle_type.downcast_handle_untyped(of.as_any()))
                    .map(|handle| handle.id())
            };
            if id_of(value) != Some(cleared.id()) || id_of(held) == Some(cleared.id()) {
                return false;
            }
        }
        return match value.try_apply(held) {
            Ok(()) => true,
            Err(err) => {
                debug!("apply_component_value: could not keep a handle: {err}");
                false
            }
        };
    }
    let mut kept = false;
    match (value.reflect_mut(), held.reflect_ref()) {
        (ReflectMut::Struct(value), ReflectRef::Struct(held)) => {
            for index in 0..held.field_len() {
                if let (Some(name), Some(field)) = (held.name_at(index), held.field_at(index))
                    && let Some(slot) = value.field_mut(name)
                {
                    kept |= keep_held_handles(slot, field, registry, keep);
                }
            }
        }
        (ReflectMut::TupleStruct(value), ReflectRef::TupleStruct(held)) => {
            for index in 0..held.field_len() {
                if let (Some(slot), Some(field)) = (value.field_mut(index), held.field(index)) {
                    kept |= keep_held_handles(slot, field, registry, keep);
                }
            }
        }
        (ReflectMut::Tuple(value), ReflectRef::Tuple(held)) => {
            for index in 0..held.field_len() {
                if let (Some(slot), Some(field)) = (value.field_mut(index), held.field(index)) {
                    kept |= keep_held_handles(slot, field, registry, keep);
                }
            }
        }
        (ReflectMut::List(value), ReflectRef::List(held)) if value.len() == held.len() => {
            for index in 0..held.len() {
                if let (Some(slot), Some(item)) = (value.get_mut(index), held.get(index)) {
                    kept |= keep_held_handles(slot, item, registry, keep);
                }
            }
        }
        (ReflectMut::Array(value), ReflectRef::Array(held)) if value.len() == held.len() => {
            for index in 0..held.len() {
                if let (Some(slot), Some(item)) = (value.get_mut(index), held.get(index)) {
                    kept |= keep_held_handles(slot, item, registry, keep);
                }
            }
        }
        (ReflectMut::Enum(value), ReflectRef::Enum(held))
            if value.variant_name() == held.variant_name() =>
        {
            for index in 0..held.field_len() {
                if let (Some(slot), Some(field)) = (value.field_at_mut(index), held.field_at(index))
                {
                    kept |= keep_held_handles(slot, field, registry, keep);
                }
            }
        }
        _ => {}
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reparent_preview_refuses_a_cycle() {
        let mut world = World::new();
        // a -> b -> c chain (c child of b, b child of a).
        let a = world.spawn_empty().id();
        let b = world.spawn(ChildOf(a)).id();
        let c = world.spawn(ChildOf(b)).id();
        // Reparenting `a` under `c` would close the cycle a -> b -> c -> a.
        reparent_preview(&mut world, a, c);
        assert!(
            world.get::<ChildOf>(a).is_none(),
            "the cycle-forming reparent is dropped, leaving `a` a root"
        );
        // A non-cyclic reparent still applies.
        let d = world.spawn_empty().id();
        reparent_preview(&mut world, d, c);
        assert_eq!(world.get::<ChildOf>(d).map(|p| p.0), Some(c));
    }

    #[derive(Component, Reflect, Default, PartialEq, Debug)]
    #[reflect(Component)]
    #[type_path = "pie_projection_tests"]
    struct Mutable(i32);

    #[derive(Component, Reflect, Default, PartialEq, Debug)]
    #[reflect(Component)]
    #[component(immutable)]
    #[type_path = "pie_projection_tests"]
    struct Frozen(i32);

    /// A live brush projected from a running game must keep its geometry through
    /// the snapshot serialize -> deserialize -> apply/insert round trip. A mapped
    /// (authored) entity takes the `apply` path; an ephemeral one takes `insert`.
    /// Either way the topology rings must stay >= 3 verts, otherwise
    /// `regenerate_brush_meshes` spawns no face meshes and the brush shows as bare
    /// wireframe (the symptom observed in the PIE Live view).
    #[test]
    fn brush_geometry_survives_pie_round_trip() {
        use jackdaw_scene_types::Brush;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.add_plugins(jackdaw_scene_types::SceneTypesPlugin::default());

        let cuboid = Brush::cuboid(1.0, 1.0, 1.0);
        let want_faces = cuboid.faces.len();
        let want_polys = cuboid.topology.polygons.len();
        let src = app.world_mut().spawn(cuboid).id();

        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let snap = {
            let reg = registry.read();
            jackdaw_pie_protocol::build_snapshot(app.world(), &reg, &[src])
        };
        let (brush_key, brush_json) = snap[0]
            .components
            .iter()
            .find(|(k, _)| k.ends_with("::Brush"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .expect("Brush serialized into the snapshot");

        // Ephemeral projection: fresh entity, no Brush yet -> insert path.
        let ephemeral = app.world_mut().spawn_empty().id();
        apply_component_value(app.world_mut(), ephemeral, &brush_key, &brush_json);

        // Mapped projection: authored entity already carries the brush -> apply path.
        let mapped = app.world_mut().spawn(Brush::cuboid(1.0, 1.0, 1.0)).id();
        apply_component_value(app.world_mut(), mapped, &brush_key, &brush_json);

        for (label, e) in [("insert(ephemeral)", ephemeral), ("apply(mapped)", mapped)] {
            let b = app
                .world()
                .entity(e)
                .get::<Brush>()
                .unwrap_or_else(|| panic!("{label}: Brush component missing after projection"));
            assert_eq!(b.faces.len(), want_faces, "{label}: face count changed");
            assert_eq!(
                b.topology.polygons.len(),
                want_polys,
                "{label}: topology polygon count changed"
            );
            for i in 0..b.topology.polygons.len() {
                let ring = b.topology.face_ring(i).count();
                assert!(
                    ring >= 3,
                    "{label}: polygon {i} ring degenerated to {ring} verts (no face mesh -> wireframe)"
                );
            }
        }
    }

    #[derive(Resource, Default)]
    struct Inserts(u32);

    fn build_world() -> World {
        let mut world = World::new();
        world.init_resource::<AppTypeRegistry>();
        world.init_resource::<Inserts>();
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            let mut w = registry.write();
            w.register::<Mutable>();
            w.register::<Frozen>();
        }
        world.add_observer(|_: On<Insert<Mutable>>, mut n: ResMut<Inserts>| n.0 += 1);
        world.add_observer(|_: On<Insert<Frozen>>, mut n: ResMut<Inserts>| n.0 += 1);
        world
    }

    #[test]
    fn mutable_value_edit_does_not_reinsert() {
        let mut world = build_world();
        let e = world.spawn(Mutable(0)).id();
        let before = world.resource::<Inserts>().0;
        apply_component_value(
            &mut world,
            e,
            <Mutable as bevy::reflect::TypePath>::type_path(),
            &serde_json::json!(7),
        );
        assert_eq!(
            world.resource::<Inserts>().0,
            before,
            "mutable edit must not re-insert"
        );
        assert_eq!(world.entity(e).get::<Mutable>(), Some(&Mutable(7)));
    }

    #[test]
    fn absent_component_is_inserted_not_panicked() {
        let mut world = build_world();
        let e = world.spawn_empty().id();
        // Streamed value for a component the entity does not have: must insert, not panic.
        apply_component_value(
            &mut world,
            e,
            <Mutable as bevy::reflect::TypePath>::type_path(),
            &serde_json::json!(7),
        );
        assert_eq!(world.entity(e).get::<Mutable>(), Some(&Mutable(7)));
    }

    #[test]
    fn stale_entity_is_a_noop_not_a_panic() {
        let mut world = build_world();
        let e = world.spawn(Mutable(0)).id();
        world.entity_mut(e).despawn();
        // Must not panic on a despawned entity handle.
        apply_component_value(
            &mut world,
            e,
            <Mutable as bevy::reflect::TypePath>::type_path(),
            &serde_json::json!(7),
        );
    }

    #[test]
    fn immutable_value_edit_reinserts() {
        let mut world = build_world();
        let e = world.spawn(Frozen(0)).id();
        let before = world.resource::<Inserts>().0;
        apply_component_value(
            &mut world,
            e,
            <Frozen as bevy::reflect::TypePath>::type_path(),
            &serde_json::json!(7),
        );
        assert_eq!(
            world.resource::<Inserts>().0,
            before + 1,
            "immutable edit must re-insert"
        );
        assert_eq!(world.entity(e).get::<Frozen>(), Some(&Frozen(7)));
    }

    // ---- project_event integration tests ----

    use jackdaw_pie_protocol::StateEvent;
    use jackdaw_pie_protocol::snapshot::RemoteEntity;
    use jackdaw_scene_types::{SCENE_NODE_ID_TYPE_PATH, SceneNodeId};

    fn build_projection_world() -> (World, Entity, SceneNodeId) {
        let mut world = build_world();
        world.init_resource::<PieProjection>();

        // Build a document with one authored node bound to a preview entity.
        let preview_entity = world.spawn(Mutable(0)).id();
        let node_id = SceneNodeId::next();
        let mut ast = SceneBsnAst::default();
        let node = ast.create_entity_node(vec![jackdaw_bsn::BsnPatch::TupleStruct(
            jackdaw_bsn::BsnTupleStructData {
                type_path: SCENE_NODE_ID_TYPE_PATH.to_string(),
                values: vec![jackdaw_bsn::BsnValue::Int(node_id.0 as i128)],
            },
        )]);
        ast.add_to_roots(node);
        ast.link(preview_entity, node);
        world.insert_resource(ast);

        (world, preview_entity, node_id)
    }

    #[test]
    fn entity_spawned_authored_maps_to_existing_preview_entity() {
        let (mut world, preview_entity, node_id) = build_projection_world();

        let mut components = std::collections::HashMap::new();
        components.insert(
            <Mutable as bevy::reflect::TypePath>::type_path().to_string(),
            serde_json::json!(42),
        );

        project_event(
            &mut world,
            StateEvent::EntitySpawned {
                entity: RemoteEntity {
                    entity: 1,
                    components,
                    scene_node_id: Some(node_id.0),
                },
            },
        );

        // The authored preview entity should have the component value applied.
        assert_eq!(
            world.entity(preview_entity).get::<Mutable>(),
            Some(&Mutable(42)),
            "authored preview entity should receive the streamed component"
        );

        // No new entity should have been spawned with PieEphemeral.
        let ephemeral_count = world.query::<&PieEphemeral>().iter(&world).count();
        assert_eq!(
            ephemeral_count, 0,
            "authored overlay must not create an ephemeral entity"
        );

        // The bits->entity mapping should point at the existing preview entity.
        let mapped = world.resource::<PieProjection>().by_bits.get(&1).copied();
        assert_eq!(mapped, Some(preview_entity));
    }

    #[test]
    fn entity_spawned_no_scene_node_id_creates_ephemeral() {
        let (mut world, _preview_entity, _node_id) = build_projection_world();

        let mut components = std::collections::HashMap::new();
        components.insert(
            <Mutable as bevy::reflect::TypePath>::type_path().to_string(),
            serde_json::json!(7),
        );

        project_event(
            &mut world,
            StateEvent::EntitySpawned {
                entity: RemoteEntity {
                    entity: 2,
                    components,
                    scene_node_id: None,
                },
            },
        );

        // A new ephemeral entity should exist with the component.
        let ephemerals: Vec<Entity> = world
            .query::<(Entity, &PieEphemeral)>()
            .iter(&world)
            .map(|(e, _)| e)
            .collect();
        assert_eq!(
            ephemerals.len(),
            1,
            "one ephemeral entity should be spawned"
        );

        let ephemeral = ephemerals[0];
        assert_eq!(
            world.entity(ephemeral).get::<Mutable>(),
            Some(&Mutable(7)),
            "ephemeral entity should have the streamed component"
        );

        let mapped = world.resource::<PieProjection>().by_bits.get(&2).copied();
        assert_eq!(mapped, Some(ephemeral));
    }

    #[test]
    fn entity_despawned_removes_ephemeral_and_leaves_authored() {
        let (mut world, preview_entity, node_id) = build_projection_world();

        // Spawn the authored entity (bits=1).
        project_event(
            &mut world,
            StateEvent::EntitySpawned {
                entity: RemoteEntity {
                    entity: 1,
                    components: std::collections::HashMap::new(),
                    scene_node_id: Some(node_id.0),
                },
            },
        );

        // Spawn an ephemeral entity (bits=2).
        project_event(
            &mut world,
            StateEvent::EntitySpawned {
                entity: RemoteEntity {
                    entity: 2,
                    components: std::collections::HashMap::new(),
                    scene_node_id: None,
                },
            },
        );

        let ephemeral = world
            .resource::<PieProjection>()
            .by_bits
            .get(&2)
            .copied()
            .expect("ephemeral should be tracked");

        // Despawn the ephemeral.
        project_event(&mut world, StateEvent::EntityDespawned { entity: 2 });

        // The ephemeral entity should no longer exist.
        assert!(
            world.get_entity(ephemeral).is_err(),
            "ephemeral entity should be despawned"
        );
        assert!(
            !world.resource::<PieProjection>().by_bits.contains_key(&2),
            "bits->entity entry for ephemeral should be removed"
        );

        // Despawn the authored entity.
        project_event(&mut world, StateEvent::EntityDespawned { entity: 1 });

        // The authored preview entity should still exist (we don't despawn it).
        assert!(
            world.get_entity(preview_entity).is_ok(),
            "authored preview entity must not be despawned by the projector"
        );
        assert!(
            !world.resource::<PieProjection>().by_bits.contains_key(&1),
            "bits->entity entry for authored entity should be removed"
        );
    }

    // ---- ChildOf remapping tests ----

    /// Serialize a real parent/child pair through the snapshot pipeline and
    /// return (parent bits, child bits, `ChildOf` type path, `ChildOf` wire value)
    /// so the tests exercise the exact format the game sends.
    fn wire_child_of() -> (u64, u64, String, serde_json::Value) {
        use bevy::prelude::ChildOf;
        let mut app = App::new();
        app.register_type::<ChildOf>();
        let parent = app.world_mut().spawn_empty().id();
        let child = app.world_mut().spawn(ChildOf(parent)).id();
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let reg = registry.read();
        let snap = jackdaw_pie_protocol::build_snapshot(app.world(), &reg, &[child]);
        let (path, value) = snap[0]
            .components
            .iter()
            .find(|(k, _)| k.ends_with("::ChildOf"))
            .map(|(k, v)| (k.clone(), v.clone()))
            .expect("ChildOf serialized into the snapshot");
        (parent.to_bits(), child.to_bits(), path, value)
    }

    fn spawn_event(bits: u64, components: Vec<(String, serde_json::Value)>) -> StateEvent {
        StateEvent::EntitySpawned {
            entity: RemoteEntity {
                entity: bits,
                components: components.into_iter().collect(),
                scene_node_id: None,
            },
        }
    }

    #[test]
    fn child_of_remaps_to_projected_parent() {
        use bevy::prelude::ChildOf;
        let (mut world, _preview, _node_id) = build_projection_world();
        let (parent_bits, child_bits, path, value) = wire_child_of();

        project_event(&mut world, spawn_event(parent_bits, vec![]));
        project_event(&mut world, spawn_event(child_bits, vec![(path, value)]));

        let proj = world.resource::<PieProjection>();
        let parent_preview = proj.by_bits[&parent_bits];
        let child_preview = proj.by_bits[&child_bits];
        assert_eq!(
            world.entity(child_preview).get::<ChildOf>().map(|c| c.0),
            Some(parent_preview),
            "streamed ChildOf must point at the parent's preview entity, not raw game bits"
        );
    }

    #[test]
    fn child_of_pends_until_parent_spawns() {
        use bevy::prelude::ChildOf;
        let (mut world, _preview, _node_id) = build_projection_world();
        let (parent_bits, child_bits, path, value) = wire_child_of();

        // Child arrives first: parent bits are unknown, so the child must wait
        // unparented (no garbage ChildOf insert).
        project_event(&mut world, spawn_event(child_bits, vec![(path, value)]));
        let child_preview = world.resource::<PieProjection>().by_bits[&child_bits];
        assert_eq!(world.entity(child_preview).get::<ChildOf>(), None);

        project_event(&mut world, spawn_event(parent_bits, vec![]));
        let parent_preview = world.resource::<PieProjection>().by_bits[&parent_bits];
        assert_eq!(
            world.entity(child_preview).get::<ChildOf>().map(|c| c.0),
            Some(parent_preview),
            "queued child must be reparented when its parent's spawn arrives"
        );
    }

    #[test]
    fn component_changed_child_of_reparents() {
        use bevy::prelude::ChildOf;
        let (mut world, _preview, _node_id) = build_projection_world();
        let (parent_bits, child_bits, path, value) = wire_child_of();

        project_event(&mut world, spawn_event(parent_bits, vec![]));
        project_event(&mut world, spawn_event(child_bits, vec![]));
        project_event(
            &mut world,
            StateEvent::ComponentChanged {
                entity: child_bits,
                type_path: path,
                value,
            },
        );

        let proj = world.resource::<PieProjection>();
        let parent_preview = proj.by_bits[&parent_bits];
        let child_preview = proj.by_bits[&child_bits];
        assert_eq!(
            world.entity(child_preview).get::<ChildOf>().map(|c| c.0),
            Some(parent_preview)
        );
    }

    fn spawn_event_with_node(bits: u64, node_id: u64) -> StateEvent {
        StateEvent::EntitySpawned {
            entity: RemoteEntity {
                entity: bits,
                components: std::collections::HashMap::new(),
                scene_node_id: Some(node_id),
            },
        }
    }

    #[test]
    fn respawn_with_new_bits_purges_the_old_alias() {
        let (mut world, _preview, node_id) = build_projection_world();
        // A zone hot-reload respawns the same authored node under new game
        // bits; the new spawn arrives before the old despawn, so the old bits
        // would otherwise still alias the same preview entity.
        project_event(&mut world, spawn_event_with_node(0xA1, node_id.0));
        project_event(&mut world, spawn_event_with_node(0xB2, node_id.0));
        let projection = world.resource::<PieProjection>();
        assert!(projection.by_bits.contains_key(&0xB2));
        assert!(
            !projection.by_bits.contains_key(&0xA1),
            "the old bits must not alias the same preview"
        );
    }

    #[test]
    fn ephemeral_despawn_rescues_authored_descendants() {
        let (mut world, authored, _node_id) = build_projection_world();
        // The game parented the authored preview under an ephemeral (a zone
        // root), with another ephemeral layered in between.
        project_event(&mut world, spawn_event(0xE1, vec![]));
        project_event(&mut world, spawn_event(0xE2, vec![]));
        let outer = world.resource::<PieProjection>().by_bits[&0xE1];
        let inner = world.resource::<PieProjection>().by_bits[&0xE2];
        world.entity_mut(inner).insert(ChildOf(outer));
        world.entity_mut(authored).insert(ChildOf(inner));

        project_event(&mut world, StateEvent::EntityDespawned { entity: 0xE1 });

        assert!(world.get_entity(outer).is_err(), "ephemeral despawned");
        assert!(
            world.get_entity(inner).is_err(),
            "nested ephemeral despawned"
        );
        assert!(
            world.get_entity(authored).is_ok(),
            "authored preview must survive the ephemeral teardown"
        );
        assert!(world.get::<ChildOf>(authored).is_none());
    }

    // ---- projection skip tests ----

    #[test]
    fn camera_components_are_never_projected() {
        assert!(projection_skips("bevy_camera::camera::Camera"));
        assert!(projection_skips("bevy_camera::camera::RenderTarget"));
        assert!(projection_skips("bevy_camera::components::Camera3d"));
        assert!(projection_skips("bevy_camera::components::Camera2d"));
        assert!(!projection_skips("bevy_camera::projection::Projection"));
        assert!(!projection_skips(
            "bevy_transform::components::transform::Transform"
        ));

        // A streamed camera component is dropped, not applied, even when the
        // type is registered editor-side.
        let (mut world, _preview, _node_id) = build_projection_world();
        {
            let registry = world.resource::<AppTypeRegistry>().clone();
            registry.write().register::<Camera>();
        }
        project_event(
            &mut world,
            spawn_event(
                77,
                vec![(
                    "bevy_camera::camera::Camera".to_string(),
                    serde_json::json!({}),
                )],
            ),
        );
        let preview = world.resource::<PieProjection>().by_bits[&77];
        assert!(world.entity(preview).get::<Camera>().is_none());
    }

    #[test]
    fn ephemeral_previews_carry_a_transform_and_visibility_backbone() {
        let (mut world, _preview, _node_id) = build_projection_world();
        project_event(&mut world, spawn_event(88, vec![]));
        let preview = world.resource::<PieProjection>().by_bits[&88];
        let entity = world.entity(preview);
        assert!(entity.get::<Transform>().is_some());
        assert!(entity.get::<Visibility>().is_some());
    }

    // ---- revert_preview tests ----
    //
    // The full re-spawn path in `respawn_scene_from_ast` requires the Scenes
    // resource, scene-load plumbing, and hierarchy systems that are not
    // available in a unit-test world. These tests cover the in-place half of
    // `revert_preview`; the re-spawn half is covered in tests/guards/play_scene.rs.

    fn build_revert_world() -> (World, Entity, SceneNodeId) {
        // Same as build_projection_world but also inserts the Scenes resource
        // (empty, so respawn_scene_from_ast's tab-empty guard returns early)
        // and CommandHistory (required by capture_active_tab).
        use crate::scenes::Scenes;
        let (mut world, preview_entity, node_id) = build_projection_world();
        world.init_resource::<jackdaw_commands::CommandHistory>();
        world.init_resource::<Scenes>();
        (world, preview_entity, node_id)
    }

    #[test]
    fn projection_skips_filtered_components() {
        let (mut world, _preview_entity, _node_id) = build_projection_world();

        // Build a component map that has one skipped path ("jackdaw::" prefix)
        // and one kept path (Mutable, registered in build_world).
        let mut components = std::collections::HashMap::new();
        components.insert("jackdaw::SomeEditorOnly".to_string(), serde_json::json!(0));
        components.insert(
            <Mutable as bevy::reflect::TypePath>::type_path().to_string(),
            serde_json::json!(7),
        );

        project_event(
            &mut world,
            StateEvent::EntitySpawned {
                entity: RemoteEntity {
                    entity: 5,
                    components,
                    scene_node_id: None,
                },
            },
        );

        // The ephemeral preview entity must have the kept component applied.
        let preview = *world
            .resource::<PieProjection>()
            .by_bits
            .get(&5)
            .expect("bits 5 should be tracked");
        assert_eq!(
            world.entity(preview).get::<Mutable>(),
            Some(&Mutable(7)),
            "kept component must be applied to the preview entity"
        );

        // The skipped path is unregistered, so it cannot appear as any component.
        // Verify this by confirming the entity has exactly one component that we
        // care about and that apply_component_value was never called for the
        // skipped path (which would have panicked or inserted a spurious component
        // if the type were somehow registered). The real assertion is that the call
        // above did NOT panic and Mutable(7) is present -- the skipped path was
        // silently dropped by the filter, not forwarded to apply_component_value.
        let ephemeral_count = world.query::<&PieEphemeral>().iter(&world).count();
        assert_eq!(
            ephemeral_count, 1,
            "exactly one ephemeral entity was spawned"
        );
    }

    #[test]
    fn streamed_default_ui_camera_marker_is_refused() {
        use bevy::ui::IsDefaultUiCamera;
        let path = <IsDefaultUiCamera as bevy::reflect::TypePath>::type_path();
        assert!(projection_skips(path));
        assert!(
            !crate::scene_io::should_skip_component(path),
            "a game's scene may still author it"
        );

        let (mut world, preview_entity, node_id) = build_projection_world();
        world
            .resource::<AppTypeRegistry>()
            .write()
            .register::<IsDefaultUiCamera>();
        let mut components = std::collections::HashMap::new();
        components.insert(path.to_string(), serde_json::json!({}));
        project_event(
            &mut world,
            StateEvent::EntitySpawned {
                entity: RemoteEntity {
                    entity: 9,
                    components,
                    scene_node_id: Some(node_id.0),
                },
            },
        );
        assert!(
            world
                .entity(preview_entity)
                .get::<IsDefaultUiCamera>()
                .is_none()
        );
    }

    #[test]
    fn revert_preview_despawns_ephemerals_and_clears_by_bits() {
        let (mut world, preview_entity, node_id) = build_revert_world();

        // Project an authored overlay (bits=1 -> preview_entity).
        let mut authored_components = std::collections::HashMap::new();
        authored_components.insert(
            <Mutable as bevy::reflect::TypePath>::type_path().to_string(),
            serde_json::json!(99),
        );
        project_event(
            &mut world,
            StateEvent::EntitySpawned {
                entity: RemoteEntity {
                    entity: 1,
                    components: authored_components,
                    scene_node_id: Some(node_id.0),
                },
            },
        );

        // Project an ephemeral (bits=2, no scene_node_id).
        project_event(
            &mut world,
            StateEvent::EntitySpawned {
                entity: RemoteEntity {
                    entity: 2,
                    components: std::collections::HashMap::new(),
                    scene_node_id: None,
                },
            },
        );

        let ephemeral = world
            .resource::<PieProjection>()
            .by_bits
            .get(&2)
            .copied()
            .expect("ephemeral should be tracked before revert");

        // Confirm the authored overlay was applied.
        assert_eq!(
            world.entity(preview_entity).get::<Mutable>(),
            Some(&Mutable(99)),
            "authored overlay should be present before revert"
        );

        revert_preview(&mut world);

        // The ephemeral entity must be gone.
        assert!(
            world.get_entity(ephemeral).is_err(),
            "revert must despawn the ephemeral entity"
        );

        // The projection map must be empty.
        assert!(
            world.resource::<PieProjection>().by_bits.is_empty(),
            "revert must clear by_bits"
        );

        // The game only changed a value, so the entity is restored in place.
        assert!(
            world.get_entity(preview_entity).is_ok(),
            "authored preview entity must survive revert"
        );
    }

    #[derive(Component, Reflect, Default, PartialEq, Debug, Clone)]
    #[reflect(Component)]
    #[type_path = "pie_projection_tests"]
    struct Inventory {
        items: Vec<i32>,
    }

    fn change_mutable(world: &mut World, node_id: SceneNodeId, value: i32) {
        project_event(
            world,
            StateEvent::EntitySpawned {
                entity: RemoteEntity {
                    entity: 1,
                    components: [(
                        <Mutable as bevy::reflect::TypePath>::type_path().to_string(),
                        serde_json::json!(value),
                    )]
                    .into_iter()
                    .collect(),
                    scene_node_id: Some(node_id.0),
                },
            },
        );
    }

    /// An undo during Live took off a component the game had changed.
    #[test]
    fn a_stop_does_not_panic_on_a_component_the_entity_lost() {
        let (mut world, preview, node_id) = build_revert_world();
        change_mutable(&mut world, node_id, 5);
        world.entity_mut(preview).remove::<Mutable>();

        revert_preview(&mut world);

        assert!(world.get::<Mutable>(preview).is_none());
    }

    #[test]
    fn a_stop_does_not_put_back_an_immutable_component_the_entity_lost() {
        let (mut world, preview, node_id) = build_revert_world();
        world.entity_mut(preview).insert(Frozen(1));
        project_event(
            &mut world,
            StateEvent::EntitySpawned {
                entity: RemoteEntity {
                    entity: 1,
                    components: [(
                        <Frozen as bevy::reflect::TypePath>::type_path().to_string(),
                        serde_json::json!(5),
                    )]
                    .into_iter()
                    .collect(),
                    scene_node_id: Some(node_id.0),
                },
            },
        );
        world.entity_mut(preview).remove::<Frozen>();

        revert_preview(&mut world);

        assert!(world.get::<Frozen>(preview).is_none());
    }

    /// A list the game grew comes back at its old length.
    #[test]
    fn a_stop_shrinks_a_list_the_game_grew() {
        let (mut world, preview, node_id) = build_revert_world();
        world
            .resource::<AppTypeRegistry>()
            .write()
            .register::<Inventory>();
        world
            .entity_mut(preview)
            .insert(Inventory { items: vec![] });
        project_event(
            &mut world,
            StateEvent::EntitySpawned {
                entity: RemoteEntity {
                    entity: 1,
                    components: [(
                        <Inventory as bevy::reflect::TypePath>::type_path().to_string(),
                        serde_json::json!({ "items": [1, 2, 3] }),
                    )]
                    .into_iter()
                    .collect(),
                    scene_node_id: Some(node_id.0),
                },
            },
        );
        assert_eq!(
            world.get::<Inventory>(preview).map(|held| held.items.len()),
            Some(3)
        );

        revert_preview(&mut world);

        assert_eq!(
            world.get::<Inventory>(preview),
            Some(&Inventory { items: vec![] })
        );
    }

    /// A child queued for a parent, then moved to another by the game, stays with
    /// the second when the first spawns late.
    #[test]
    fn a_late_parent_does_not_take_back_a_child_the_game_moved_on() {
        use bevy::prelude::ChildOf;
        let (mut world, _preview, _node_id) = build_projection_world();
        let (first_bits, child_bits, path, first_value) = wire_child_of();
        project_event(
            &mut world,
            spawn_event(child_bits, vec![(path.clone(), first_value)]),
        );
        let child = world.resource::<PieProjection>().by_bits[&child_bits];

        let second_bits = Entity::from_raw_u32(4242).expect("an index").to_bits();
        project_event(&mut world, spawn_event(second_bits, vec![]));
        let second = world.resource::<PieProjection>().by_bits[&second_bits];
        project_event(
            &mut world,
            StateEvent::ComponentChanged {
                entity: child_bits,
                type_path: path,
                value: serde_json::json!(format!("{}", Entity::from_bits(second_bits))),
            },
        );
        assert_eq!(world.get::<ChildOf>(child).map(|c| c.0), Some(second));

        project_event(&mut world, spawn_event(first_bits, vec![]));

        assert_eq!(world.get::<ChildOf>(child).map(|c| c.0), Some(second));
    }

    /// A selected Live entity goes from the selection with the entity.
    #[test]
    fn a_stop_leaves_no_dead_entity_selected() {
        let (mut world, preview, _node_id) = build_revert_world();
        world.init_resource::<crate::selection::Selection>();
        project_event(&mut world, spawn_event(2, vec![]));
        let ephemeral = world.resource::<PieProjection>().by_bits[&2];
        world.resource_mut::<crate::selection::Selection>().entities = vec![preview, ephemeral];

        revert_preview(&mut world);

        assert_eq!(
            world.resource::<crate::selection::Selection>().entities,
            vec![preview]
        );
    }

    // ---- set_focused_instance tests ----

    use crate::pie::InstanceKey;
    use crate::pie_mirror::{InstanceBuffer, PieInstances};

    fn instance_key(name: &str) -> InstanceKey {
        InstanceKey {
            config: name.to_string(),
            instance: 1,
        }
    }

    fn build_focus_world() -> World {
        use crate::scenes::Scenes;
        let (mut world, _preview, _node_id) = build_projection_world();
        world.init_resource::<jackdaw_commands::CommandHistory>();
        world.init_resource::<Scenes>();
        world.init_resource::<PieInstances>();
        world
    }

    fn make_buffer_with_entity(bits: u64) -> InstanceBuffer {
        let mut buf = InstanceBuffer::default();
        let mut components = std::collections::HashMap::new();
        components.insert(
            <Mutable as bevy::reflect::TypePath>::type_path().to_string(),
            serde_json::json!(bits as i32),
        );
        buf.entities.insert(
            bits,
            crate::pie_mirror::PieMirrorEntry {
                components,
                scene_node_id: None,
            },
        );
        buf
    }

    #[test]
    fn set_focused_instance_projects_new_instance_and_clears_old() {
        let mut world = build_focus_world();

        let key_a = instance_key("A");
        let key_b = instance_key("B");

        // Populate both instance buffers before any focus is set.
        world
            .resource_mut::<PieInstances>()
            .buffers
            .insert(key_a.clone(), make_buffer_with_entity(0xA0));
        world
            .resource_mut::<PieInstances>()
            .buffers
            .insert(key_b.clone(), make_buffer_with_entity(0xB0));

        // Focus A: this projects A's entity (bits=0xA0) into the preview world.
        set_focused_instance(&mut world, key_a.clone());

        assert_eq!(
            world.resource::<PieInstances>().focused.as_ref(),
            Some(&key_a),
            "focus should be A after first call"
        );
        assert!(
            world
                .resource::<PieProjection>()
                .by_bits
                .contains_key(&0xA0),
            "A's entity should be projected"
        );
        assert!(
            !world
                .resource::<PieProjection>()
                .by_bits
                .contains_key(&0xB0),
            "B's entity must not appear while A is focused"
        );

        // Now switch focus to B.
        set_focused_instance(&mut world, key_b.clone());

        assert_eq!(
            world.resource::<PieInstances>().focused.as_ref(),
            Some(&key_b),
            "focus should be B after switch"
        );
        // A's ephemeral should have been reverted (by_bits cleared on revert).
        assert!(
            !world
                .resource::<PieProjection>()
                .by_bits
                .contains_key(&0xA0),
            "A's entity must be gone after switching to B"
        );
        // B's entity should now be projected.
        assert!(
            world
                .resource::<PieProjection>()
                .by_bits
                .contains_key(&0xB0),
            "B's entity should be projected after switch"
        );
    }

    #[test]
    fn set_focused_instance_noop_when_already_focused() {
        let mut world = build_focus_world();

        let key_a = instance_key("A");
        world
            .resource_mut::<PieInstances>()
            .buffers
            .insert(key_a.clone(), make_buffer_with_entity(0xA0));

        set_focused_instance(&mut world, key_a.clone());

        // Project a second entity directly to simulate ongoing activity.
        project_event(
            &mut world,
            StateEvent::EntitySpawned {
                entity: RemoteEntity {
                    entity: 0xA1,
                    components: std::collections::HashMap::new(),
                    scene_node_id: None,
                },
            },
        );
        assert!(
            world
                .resource::<PieProjection>()
                .by_bits
                .contains_key(&0xA1),
            "manually projected entity should exist"
        );

        // Calling set_focused_instance with the same key must not revert.
        set_focused_instance(&mut world, key_a.clone());

        assert!(
            world
                .resource::<PieProjection>()
                .by_bits
                .contains_key(&0xA1),
            "no-op call must not revert projection"
        );
    }

    fn make_buffer_entry(
        components: Vec<(String, serde_json::Value)>,
        scene_node_id: Option<u64>,
    ) -> crate::pie_mirror::PieMirrorEntry {
        crate::pie_mirror::PieMirrorEntry {
            components: components.into_iter().collect(),
            scene_node_id,
        }
    }

    #[test]
    fn replay_reproduces_stream_parenting() {
        use crate::pie_mirror::{InstanceBuffer, PieInstances, PieViewMode};
        use bevy::prelude::ChildOf;

        let (parent_bits, child_bits, path, value) = wire_child_of();

        // Live stream: container (no node id) then a node-id child carrying
        // ChildOf(container). The child preview must end up parented under the
        // container preview.
        let stream_child_parent = {
            let (mut world, _preview, _node_id) = build_projection_world();
            project_event(&mut world, spawn_event(parent_bits, vec![]));
            project_event(
                &mut world,
                spawn_event(child_bits, vec![(path.clone(), value.clone())]),
            );
            let child_preview = world.resource::<PieProjection>().by_bits[&child_bits];
            world.entity(child_preview).get::<ChildOf>().map(|c| c.0)
        };
        let stream_parent = world_parent_preview(parent_bits, child_bits, &path, &value);
        assert_eq!(
            stream_child_parent,
            Some(stream_parent),
            "live stream must parent the child under the container preview"
        );

        // Replay: install a buffer holding the same two entities, focus it,
        // set Live view, then reproject. The child preview must again be
        // parented under the container preview.
        let mut world = build_focus_world();
        world.init_resource::<PieViewMode>();
        *world.resource_mut::<PieViewMode>() = PieViewMode::Live;
        // The asserted child is an authored scene node streamed back, so on
        // replay it resolves to the authored preview entity. The container has
        // no node id and projects as an ephemeral.
        let child_node_id = {
            let ast = world.resource::<SceneBsnAst>();
            ast.roots.first().and_then(|&node| ast.stable_id_of(node))
        };

        let key = instance_key("A");
        let mut buf = InstanceBuffer::default();
        buf.entities
            .insert(parent_bits, make_buffer_entry(vec![], None));
        buf.entities.insert(
            child_bits,
            make_buffer_entry(vec![(path.clone(), value.clone())], child_node_id),
        );
        // Many sibling children so the HashMap replay order is likely to place
        // a child before the container, exercising the pending-children drain;
        // the parenting assertion holds regardless of replay order.
        for extra in 0..32u64 {
            buf.entities.insert(
                0x1000 + extra,
                make_buffer_entry(vec![(path.clone(), value.clone())], None),
            );
        }
        world
            .resource_mut::<PieInstances>()
            .buffers
            .insert(key.clone(), buf);
        world.resource_mut::<PieInstances>().focused = Some(key.clone());

        // Mimic an already-running Live session before the toggle: the stream
        // projected these same entities once already, leaving by_bits populated
        // and ephemerals spawned. Toggling Scene -> Live calls reproject_focused,
        // which clears and replays.
        replay_buffer(&mut world, &key);

        reproject_focused(&mut world);

        let proj = world.resource::<PieProjection>();
        assert!(
            proj.by_bits.contains_key(&parent_bits),
            "container preview must exist in by_bits after replay"
        );
        let child_preview = proj.by_bits[&child_bits];
        let container_preview = proj.by_bits[&parent_bits];
        assert_eq!(
            world.entity(child_preview).get::<ChildOf>().map(|c| c.0),
            Some(container_preview),
            "replay must parent the child under the container, matching the live stream"
        );
    }

    /// Project container then child as a live stream and return the container's
    /// preview entity, so the parity test can compare the replay outcome.
    fn world_parent_preview(
        parent_bits: u64,
        child_bits: u64,
        path: &str,
        value: &serde_json::Value,
    ) -> Entity {
        let (mut world, _preview, _node_id) = build_projection_world();
        project_event(&mut world, spawn_event(parent_bits, vec![]));
        project_event(
            &mut world,
            spawn_event(child_bits, vec![(path.to_string(), value.clone())]),
        );
        world.resource::<PieProjection>().by_bits[&parent_bits]
    }

    fn log_with_one_entry() -> crate::live_edits::LiveEditLog {
        let mut log = crate::live_edits::LiveEditLog::default();
        log.entries.push((
            crate::live_edits::LiveEditKey {
                bits: 7,
                type_path: "game::Health".to_string(),
                field_path: "current".to_string(),
            },
            crate::live_edits::LiveEditEntry {
                node_id: None,
                baseline: None,
                live_value: serde_json::json!(50.0),
                label: "player / Health.current".to_string(),
            },
        ));
        log
    }

    #[test]
    fn focus_change_skips_log_clear_while_prompt_open() {
        let mut world = build_focus_world();
        world.insert_resource(log_with_one_entry());
        world.insert_resource(crate::live_edits_ui::StopPrompt(true));

        set_focused_instance(&mut world, instance_key("A"));

        assert_eq!(
            world
                .resource::<crate::live_edits::LiveEditLog>()
                .entries
                .len(),
            1,
            "the stop prompt owns the log while it is open"
        );

        // With the prompt closed (or the resource absent entirely), the
        // focus change clears the log as usual.
        let mut world = build_focus_world();
        world.insert_resource(log_with_one_entry());
        world.insert_resource(crate::live_edits_ui::StopPrompt(false));

        set_focused_instance(&mut world, instance_key("A"));

        assert!(
            world
                .resource::<crate::live_edits::LiveEditLog>()
                .is_empty(),
            "a closed prompt does not block the clear"
        );
    }
}
