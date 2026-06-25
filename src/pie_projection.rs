//! Projects the focused PIE instance's streamed state into the editor preview
//! ECS, so the Scene outliner and inspector show the live game's entities, and
//! the Game panel's Select mode can resolve a picked game entity. (Those
//! entities also appear in the viewport because they are real preview ECS
//! entities; there is no separate viewport renderer for the running game.)

use std::collections::HashMap;

use bevy::ecs::component::ComponentId;
use bevy::prelude::*;
use bevy::reflect::serde::TypedReflectDeserializer;
use jackdaw_jsn::ast::{JsnNodeId, SceneJsnAst};
use jackdaw_pie_protocol::StateEvent;
use serde::de::{DeserializeSeed, IntoDeserializer};

/// Marker on a preview entity that exists only because the running game spawned
/// it (no authored AST node). Removed when play stops or the focus instance
/// changes; never serialized.
#[derive(Component, Debug, Clone, Copy)]
pub struct PieEphemeral;

/// Maps a streamed entity (game-side bits) to the preview entity representing
/// it: an authored entity resolved via `JsnNodeId`, or an ephemeral preview
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
/// pass (the default image is not renderable). New game builds no longer send
/// these, but the projector refuses them regardless so an older game binary
/// cannot crash the editor.
const PROJECTION_SKIP_PREFIXES: &[&str] = &["bevy_camera::camera::", "bevy_camera::components::"];

/// Streamed but never saved: these must reach the preview even though the
/// save filter rejects them. The rig activation marker is how the Live
/// camera lock finds the game's active rig; dropping it here would silently
/// misalign picking and overlays against the streamed frame.
const PROJECTION_ALLOW_PATHS: &[&str] = &["jackdaw_camera_rig::ActiveCameraRig"];

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

/// `ChildOf` is a single-field struct over `Entity`, and the snapshot
/// serializer writes `Entity` as raw u64 bits, so the wire value is the bare
/// number. The array/object arms tolerate the other reflect serialization
/// shapes a tuple struct can take.
fn parent_bits_from_value(value: &serde_json::Value) -> Option<u64> {
    match value {
        serde_json::Value::Number(n) => n.as_u64(),
        serde_json::Value::Array(a) => a.first().and_then(serde_json::Value::as_u64),
        serde_json::Value::Object(o) => o.values().next().and_then(serde_json::Value::as_u64),
        _ => None,
    }
}

fn reparent_preview(world: &mut World, child: Entity, parent: Entity) {
    if child == parent || would_cycle(world, child, parent) {
        return;
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
    match parent {
        Some(parent) => reparent_preview(world, child, parent),
        None => world
            .resource_mut::<PieProjection>()
            .pending_children
            .entry(parent_bits)
            .or_default()
            .push(child),
    }
}

/// Tear down all projected live state and restore the authored preview.
///
/// Despawns every ephemeral entity, clears the projection map, then
/// re-spawns the scene from the AST so authored overlays revert to their
/// authored values. The AST is the untouched baseline; this reuses the same
/// re-spawn that tab activation uses on a tab switch.
///
/// Safe to call when nothing was projected: no ephemerals, empty `by_bits`,
/// and re-spawning an unchanged scene is harmless.
pub fn revert_preview(world: &mut World) {
    clear_projection(world);
    crate::scenes::swap::respawn_scene_from_ast(world);
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
            let preview = entity
                .scene_node_id
                .and_then(|id| {
                    world
                        .resource::<SceneJsnAst>()
                        .entity_for_node_id(JsnNodeId(id))
                })
                .unwrap_or_else(|| {
                    // Seed Transform/Visibility so children inherit before
                    // streamed values overwrite the defaults.
                    world
                        .spawn((PieEphemeral, Transform::default(), Visibility::default()))
                        .id()
                });
            world
                .resource_mut::<PieProjection>()
                .by_bits
                .insert(bits, preview);
            // A zone hot-reload respawns an authored node under new bits and
            // sends the new spawn before the old despawn, so drop any other
            // bits still aliasing this preview entity. Otherwise a reverse
            // lookup could resolve dead bits.
            world
                .resource_mut::<PieProjection>()
                .by_bits
                .retain(|&b, &mut mapped| b == bits || mapped != preview);
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
                apply_component_value(world, preview, &type_path, &value);
            }
            if let Some(value) = child_of {
                apply_child_of(world, preview, &value);
            }
            // This entity may be the parent earlier children were waiting for.
            let waiting = world
                .resource_mut::<PieProjection>()
                .pending_children
                .remove(&bits);
            if let Some(children) = waiting {
                for child in children {
                    reparent_preview(world, child, preview);
                }
            }
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
                    apply_component_value(world, preview, &type_path, &value);
                }
            }
        }
        StateEvent::EntityDespawned { entity } => {
            let preview = world
                .resource_mut::<PieProjection>()
                .by_bits
                .remove(&entity);
            // Children queued for a parent that will never spawn again.
            world
                .resource_mut::<PieProjection>()
                .pending_children
                .remove(&entity);
            if let Some(preview) = preview {
                // Only despawn entities this projector created; authored
                // entities are reverted on stop, not despawned here.
                if world.get::<PieEphemeral>(preview).is_some() {
                    detach_authored_descendants(world, preview);
                    if let Ok(e) = world.get_entity_mut(preview) {
                        e.despawn();
                    }
                }
            }
        }
        StateEvent::Status { .. }
        | StateEvent::Log { .. }
        | StateEvent::CursorState { .. }
        | StateEvent::PickResult { .. } => {}
    }
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
/// registered, has no `ReflectComponent`, or the value does not deserialize.
pub fn apply_component_value(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    value: &serde_json::Value,
) {
    let registry = world.resource::<AppTypeRegistry>().clone();
    let registry_guard = registry.read();

    let Some(registration) = registry_guard.get_with_type_path(type_path) else {
        debug!("apply_component_value: type not registered: {type_path}");
        return;
    };

    let Some(reflect_component) = registration.data::<ReflectComponent>().cloned() else {
        debug!("apply_component_value: no ReflectComponent for: {type_path}");
        return;
    };

    let mut processor = jackdaw_pie_protocol::RemoteDeserializerProcessor;
    let reflect_deserializer =
        TypedReflectDeserializer::with_processor(registration, &registry_guard, &mut processor);
    let reflected = match reflect_deserializer.deserialize(value.clone().into_deserializer()) {
        Ok(v) => v,
        Err(e) => {
            debug!("apply_component_value: deserialization failed for {type_path}: {e}");
            return;
        }
    };

    let type_id = registration.type_id();

    let is_mutable = world
        .components()
        .get_id(type_id)
        .and_then(|id: ComponentId| world.components().get_info(id))
        .map(bevy::ecs::component::ComponentInfo::mutable)
        .unwrap_or(true);

    // Check presence while we still have shared world access (before the &mut borrow).
    let present = match world.get_entity(entity) {
        Ok(entity_ref) => reflect_component.contains(entity_ref),
        Err(_) => {
            debug!("apply_component_value: entity {entity:?} does not exist");
            return;
        }
    };

    drop(registry_guard);

    let Ok(mut entity_mut) = world.get_entity_mut(entity) else {
        debug!("apply_component_value: entity {entity:?} was despawned concurrently");
        return;
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
        use jackdaw_jsn::Brush;

        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default()));
        app.add_plugins(jackdaw_jsn::JsnPlugin::default());

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
        world.add_observer(|_: On<Insert, Mutable>, mut n: ResMut<Inserts>| n.0 += 1);
        world.add_observer(|_: On<Insert, Frozen>, mut n: ResMut<Inserts>| n.0 += 1);
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

    use jackdaw_jsn::ast::{JsnEntityNode, JsnNodeId, SceneJsnAst};
    use jackdaw_pie_protocol::StateEvent;
    use jackdaw_pie_protocol::snapshot::RemoteEntity;
    use std::collections::HashSet;

    fn build_projection_world() -> (World, Entity, JsnNodeId) {
        let mut world = build_world();
        world.init_resource::<PieProjection>();

        // Build a SceneJsnAst with one authored node bound to a preview entity.
        let preview_entity = world.spawn(Mutable(0)).id();
        let node_id = JsnNodeId::next();
        let mut ast = SceneJsnAst::default();
        ast.nodes.push(JsnEntityNode {
            id: Some(node_id),
            parent: None,
            components: std::collections::HashMap::new(),
            derived_components: HashSet::new(),
            ecs_entity: Some(preview_entity),
        });
        ast.ecs_to_jsn.insert(preview_entity, 0);
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
        // Never saved, but MUST stream: the Live camera lock finds the game's
        // active rig through this marker.
        assert!(!projection_skips("jackdaw_camera_rig::ActiveCameraRig"));
        assert!(crate::scene_io::should_skip_component(
            "jackdaw_camera_rig::ActiveCameraRig"
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
    // available in a unit-test world. These tests verify the ephemeral-despawn
    // and by_bits-clear halves of `revert_preview`; the re-spawn half is
    // covered by integration tests for `swap_active_tab`.

    fn build_revert_world() -> (World, Entity, JsnNodeId) {
        // Same as build_projection_world but also inserts the Scenes resource
        // (empty, so respawn_scene_from_ast's tab-empty guard returns early)
        // and CommandHistory (required by capture_active_tab). With an empty
        // Scenes, the re-spawn step is skipped and the test only covers the
        // ephemeral-despawn and by_bits-clear portions of revert_preview.
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

        // The authored preview entity must still exist (re-spawn recreates it;
        // in this minimal test world the re-spawn path no-ops because Scenes
        // has no loaded content, but the entity was not despawned by revert itself).
        assert!(
            world.get_entity(preview_entity).is_ok(),
            "authored preview entity must survive revert"
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
        let child_node_id = world.resource::<SceneJsnAst>().nodes[0].id.map(|id| id.0);

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
