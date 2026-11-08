//! Questions the editor asks the running game, and the live values of the entity
//! it watches.
//!
//! [`ask_game`] sends a [`GameQuery`] to the focused game and hands back a
//! request id; [`GameQueries::take`] gives its answer once it is in, or an error
//! once the game stopped or [`QUERY_TIMEOUT`] passed. The game keeps the editor
//! told of the primary selection's entities, or of the runtime entity
//! [`pick_runtime_entity`] picked, and [`GameQueries::watched`] holds their
//! latest values. Nothing goes either way while nothing is asked or
//! selected.

use std::time::{Duration, Instant};

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use jackdaw_pie_protocol::ControlEvent;
pub use jackdaw_pie_protocol::{GameEntities, GameEntity, GameEntityRef, GameQuery, QueryAnswer};

use crate::pie::{InstanceKey, PieSession, focused_live_instance, send_control_to_instance};

/// How long a question waits for its answer, and an answer for someone to take it.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(5);

/// The questions in flight to the running game, their answers, and the watched
/// entity's values.
#[derive(Resource, Default)]
pub struct GameQueries {
    next_request: u64,
    waiting: HashMap<u64, Waiting>,
    answered: HashMap<u64, (Instant, QueryAnswer)>,
    runtime_pick: Option<RuntimePick>,
    next_watch: u64,
    watching: Option<Watching>,
    watched: Vec<GameEntity>,
    watch_answered: bool,
    generation: u64,
}

/// A game entity watched in place of the selection, until the selection moves
/// off what it was when the entity was picked.
struct RuntimePick {
    entity: u64,
    selected: Option<Entity>,
    key: Option<InstanceKey>,
}

struct Waiting {
    key: InstanceKey,
    asked: Instant,
}

/// The watch last sent to a game.
#[derive(Clone)]
struct Watching {
    key: InstanceKey,
    watch: u64,
    target: GameEntityRef,
}

impl GameQueries {
    /// The answer to `request`, once. `None` while the game has yet to answer.
    pub fn take(&mut self, request: u64) -> Option<QueryAnswer> {
        if self.waiting.contains_key(&request) {
            return None;
        }
        Some(
            self.answered
                .remove(&request)
                .map_or_else(|| Err(format!("no question {request} is held")), |(_, a)| a),
        )
    }

    /// The entities being watched, as the game last sent them. Empty while
    /// nothing is watched or before the game has answered.
    pub fn watched(&self) -> &[GameEntity] {
        &self.watched
    }

    /// Whether the game has sent the watched entities since the watch began.
    pub fn watch_answered(&self) -> bool {
        self.watch_answered
    }

    /// A count that moves whenever what is watched, its values or the runtime
    /// pick change.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// What the game is watching, if anything.
    pub fn watch_target(&self) -> Option<&GameEntityRef> {
        self.watching.as_ref().map(|watching| &watching.target)
    }

    /// The runtime entity watched in place of the selection, as `Entity::to_bits`
    /// in the game.
    pub fn runtime_pick(&self) -> Option<u64> {
        self.runtime_pick.as_ref().map(|pick| pick.entity)
    }

    /// Whether `request` is waiting or holds an answer to take.
    pub fn holds(&self, request: u64) -> bool {
        self.waiting.contains_key(&request) || self.answered.contains_key(&request)
    }

    /// How many questions are waiting on the game.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }

    fn receive(&mut self, key: &InstanceKey, request: u64, answer: QueryAnswer) {
        if self
            .waiting
            .get(&request)
            .is_some_and(|waiting| waiting.key == *key)
        {
            self.waiting.remove(&request);
            self.answered.insert(request, (Instant::now(), answer));
        }
    }

    fn receive_watched(&mut self, key: &InstanceKey, watch: u64, entities: Vec<GameEntity>) {
        if self
            .watching
            .as_ref()
            .is_some_and(|watching| watching.key == *key && watching.watch == watch)
        {
            self.watched = entities;
            self.watch_answered = true;
            self.generation += 1;
        }
    }

    /// Answer with an error every question asked of a game no longer in `live`
    /// or asked longer than [`QUERY_TIMEOUT`] before `now`, and drop answers
    /// nobody took in that time.
    pub fn expire(&mut self, now: Instant, live: &[InstanceKey]) {
        let mut failed = Vec::new();
        self.waiting.retain(|&request, waiting| {
            let reason = if !live.contains(&waiting.key) {
                format!("{} stopped before it answered", waiting.key)
            } else if now.saturating_duration_since(waiting.asked) >= QUERY_TIMEOUT {
                format!("{} did not answer in time", waiting.key)
            } else {
                return true;
            };
            failed.push((request, reason));
            false
        });
        self.answered
            .retain(|_, (at, _)| now.saturating_duration_since(*at) < QUERY_TIMEOUT);
        for (request, reason) in failed {
            self.answered.insert(request, (now, Err(reason)));
        }
    }
}

/// Send `query` to the focused game, returning the request its answer comes
/// back under, or an error with no game running.
pub fn ask_game(world: &mut World, query: GameQuery) -> Result<u64, String> {
    let key = focused_live_instance(world)
        .ok_or_else(|| "no game is running; start one with the pie.play operator".to_string())?;
    let mut queries = world.get_resource_or_init::<GameQueries>();
    queries.next_request += 1;
    let request = queries.next_request;
    queries.waiting.insert(
        request,
        Waiting {
            key: key.clone(),
            asked: Instant::now(),
        },
    );
    send_control_to_instance(world, &key, ControlEvent::Query { request, query });
    Ok(request)
}

/// Take a game's answer to a question.
pub(crate) fn receive_answer(
    world: &mut World,
    key: &InstanceKey,
    request: u64,
    answer: QueryAnswer,
) {
    if let Some(mut queries) = world.get_resource_mut::<GameQueries>() {
        queries.receive(key, request, answer);
    }
}

/// Take a game's latest values of the watched entities.
pub(crate) fn receive_watched(
    world: &mut World,
    key: &InstanceKey,
    watch: u64,
    entities: Vec<GameEntity>,
) {
    if let Some(mut queries) = world.get_resource_mut::<GameQueries>() {
        queries.receive_watched(key, watch, entities);
    }
}

/// Fail the questions the game will not answer.
pub(crate) fn expire_game_queries(world: &mut World) {
    let live = world
        .get_non_send::<PieSession>()
        .map(PieSession::live_keys)
        .unwrap_or_default();
    if let Some(mut queries) = world.get_resource_mut::<GameQueries>() {
        queries.expire(Instant::now(), &live);
    }
}

/// The editor's primary selection, if it keeps one.
fn primary_selection(world: &World) -> Option<Entity> {
    world
        .get_resource::<crate::selection::Selection>()
        .and_then(crate::selection::Selection::primary)
}

/// Watch the game entity `entity`, as `Entity::to_bits` there, in place of the
/// selection. Selecting anything else in the editor ends the pick.
pub fn pick_runtime_entity(world: &mut World, entity: u64) {
    let selected = primary_selection(world);
    let key = focused_live_instance(world);
    let mut queries = world.get_resource_or_init::<GameQueries>();
    queries.runtime_pick = Some(RuntimePick {
        entity,
        selected,
        key,
    });
    queries.generation += 1;
}

/// Go back to watching the selection.
pub fn clear_runtime_pick(world: &mut World) {
    if let Some(mut queries) = world.get_resource_mut::<GameQueries>() {
        queries.runtime_pick = None;
        queries.generation += 1;
    }
}

/// What the focused game `key` should watch: the runtime entity picked, or the
/// primary selection's entities.
fn watch_target(world: &World, key: &InstanceKey) -> Option<GameEntityRef> {
    if let Some(pick) = world.resource::<GameQueries>().runtime_pick.as_ref() {
        return Some(GameEntityRef::Entity(pick.entity));
    }
    if !world.contains_resource::<crate::selection::Selection>() {
        return None;
    }
    let places = crate::live_highlight::highlight_places(world, key);
    (!places.is_empty()).then_some(GameEntityRef::Places(places))
}

/// Point the focused game's watch at what is selected, telling a game that loses
/// it to stop. Sends only when the target or the focused game changes.
pub(crate) fn sync_watch(world: &mut World) {
    let selected = primary_selection(world);
    let focused = focused_live_instance(world);
    let mut queries = world.get_resource_or_init::<GameQueries>();
    if queries.runtime_pick.as_ref().is_some_and(|pick| {
        (selected.is_some() && pick.selected != selected) || pick.key != focused
    }) {
        queries.runtime_pick = None;
        queries.generation += 1;
    }
    let current = queries.watching.clone();
    let desired = focused_live_instance(world)
        .and_then(|key| watch_target(world, &key).map(|target| (key, target)));
    let unchanged = match (&current, &desired) {
        (Some(watching), Some((key, target))) => watching.key == *key && watching.target == *target,
        (None, None) => true,
        _ => false,
    };
    if unchanged {
        return;
    }
    let mut queries = world.resource_mut::<GameQueries>();
    queries.watched.clear();
    queries.watch_answered = false;
    queries.generation += 1;
    queries.next_watch += 1;
    let watch = queries.next_watch;
    queries.watching = desired
        .clone()
        .map(|(key, target)| Watching { key, watch, target });
    if let Some(old) = current
        && desired.as_ref().is_none_or(|(key, _)| *key != old.key)
    {
        let stop = ControlEvent::Watch {
            watch,
            target: None,
        };
        send_control_to_instance(world, &old.key, stop);
    }
    if let Some((key, target)) = desired {
        let start = ControlEvent::Watch {
            watch,
            target: Some(target),
        };
        send_control_to_instance(world, &key, start);
    }
}

/// Forget the watch and the questions of a session that ended.
pub(crate) fn forget_game_queries(world: &mut World) {
    if let Some(mut queries) = world.get_resource_mut::<GameQueries>() {
        queries.watching = None;
        queries.watched.clear();
        queries.watch_answered = false;
        queries.runtime_pick = None;
        queries.generation += 1;
    }
    expire_game_queries(world);
}

#[cfg(test)]
mod tests {
    use std::process::{Command, Stdio};

    use jackdaw_pie_protocol::event::{from_bytes, to_bytes};
    use jackdaw_pie_protocol::{
        IpcChannelTransport, PieChannel, PieTransport, StateEvent, connect, serve,
    };

    use super::*;
    use crate::live_highlight::tests::{at_the_node, game, world_with_node};
    use crate::selection::Selection;

    const GUARD: Duration = Duration::from_secs(30);

    fn link() -> (IpcChannelTransport, IpcChannelTransport) {
        let (server, name) = serve().expect("a rendezvous");
        let accept = std::thread::spawn(move || server.accept());
        let game = connect(&name).expect("the game connects");
        let editor = accept
            .join()
            .expect("accept ran")
            .expect("the editor accepts");
        (editor, game)
    }

    /// An editor playing game 1 over a real link, with a node it can select, and
    /// the game's end of the link.
    fn playing() -> (World, Entity, IpcChannelTransport) {
        let (mut world, node) = world_with_node();
        world.init_resource::<GameQueries>();
        let (editor, game_end) = link();
        let child = Command::new(std::env::current_exe().expect("the test binary"))
            .arg("--list")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("a child process");
        crate::pie::connect_test_game(&mut world, game(1), child, editor).expect("a link");
        (world, node, game_end)
    }

    /// The control events that reach the game within `wait`.
    fn reaching_game(game_end: &mut IpcChannelTransport, wait: Duration) -> Vec<ControlEvent> {
        let until = Instant::now() + wait;
        let mut events = Vec::new();
        while Instant::now() < until {
            for (_, bytes) in game_end.drain_received() {
                events.push(from_bytes::<ControlEvent>(&bytes).expect("a control event"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        events
    }

    fn next_control(game_end: &mut IpcChannelTransport) -> ControlEvent {
        let until = Instant::now() + GUARD;
        loop {
            assert!(Instant::now() < until, "nothing reached the game");
            if let Some((_, bytes)) = game_end.drain_received().into_iter().next() {
                return from_bytes::<ControlEvent>(&bytes).expect("a control event");
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn reply(game_end: &mut IpcChannelTransport, event: &StateEvent) {
        game_end.send(PieChannel::Reliable, &to_bytes(event).expect("encode"));
    }

    /// Drain the link until `done` holds.
    fn drain_until(world: &mut World, mut done: impl FnMut(&mut World) -> bool) {
        let until = Instant::now() + GUARD;
        while !done(world) {
            assert!(Instant::now() < until, "the editor never took it");
            crate::pie::drain_game_events(world);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn bullet() -> GameEntity {
        GameEntity {
            entity: 42,
            name: Some("Bullet".to_string()),
            parent: None,
            node: None,
            components: vec![("game::Speed".to_string(), serde_json::json!(2.0))],
            opaque: Vec::new(),
        }
    }

    fn answer() -> GameEntities {
        GameEntities {
            entities: vec![bullet()],
            truncated: false,
        }
    }

    #[test]
    fn a_question_reaches_the_focused_game_and_its_answer_is_taken_once() {
        let (mut world, _, mut game_end) = playing();

        let request = ask_game(&mut world, GameQuery::Runtime).expect("a game is running");
        let ControlEvent::Query {
            request: asked,
            query,
        } = next_control(&mut game_end)
        else {
            panic!("expected the question");
        };
        assert_eq!((asked, query), (request, GameQuery::Runtime));
        assert_eq!(world.resource_mut::<GameQueries>().take(request), None);

        reply(
            &mut game_end,
            &StateEvent::QueryAnswer {
                request,
                answer: Ok(answer()),
            },
        );
        drain_until(&mut world, |world| {
            world.resource::<GameQueries>().waiting() == 0
        });
        assert_eq!(
            world.resource_mut::<GameQueries>().take(request),
            Some(Ok(answer()))
        );
        assert!(
            matches!(
                world.resource_mut::<GameQueries>().take(request),
                Some(Err(_))
            ),
            "an answer is taken once"
        );
    }

    #[test]
    fn the_watch_follows_the_selection_and_stops_when_it_is_cleared() {
        let (mut world, node, mut game_end) = playing();

        sync_watch(&mut world);
        assert!(
            reaching_game(&mut game_end, Duration::from_millis(50)).is_empty(),
            "nothing selected, nothing watched"
        );

        world.resource_mut::<Selection>().entities.push(node);
        sync_watch(&mut world);
        let ControlEvent::Watch { watch, target } = next_control(&mut game_end) else {
            panic!("expected a watch");
        };
        assert_eq!(target, Some(GameEntityRef::Places(at_the_node())));
        reply(
            &mut game_end,
            &StateEvent::Watched {
                watch,
                entities: vec![bullet()],
            },
        );
        drain_until(&mut world, |world| {
            !world.resource::<GameQueries>().watched().is_empty()
        });
        assert_eq!(world.resource::<GameQueries>().watched(), &[bullet()]);

        sync_watch(&mut world);
        assert!(
            reaching_game(&mut game_end, Duration::from_millis(50)).is_empty(),
            "the same selection is not sent again"
        );

        world.resource_mut::<Selection>().entities.clear();
        sync_watch(&mut world);
        let ControlEvent::Watch {
            watch: stopped,
            target,
        } = next_control(&mut game_end)
        else {
            panic!("expected the watch to stop");
        };
        assert_eq!(target, None);
        assert!(stopped > watch);
        assert!(world.resource::<GameQueries>().watched().is_empty());

        reply(
            &mut game_end,
            &StateEvent::Watched {
                watch,
                entities: vec![bullet()],
            },
        );
        std::thread::sleep(Duration::from_millis(50));
        crate::pie::drain_game_events(&mut world);
        assert!(
            world.resource::<GameQueries>().watched().is_empty(),
            "values of a watch that ended are dropped"
        );
    }

    #[test]
    fn a_runtime_entity_picked_is_watched_in_place_of_the_selection() {
        let (mut world, node, mut game_end) = playing();
        world.resource_mut::<Selection>().entities.push(node);
        sync_watch(&mut world);
        next_control(&mut game_end);
        pick_runtime_entity(&mut world, 42);
        sync_watch(&mut world);
        let ControlEvent::Watch { target, .. } = next_control(&mut game_end) else {
            panic!("expected a watch");
        };
        assert_eq!(target, Some(GameEntityRef::Entity(42)));

        let other = world.spawn_empty().id();
        world.resource_mut::<Selection>().entities = vec![other];
        sync_watch(&mut world);
        assert_eq!(world.resource::<GameQueries>().runtime_pick(), None);
        let ControlEvent::Watch { target, .. } = next_control(&mut game_end) else {
            panic!("expected the watch to move");
        };
        assert_ne!(
            target,
            Some(GameEntityRef::Entity(42)),
            "selecting another entity ends the pick"
        );
    }

    #[test]
    fn a_runtime_pick_ends_when_another_game_takes_focus() {
        let (mut world, _, mut game_end) = playing();
        pick_runtime_entity(&mut world, 42);
        sync_watch(&mut world);
        next_control(&mut game_end);
        let (editor, _other_end) = link();
        let child = Command::new(std::env::current_exe().expect("the test binary"))
            .arg("--list")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("a child process");
        crate::pie::connect_test_game(&mut world, game(2), child, editor).expect("a link");

        sync_watch(&mut world);
        assert_eq!(
            world.resource::<GameQueries>().runtime_pick(),
            None,
            "the other game's entity ids mean nothing here"
        );
        assert_ne!(
            world.resource::<GameQueries>().watch_target(),
            Some(&GameEntityRef::Entity(42))
        );
    }

    #[test]
    fn nothing_goes_to_an_idle_game() {
        let (mut world, _, mut game_end) = playing();
        for _ in 0..20 {
            crate::pie::drain_game_events(&mut world);
            sync_watch(&mut world);
            expire_game_queries(&mut world);
        }
        assert!(reaching_game(&mut game_end, Duration::from_millis(100)).is_empty());
    }

    #[test]
    fn an_unanswered_question_expires_and_a_stopped_game_is_not_asked() {
        let (mut world, _, _game_end) = playing();
        let request = ask_game(&mut world, GameQuery::Runtime).expect("a game is running");
        let mut queries = world.resource_mut::<GameQueries>();
        queries.expire(Instant::now(), &[game(1)]);
        assert_eq!(queries.take(request), None, "still within its time");
        queries.expire(Instant::now() + QUERY_TIMEOUT, &[game(1)]);
        let answer = queries.take(request).expect("it expired");
        assert!(answer.is_err());

        let request = ask_game(&mut world, GameQuery::Runtime).expect("a game is running");
        let mut queries = world.resource_mut::<GameQueries>();
        queries.expire(Instant::now(), &[]);
        assert!(
            matches!(queries.take(request), Some(Err(_))),
            "a game that stopped answers nothing"
        );

        world.resource_mut::<crate::pie::PieInstances>().focused = None;
        assert!(ask_game(&mut world, GameQuery::Runtime).is_err());
    }
}
