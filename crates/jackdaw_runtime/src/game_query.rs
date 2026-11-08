//! Answers the editor's questions about the running game, and keeps it told of
//! the values of the entities it watches.
//!
//! A [`ControlEvent::Query`](jackdaw_pie_protocol::ControlEvent::Query) is
//! answered in the frame it arrives. A watch reads its entities at most every
//! [`WATCH_PERIOD`], and only serializes them when one of their components
//! changed. It sends them only when their values differ from the last sent, so a
//! game with nothing watched, or nothing changing, sends nothing.

use std::time::Duration;

use bevy::ecs::archetype::ArchetypeId;
use bevy::ecs::change_detection::Tick;
use bevy::ecs::observer::Observer;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::ecs::resource::IsResource;
use bevy::ecs::system::SystemIdMarker;
use bevy::ecs::world::EntityRef;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use jackdaw_pie_protocol::event::MAX_ANSWER_ENTITIES;
use jackdaw_pie_protocol::snapshot::{entity_component, entity_components};
use jackdaw_pie_protocol::{
    GameEntities, GameEntity, GameEntityRef, GameQuery, QueryAnswer, StateEvent,
};
use jackdaw_scene_types::{SceneDerived, SceneNodeId};

use crate::node_structure::entities_at;
use crate::node_sync::SceneNodeIndex;

/// The shortest time between two reads of a watched entity.
pub(crate) const WATCH_PERIOD: Duration = Duration::from_millis(200);

/// The most entities a watch sends, for a selection found in many instances.
pub(crate) const MAX_WATCHED: usize = 8;

/// What the editor watches, and what it was last sent.
#[derive(Resource, Default)]
pub(crate) struct Watch {
    watch: u64,
    target: Option<GameEntityRef>,
    sent: Option<Vec<GameEntity>>,
    /// The real time the next read is due.
    due: Duration,
    /// The entities last read and their archetypes, so a removed component counts
    /// as a change.
    read: Vec<(Entity, ArchetypeId)>,
    /// The change tick of the last read.
    read_at: Option<Tick>,
}

/// Watch `target`'s entities for the editor under `watch`, or stop for none. The
/// first read is due at once.
pub(crate) fn set_watch(world: &mut World, watch: u64, target: Option<GameEntityRef>) {
    world.insert_resource(Watch {
        watch,
        target,
        ..default()
    });
}

/// Send the watched entities when a read is due and they changed since the last
/// one sent.
pub(crate) fn send_watched(world: &mut World) {
    let now = world
        .get_resource::<Time<Real>>()
        .map_or(Duration::ZERO, Time::elapsed);
    let Some(watch) = world.get_resource::<Watch>() else {
        return;
    };
    let Some(target) = watch.target.clone() else {
        return;
    };
    if now < watch.due {
        return;
    }
    let read_at = watch.read_at;
    let mut found = match &target {
        GameEntityRef::Places(places) => crate::pie::highlighted_entities(world, places)
            .map_or_else(|| entities_of(world, &target), Ok),
        _ => entities_of(world, &target),
    }
    .unwrap_or_default();
    found.truncate(MAX_WATCHED);
    let read: Vec<(Entity, ArchetypeId)> = found
        .iter()
        .map(|&entity| (entity, world.entity(entity).archetype().id()))
        .collect();

    world.increment_change_tick();
    let this_run = world.change_tick();
    let unchanged = read_at.is_some_and(|last_run| {
        world.resource::<Watch>().read == read
            && !found
                .iter()
                .any(|&entity| changed_since(world.entity(entity), last_run, this_run))
    });
    let entities = (!unchanged).then(|| {
        found
            .iter()
            .map(|&entity| describe(world, entity, Values::All))
            .collect::<Vec<_>>()
    });
    let mut watch = world.resource_mut::<Watch>();
    watch.due = now + WATCH_PERIOD;
    watch.read = read;
    watch.read_at = Some(this_run);
    let Some(entities) = entities.filter(|entities| watch.sent.as_ref() != Some(entities)) else {
        world.increment_change_tick();
        return;
    };
    watch.sent = Some(entities.clone());
    let event = StateEvent::Watched {
        watch: watch.watch,
        entities,
    };
    world.increment_change_tick();
    crate::pie::send_state(world, &event);
}

/// Whether any component of `entity` changed after `last_run`.
fn changed_since(entity: EntityRef, last_run: Tick, this_run: Tick) -> bool {
    entity.archetype().iter_components().any(|id| {
        entity
            .get_change_ticks_by_id(id)
            .is_some_and(|ticks| ticks.is_changed(last_run, this_run))
    })
}

/// Answer one query from the editor.
pub(crate) fn answer(world: &mut World, query: &GameQuery) -> QueryAnswer {
    match query {
        GameQuery::Runtime => Ok(runtime_entities(world)),
        GameQuery::Entity(target) => {
            let found = entities_of(world, target)?;
            Ok(capped(
                found
                    .into_iter()
                    .map(|entity| describe(world, entity, Values::All))
                    .collect(),
            ))
        }
        GameQuery::Find { with, name } => find(world, with.as_deref(), name.as_deref()),
    }
}

/// `entities` sorted by id, at most [`MAX_ANSWER_ENTITIES`] of them.
fn capped(mut entities: Vec<GameEntity>) -> GameEntities {
    entities.sort_by_key(|entity| entity.entity);
    let truncated = entities.len() > MAX_ANSWER_ENTITIES;
    entities.truncate(MAX_ANSWER_ENTITIES);
    GameEntities {
        entities,
        truncated,
    }
}

/// The entities `target` names.
fn entities_of(world: &mut World, target: &GameEntityRef) -> Result<Vec<Entity>, String> {
    match target {
        GameEntityRef::Entity(bits) => Entity::try_from_bits(*bits)
            .filter(|&entity| world.get_entity(entity).is_ok())
            .map(|entity| vec![entity])
            .ok_or_else(|| format!("the game has no entity {bits}")),
        GameEntityRef::Node(node) => Ok(world
            .get_resource::<SceneNodeIndex>()
            .map(|index| index.entities(SceneNodeId(*node)).to_vec())
            .unwrap_or_default()),
        GameEntityRef::Places(places) => Ok(entities_at(world, &crate::pie::node_places(places))),
    }
}

/// Entities the world holds for itself: resources, observers and registered
/// systems.
fn is_internal(entity: EntityRef) -> bool {
    entity.contains::<IsResource>()
        || entity.contains::<Observer>()
        || entity.contains::<SystemIdMarker>()
}

/// Whether `entity` is game content: placed in the world or on screen, and
/// neither authored nor built by the scene from what it authors.
fn is_runtime(entity: EntityRef) -> bool {
    !is_internal(entity)
        && !entity.contains::<SceneNodeId>()
        && !entity.contains::<SceneDerived>()
        && (entity.contains::<Transform>() || entity.contains::<bevy::ui::Node>())
}

/// Whether entities sit inside a model a scene instance spawned below a root,
/// remembered per entity so a deep tree is walked once.
#[derive(Default)]
struct ModelMembers(HashMap<Entity, bool>);

impl ModelMembers {
    fn inside(&mut self, world: &World, entity: Entity) -> bool {
        #[cfg(feature = "render")]
        {
            let mut path = Vec::new();
            let mut at = world.get::<ChildOf>(entity).map(ChildOf::parent);
            let mut inside = false;
            while let Some(parent) = at {
                if let Some(&known) = self.0.get(&parent) {
                    inside = known;
                    break;
                }
                if world
                    .get::<bevy::world_serialization::WorldAssetRoot>(parent)
                    .is_some()
                {
                    inside = true;
                    break;
                }
                path.push(parent);
                at = world.get::<ChildOf>(parent).map(ChildOf::parent);
            }
            for below in path {
                self.0.insert(below, inside);
            }
            inside
        }
        #[cfg(not(feature = "render"))]
        {
            let _ = (self, world, entity);
            false
        }
    }
}

/// Every runtime entity, without components, plus the authored parents they sit
/// under.
fn runtime_entities(world: &World) -> GameEntities {
    let mut models = ModelMembers::default();
    let mut found = Vec::new();
    let mut anchors: Vec<Entity> = Vec::new();
    for entity in world.iter_entities() {
        if !is_runtime(entity) || models.inside(world, entity.id()) {
            continue;
        }
        if let Some(parent) = entity.get::<ChildOf>().map(ChildOf::parent)
            && world.get::<SceneNodeId>(parent).is_some()
            && !anchors.contains(&parent)
        {
            anchors.push(parent);
        }
        found.push(describe(world, entity.id(), Values::None));
    }
    found.extend(
        anchors
            .into_iter()
            .map(|entity| describe(world, entity, Values::None)),
    );
    capped(found)
}

/// Entities holding `with` and named like `name`, each with the value of `with`.
fn find(world: &World, with: Option<&str>, name: Option<&str>) -> QueryAnswer {
    let component = match with {
        Some(type_path) => {
            let registry = world.resource::<AppTypeRegistry>().read();
            let type_id = registry
                .get_with_type_path(type_path)
                .map(bevy::reflect::TypeRegistration::type_id)
                .ok_or_else(|| format!("the game has no type {type_path}"))?;
            match world.components().get_id(type_id) {
                Some(id) => Some(id),
                None => return Ok(GameEntities::default()),
            }
        }
        None => None,
    };
    let values = with.map_or(Values::None, Values::One);
    let name = name.map(str::to_lowercase);
    let mut found = Vec::new();
    for entity in world.iter_entities() {
        if is_internal(entity) || component.is_some_and(|id| !entity.contains_id(id)) {
            continue;
        }
        if let Some(name) = &name {
            let named = entity
                .get::<Name>()
                .is_some_and(|held| held.as_str().to_lowercase().contains(name));
            if !named {
                continue;
            }
        }
        found.push(entity.id());
    }
    let truncated = found.len() > MAX_ANSWER_ENTITIES;
    found.sort_by_key(|entity| entity.to_bits());
    found.truncate(MAX_ANSWER_ENTITIES);
    Ok(GameEntities {
        entities: found
            .into_iter()
            .map(|entity| describe(world, entity, values))
            .collect(),
        truncated,
    })
}

/// Which component values an answer carries.
#[derive(Clone, Copy)]
enum Values<'a> {
    None,
    All,
    /// Only the component with this type path.
    One(&'a str),
}

/// `entity` as a query answers it, with the component values `values` asks for.
fn describe(world: &World, entity: Entity, values: Values) -> GameEntity {
    let entity_ref = world.entity(entity);
    let (components, opaque) = match values {
        Values::None => (Vec::new(), Vec::new()),
        Values::All => {
            let registry = world.resource::<AppTypeRegistry>().read();
            entity_components(world, &registry, entity_ref)
        }
        Values::One(type_path) => {
            let registry = world.resource::<AppTypeRegistry>().read();
            entity_component(&registry, entity_ref, type_path)
        }
    };
    GameEntity {
        entity: entity.to_bits(),
        name: entity_ref
            .get::<Name>()
            .map(|name| name.as_str().to_string()),
        parent: entity_ref
            .get::<ChildOf>()
            .map(|child_of| child_of.parent().to_bits()),
        node: entity_ref.get::<SceneNodeId>().map(|node| node.0),
        components,
        opaque,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use jackdaw_pie_protocol::event::{from_bytes, to_bytes};
    use jackdaw_pie_protocol::{
        ControlEvent, IpcChannelTransport, PieChannel, PieTransport, connect, serve,
    };

    use super::*;

    const GUARD: Duration = Duration::from_secs(30);
    const SPEED: &str = "jackdaw_runtime::game_query::tests::Speed";
    const TRANSFORM: &str = "bevy_transform::components::transform::Transform";

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    struct Speed(f32);

    /// A game linked to an editor end over a real link.
    fn linked_game() -> (App, IpcChannelTransport) {
        let (server, name) = serve().expect("a rendezvous");
        let accept = std::thread::spawn(move || server.accept());
        let transport = connect(&name).expect("the game connects");
        let editor = accept
            .join()
            .expect("accept ran")
            .expect("the editor accepts");
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.register_type::<Name>();
        app.register_type::<Transform>();
        app.register_type::<SceneNodeId>();
        app.register_type::<Speed>();
        crate::pie::attach_pie(&mut app, transport);
        app.update();
        (app, editor)
    }

    fn send(editor: &mut IpcChannelTransport, event: &ControlEvent) {
        editor.send(PieChannel::Reliable, &to_bytes(event).expect("encode"));
    }

    /// The state events that reach the editor within `wait`.
    fn received(editor: &mut IpcChannelTransport, wait: Duration) -> Vec<StateEvent> {
        let until = Instant::now() + wait;
        let mut events = Vec::new();
        while Instant::now() < until {
            for (_, bytes) in editor.drain_received() {
                events.push(from_bytes::<StateEvent>(&bytes).expect("a state event"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        events
    }

    /// The first state event to reach the editor.
    fn next_event(editor: &mut IpcChannelTransport) -> StateEvent {
        let until = Instant::now() + GUARD;
        loop {
            assert!(Instant::now() < until, "the game never answered");
            if let Some((_, bytes)) = editor.drain_received().into_iter().next() {
                return from_bytes::<StateEvent>(&bytes).expect("a state event");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Ask the game `query`, run one frame, and return its answer.
    fn ask_whole(app: &mut App, editor: &mut IpcChannelTransport, query: GameQuery) -> QueryAnswer {
        send(editor, &ControlEvent::Query { request: 9, query });
        app.update();
        match next_event(editor) {
            StateEvent::QueryAnswer { request: 9, answer } => answer,
            other => panic!("expected the answer, got {other:?}"),
        }
    }

    /// The entities of the game's answer to `query`, which holds them all.
    fn ask(
        app: &mut App,
        editor: &mut IpcChannelTransport,
        query: GameQuery,
    ) -> Result<Vec<GameEntity>, String> {
        ask_whole(app, editor, query).map(|answer| {
            assert!(!answer.truncated);
            answer.entities
        })
    }

    fn bits(entity: Entity) -> u64 {
        entity.to_bits()
    }

    #[test]
    fn runtime_entities_and_the_authored_parents_they_sit_under_are_answered_in_a_frame() {
        let (mut app, mut editor) = linked_game();
        let world = app.world_mut();
        let door = world.spawn((SceneNodeId(7), Name::new("Door"))).id();
        let spark = world
            .spawn((Name::new("Spark"), Transform::default(), ChildOf(door)))
            .id();
        let bullet = world
            .spawn((Name::new("Bullet"), Transform::default()))
            .id();
        let authored_child = world.spawn((SceneNodeId(8), ChildOf(door))).id();
        let observer = world.add_observer(|_: On<Add<Speed>>| {}).id();

        let found = ask(&mut app, &mut editor, GameQuery::Runtime).expect("an answer");

        let of = |entity: Entity| found.iter().find(|held| held.entity == bits(entity));
        let spark = of(spark).expect("the spawned child");
        assert_eq!(spark.parent, Some(bits(door)));
        assert_eq!(spark.name.as_deref(), Some("Spark"));
        assert!(spark.components.is_empty(), "a listing carries no values");
        assert!(of(bullet).is_some());
        assert_eq!(of(door).expect("the parent it sits under").node, Some(7));
        assert!(
            of(authored_child).is_none(),
            "authored entities are not runtime"
        );
        assert!(
            of(observer).is_none(),
            "the world's own entities are left out"
        );
        assert!(
            found
                .iter()
                .all(|held| held.node.is_none() || held.entity == bits(door))
        );
    }

    #[test]
    fn an_entity_is_answered_with_its_components_by_its_id_or_its_node() {
        let (mut app, mut editor) = linked_game();
        let world = app.world_mut();
        let rock = world
            .spawn((
                SceneNodeId(7),
                Name::new("Rock"),
                Transform::from_xyz(1.0, 2.0, 3.0),
            ))
            .id();
        let bullet = world.spawn((Name::new("Bullet"), Speed(4.0))).id();

        let found = ask(
            &mut app,
            &mut editor,
            GameQuery::Entity(GameEntityRef::Entity(bits(bullet))),
        )
        .expect("an answer");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name.as_deref(), Some("Bullet"));
        assert!(
            found[0]
                .components
                .contains(&(SPEED.to_string(), serde_json::json!(4.0)))
        );

        let found = ask(
            &mut app,
            &mut editor,
            GameQuery::Entity(GameEntityRef::Node(7)),
        )
        .expect("an answer");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].entity, bits(rock));
        let (_, transform) = found[0]
            .components
            .iter()
            .find(|(path, _)| path == TRANSFORM)
            .expect("the transform");
        assert_eq!(transform["translation"], serde_json::json!([1.0, 2.0, 3.0]));

        let missing = Entity::from_raw_u32(90_000).expect("an index").to_bits();
        let answer = ask(
            &mut app,
            &mut editor,
            GameQuery::Entity(GameEntityRef::Entity(missing)),
        );
        assert!(
            answer.is_err(),
            "a missing entity is an error, got {answer:?}"
        );
    }

    #[test]
    fn a_filtered_query_matches_a_component_and_a_name() {
        let (mut app, mut editor) = linked_game();
        let world = app.world_mut();
        let fast = world.spawn((Name::new("Bullet A"), Speed(2.0))).id();
        world.spawn((Name::new("Rock"), Speed(0.0)));
        world.spawn(Name::new("Bullet B"));

        let found = ask(
            &mut app,
            &mut editor,
            GameQuery::Find {
                with: Some(SPEED.to_string()),
                name: Some("bullet".to_string()),
            },
        )
        .expect("an answer");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].entity, bits(fast));
        assert_eq!(
            found[0].components,
            vec![(SPEED.to_string(), serde_json::json!(2.0))],
            "each carries only the value asked for"
        );

        let found = ask(
            &mut app,
            &mut editor,
            GameQuery::Find {
                with: None,
                name: Some("BULLET".to_string()),
            },
        )
        .expect("an answer");
        assert_eq!(found.len(), 2);

        let answer = ask(
            &mut app,
            &mut editor,
            GameQuery::Find {
                with: Some("game::Unknown".to_string()),
                name: None,
            },
        );
        assert!(
            answer.is_err(),
            "an unknown type is an error, got {answer:?}"
        );
    }

    fn speed_of(entities: &[GameEntity]) -> Option<f64> {
        let (_, value) = entities
            .first()?
            .components
            .iter()
            .find(|(path, _)| path == SPEED)?;
        value.as_f64()
    }

    #[test]
    fn a_watch_sends_its_entity_when_it_changes_at_a_low_rate_and_stops_when_cleared() {
        let (mut app, mut editor) = linked_game();
        let bullet = app.world_mut().spawn(Speed(1.0)).id();
        let watch = |watch, target| ControlEvent::Watch { watch, target };

        send(
            &mut editor,
            &watch(1, Some(GameEntityRef::Entity(bits(bullet)))),
        );
        app.update();
        match next_event(&mut editor) {
            StateEvent::Watched { watch: 1, entities } => {
                assert_eq!(
                    speed_of(&entities),
                    Some(1.0),
                    "the first read goes at once"
                );
            }
            other => panic!("expected the watched entity, got {other:?}"),
        }

        std::thread::sleep(WATCH_PERIOD);
        app.update();
        assert!(
            received(&mut editor, Duration::from_millis(50)).is_empty(),
            "nothing changed, so nothing is sent"
        );

        app.world_mut().get_mut::<Speed>(bullet).expect("speed").0 = 2.0;
        app.update();
        assert!(
            received(&mut editor, Duration::from_millis(50)).is_empty(),
            "a change waits for the next read"
        );
        std::thread::sleep(WATCH_PERIOD);
        app.update();
        match next_event(&mut editor) {
            StateEvent::Watched { watch: 1, entities } => {
                assert_eq!(speed_of(&entities), Some(2.0));
            }
            other => panic!("expected the watched entity, got {other:?}"),
        }

        send(&mut editor, &watch(2, None));
        for step in 0..3 {
            app.world_mut().get_mut::<Speed>(bullet).expect("speed").0 = 3.0 + step as f32;
            std::thread::sleep(WATCH_PERIOD);
            app.update();
        }
        assert!(
            received(&mut editor, Duration::from_millis(50)).is_empty(),
            "a cleared watch sends nothing"
        );
    }

    #[test]
    fn a_game_asked_nothing_sends_nothing_while_its_world_changes() {
        let (mut app, mut editor) = linked_game();
        let bullet = app
            .world_mut()
            .spawn((Speed(0.0), Transform::default()))
            .id();
        for step in 0..20 {
            app.world_mut().get_mut::<Speed>(bullet).expect("speed").0 = step as f32;
            app.world_mut().spawn(Name::new("Spark"));
            app.update();
        }
        assert!(received(&mut editor, Duration::from_millis(100)).is_empty());
    }

    #[test]
    fn scene_built_children_and_engine_plumbing_are_not_runtime_entities() {
        let (mut app, mut editor) = linked_game();
        let world = app.world_mut();
        let rock = world
            .spawn((SceneNodeId(7), Transform::default(), Name::new("Rock")))
            .id();
        let face = world
            .spawn((
                jackdaw_scene_types::DerivedFaceMesh,
                Transform::default(),
                ChildOf(rock),
            ))
            .id();
        let renderer = world.spawn(Name::new("LineGizmoRenderer")).id();
        let camera = world
            .spawn((Name::new("Camera"), Transform::default()))
            .id();

        let found = ask(&mut app, &mut editor, GameQuery::Runtime).expect("an answer");
        let held = |entity: Entity| found.iter().any(|held| held.entity == bits(entity));
        assert!(!held(face), "a brush face mesh is built by the scene");
        assert!(!held(renderer), "a renderer with no place is plumbing");
        assert!(!held(rock), "nothing runtime sits under the rock");
        assert!(held(camera), "an entity the game placed is listed");
    }

    #[test]
    fn a_large_answer_is_cut_short_and_says_so() {
        let (mut app, mut editor) = linked_game();
        let many: Vec<_> = (0..MAX_ANSWER_ENTITIES + 5)
            .map(|_| (Transform::default(), Speed(1.0)))
            .collect();
        app.world_mut().spawn_batch(many);

        let answer = ask_whole(&mut app, &mut editor, GameQuery::Runtime).expect("an answer");
        assert!(answer.truncated);
        assert_eq!(answer.entities.len(), MAX_ANSWER_ENTITIES);

        let answer = ask_whole(
            &mut app,
            &mut editor,
            GameQuery::Find {
                with: Some(SPEED.to_string()),
                name: None,
            },
        )
        .expect("an answer");
        assert!(answer.truncated);
        assert_eq!(answer.entities.len(), MAX_ANSWER_ENTITIES);
    }

    #[test]
    fn a_watch_holds_a_few_of_many_instances_and_sees_a_removed_component() {
        let (mut app, mut editor) = linked_game();
        let first = app.world_mut().spawn((SceneNodeId(7), Speed(1.0))).id();
        for _ in 0..MAX_WATCHED + 3 {
            app.world_mut().spawn((SceneNodeId(7), Speed(1.0)));
        }
        send(
            &mut editor,
            &ControlEvent::Watch {
                watch: 1,
                target: Some(GameEntityRef::Node(7)),
            },
        );
        app.update();
        match next_event(&mut editor) {
            StateEvent::Watched { entities, .. } => assert_eq!(entities.len(), MAX_WATCHED),
            other => panic!("expected the watched entities, got {other:?}"),
        }

        app.world_mut().entity_mut(first).remove::<Speed>();
        std::thread::sleep(WATCH_PERIOD);
        app.update();
        match next_event(&mut editor) {
            StateEvent::Watched { entities, .. } => {
                let first = entities
                    .iter()
                    .find(|held| held.entity == bits(first))
                    .expect("the first instance");
                assert!(first.components.iter().all(|(path, _)| path != SPEED));
            }
            other => panic!("expected the watched entities, got {other:?}"),
        }
    }
}
