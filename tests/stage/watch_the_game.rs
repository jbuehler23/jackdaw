//! Watching the running game: the Outliner's Runtime group lists what the game
//! spawned, a picked runtime entity shows read-only in the Inspector, and a
//! selected entity's fields carry the game's value where it differs.

use std::time::{Duration, Instant};

use bevy::prelude::*;
use jackdaw::boot_ops::run_op_clause;
use jackdaw::pie::InstanceKey;
use jackdaw::runtime_outliner::{RUNTIME_REFRESH, runtime_group_labels, runtime_group_open};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_pie_protocol::event::{from_bytes, to_bytes};
use jackdaw_pie_protocol::{
    ControlEvent, GameEntities, GameEntity, GameEntityRef, GameQuery, IpcChannelTransport,
    PieChannel, PieTransport as _, StateEvent,
};
use serde_json::json;

use crate::util::{self, OperatorResultExt as _};

const GUARD: Duration = Duration::from_secs(30);
const TRANSFORM: &str = "bevy_transform::components::transform::Transform";

struct Watching {
    app: App,
    game: IpcChannelTransport,
    zone: Entity,
    _dir: tempfile::TempDir,
}

/// An editor with an Outliner and an Inspector on a project whose scene holds a
/// `Zone`, no game running yet, and the editor's end of a link to one.
fn editor_and_link() -> (Watching, IpcChannelTransport) {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("assets")).expect("assets folder");
    std::fs::write(
        dir.path().join("assets/zone.bsn"),
        format!("#Zone\n{TRANSFORM}\n"),
    )
    .expect("scene written");
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: dir.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    run_op_clause(app.world_mut(), "scene.open path=assets/zone.bsn")
        .expect("the scene opens")
        .assert_finished();
    for _ in 0..4 {
        app.update();
    }
    let zone = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .find(|(_, name)| name.as_str() == "Zone")
        .map(|(entity, _)| entity)
        .expect("the scene's Zone");
    let (server, name) = jackdaw_pie_protocol::serve().expect("a rendezvous");
    let accept = std::thread::spawn(move || server.accept());
    let game = jackdaw_pie_protocol::connect(&name).expect("the game connects");
    let editor_end = accept
        .join()
        .expect("accept ran")
        .expect("the editor accepts");
    (
        Watching {
            app,
            game,
            zone,
            _dir: dir,
        },
        editor_end,
    )
}

impl Drop for Watching {
    fn drop(&mut self) {
        jackdaw::pie::handle_stop(self.app.world_mut());
    }
}

fn key() -> InstanceKey {
    InstanceKey {
        config: "Game".to_string(),
        instance: 1,
    }
}

impl Watching {
    /// Connect the game, as if it had launched and asked for the scene in front.
    fn play(&mut self, editor_end: IpcChannelTransport) {
        let child = std::process::Command::new("sleep")
            .arg("600")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("a child process");
        jackdaw::pie::connect_test_game(self.app.world_mut(), key(), child, editor_end)
            .expect("a link");
        jackdaw::play_scene::note_test_asked_scene(self.app.world_mut(), &key(), "zone.bsn");
        self.app.update();
    }

    /// Run frames for `wait`, collecting what reaches the game.
    fn run_for(&mut self, wait: Duration) -> Vec<ControlEvent> {
        let until = Instant::now() + wait;
        let mut events = Vec::new();
        while Instant::now() < until {
            self.app.update();
            events.extend(self.received());
            std::thread::sleep(Duration::from_millis(2));
        }
        events
    }

    fn received(&mut self) -> Vec<ControlEvent> {
        self.game
            .drain_received()
            .into_iter()
            .map(|(_, bytes)| from_bytes::<ControlEvent>(&bytes).expect("a control event"))
            .collect()
    }

    /// Run frames until the game receives an event `pick` accepts.
    fn next<T>(&mut self, mut pick: impl FnMut(ControlEvent) -> Option<T>) -> T {
        let until = Instant::now() + GUARD;
        loop {
            assert!(
                Instant::now() < until,
                "nothing of the kind reached the game"
            );
            self.app.update();
            for event in self.received() {
                if let Some(found) = pick(event) {
                    return found;
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn reply(&mut self, event: &StateEvent) {
        self.game
            .send(PieChannel::Reliable, &to_bytes(event).expect("encode"));
    }

    /// Run frames until `done` holds.
    fn until(&mut self, mut done: impl FnMut(&mut World) -> bool) {
        let until = Instant::now() + GUARD;
        while !done(self.app.world_mut()) {
            assert!(Instant::now() < until, "it never happened");
            self.app.update();
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn open_group(&mut self, on: bool) {
        self.app
            .world_mut()
            .operator("hierarchy.runtime_group")
            .param("on", on)
            .call()
            .expect("dispatch")
            .assert_finished();
        self.app.update();
    }

    fn pick(&mut self, entity: i64) {
        self.app
            .world_mut()
            .operator("hierarchy.pick_runtime")
            .param("entity", entity)
            .call()
            .expect("dispatch")
            .assert_finished();
        self.app.update();
    }

    /// Answer the next runtime question with `found`.
    fn answer_runtime(&mut self, found: GameEntities) {
        let request = self.next(|event| match event {
            ControlEvent::Query {
                request,
                query: GameQuery::Runtime,
            } => Some(request),
            _ => None,
        });
        self.reply(&StateEvent::QueryAnswer {
            request,
            answer: Ok(found),
        });
    }
}

fn spawned(entity: u64, name: Option<&str>, parent: Option<u64>) -> GameEntity {
    GameEntity {
        entity,
        name: name.map(str::to_string),
        parent,
        node: None,
        components: Vec::new(),
        opaque: Vec::new(),
    }
}

fn runtime_answer() -> GameEntities {
    let zone = GameEntity {
        node: Some(1),
        ..spawned(1, Some("Zone"), None)
    };
    GameEntities {
        entities: vec![
            zone,
            spawned(42, Some("Bullet"), Some(1)),
            spawned(43, Some("Spark"), Some(42)),
            spawned(44, None, None),
        ],
        truncated: true,
    }
}

fn queries(events: &[ControlEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, ControlEvent::Query { .. }))
        .count()
}

#[test]
fn the_runtime_group_shows_only_while_playing_and_lists_what_the_game_spawned() {
    let (mut watching, editor_end) = editor_and_link();
    assert_eq!(
        runtime_group_open(watching.app.world_mut()),
        None,
        "no group with no game"
    );

    watching.play(editor_end);
    assert_eq!(
        runtime_group_open(watching.app.world_mut()),
        Some(false),
        "the group comes up closed"
    );

    watching.open_group(true);
    watching.answer_runtime(runtime_answer());
    watching.until(|world| runtime_group_labels(world).is_some_and(|labels| labels.len() > 1));
    assert_eq!(
        runtime_group_labels(watching.app.world_mut()),
        Some(vec![
            "Bullet".to_string(),
            "Spark".to_string(),
            "Entity 44".to_string(),
            "More entities than shown".to_string(),
        ]),
        "runtime entities under their runtime parent, the authored Zone left out"
    );

    watching.pick(42);
    watching.until(|world| jackdaw::runtime_outliner::runtime_row_picked(world) == Some(42));

    jackdaw::pie::handle_stop(watching.app.world_mut());
    watching.app.update();
    assert_eq!(runtime_group_open(watching.app.world_mut()), None);
}

#[test]
fn a_closed_group_or_a_stopped_game_is_asked_nothing() {
    let (mut watching, editor_end) = editor_and_link();
    watching.play(editor_end);
    assert_eq!(
        queries(&watching.run_for(RUNTIME_REFRESH + Duration::from_millis(300))),
        0,
        "a closed group asks nothing"
    );

    watching.open_group(true);
    watching.answer_runtime(runtime_answer());
    watching.open_group(false);
    assert_eq!(
        queries(&watching.run_for(RUNTIME_REFRESH + Duration::from_millis(300))),
        0,
        "closing it stops the questions"
    );

    watching.open_group(true);
    watching.answer_runtime(runtime_answer());
    watching.answer_runtime(runtime_answer());

    jackdaw::pie::handle_stop(watching.app.world_mut());
    watching.received();
    assert_eq!(
        queries(&watching.run_for(RUNTIME_REFRESH + Duration::from_millis(300))),
        0,
        "a stopped game is asked nothing"
    );
}

#[test]
fn a_picked_runtime_entity_shows_read_only() {
    let (mut watching, editor_end) = editor_and_link();
    let zone = watching.zone;
    watching.play(editor_end);
    watching
        .app
        .world_mut()
        .resource_scope(|world, mut selection: Mut<Selection>| {
            let mut commands = world.commands();
            selection.select_single(&mut commands, zone);
        });
    watching.app.update();

    watching.pick(42);
    assert!(
        watching
            .app
            .world()
            .resource::<Selection>()
            .primary()
            .is_none(),
        "picking a runtime entity clears the selection"
    );
    let watch = watching.next(|event| match event {
        ControlEvent::Watch {
            watch,
            target: Some(GameEntityRef::Entity(42)),
        } => Some(watch),
        _ => None,
    });
    let bullet = GameEntity {
        components: vec![
            ("game::Speed".to_string(), json!(2.5)),
            (
                TRANSFORM.to_string(),
                json!({
                    "translation": [1.0, 2.0, 3.0],
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0],
                }),
            ),
        ],
        opaque: vec!["game::Brain".to_string()],
        ..spawned(42, Some("Bullet"), None)
    };
    watching.reply(&StateEvent::Watched {
        watch,
        entities: vec![bullet],
    });
    watching.until(|world| {
        jackdaw::inspector::runtime_card::runtime_card_text(world)
            .is_some_and(|lines| lines.iter().any(|line| line == "Speed"))
    });
    let lines = jackdaw::inspector::runtime_card::runtime_card_text(watching.app.world_mut())
        .expect("the card");
    for line in [
        "Bullet",
        "Speed",
        "Value: 2.5",
        "Transform",
        "Translation: 1, 2, 3",
        "Not readable: Brain",
    ] {
        assert!(lines.iter().any(|held| held == line), "{line} in {lines:?}");
    }

    let world = watching.app.world_mut();
    let mut inspectors = world.query_filtered::<Entity, With<jackdaw::inspector::Inspector>>();
    let mut stack: Vec<Entity> = inspectors.iter(world).collect();
    assert!(!stack.is_empty(), "the editor has an inspector");
    while let Some(node) = stack.pop() {
        assert!(
            jackdaw::inspector::field_edited_by(world, node).is_none(),
            "no field control"
        );
        assert!(
            world.get::<bevy::text::EditableText>(node).is_none(),
            "no text input"
        );
        assert!(
            world.get::<bevy::ui_widgets::Button>(node).is_none(),
            "no button"
        );
        if let Some(children) = world.get::<Children>(node) {
            stack.extend(children.iter());
        }
    }
    assert!(
        !jackdaw::inspector::runtime_card::add_component_offered(world),
        "no add component"
    );

    watching
        .app
        .world_mut()
        .resource_scope(|world, mut selection: Mut<Selection>| {
            let mut commands = world.commands();
            selection.select_single(&mut commands, zone);
        });
    for _ in 0..4 {
        watching.app.update();
    }
    assert_eq!(
        jackdaw::inspector::runtime_card::runtime_card_text(watching.app.world_mut()),
        None,
        "selecting an entity ends the read-only view"
    );
}

fn transform_chips(world: &mut World) -> Vec<(String, String)> {
    jackdaw::inspector::live_values::live_value_chips(world)
        .into_iter()
        .filter(|(type_path, _, _)| type_path == TRANSFORM)
        .map(|(_, field, text)| (field, text))
        .collect()
}

impl Watching {
    /// Select the `Zone` and return the watch the game is told of.
    fn select_zone(&mut self) -> u64 {
        let zone = self.zone;
        self.app
            .world_mut()
            .resource_scope(|world, mut selection: Mut<Selection>| {
                let mut commands = world.commands();
                selection.select_single(&mut commands, zone);
            });
        self.next(|event| match event {
            ControlEvent::Watch {
                watch,
                target: Some(GameEntityRef::Places(_)),
            } => Some(watch),
            _ => None,
        })
    }

    /// The `Zone`'s scene node id.
    fn zone_node(&self) -> u64 {
        let ast = self.app.world().resource::<jackdaw_bsn::SceneBsnAst>();
        ast.stable_id_of(ast.ast_for(self.zone).expect("the Zone is in the document"))
            .expect("the Zone has a node id")
    }

    /// The `Zone` as the game holds it, spawned from `node`.
    fn zone_in_game(&self, node: u64, translation: [f32; 3]) -> GameEntity {
        GameEntity {
            node: Some(node),
            components: vec![(
                TRANSFORM.to_string(),
                json!({
                    "translation": translation,
                    "rotation": [0.0, 0.0, 0.0, 1.0],
                    "scale": [1.0, 1.0, 1.0],
                }),
            )],
            ..spawned(7, Some("Zone"), None)
        }
    }

    fn send_watched(&mut self, watch: u64, entities: Vec<GameEntity>) {
        self.reply(&StateEvent::Watched { watch, entities });
    }
}

#[test]
fn a_drifted_field_shows_the_game_value_until_it_agrees_or_play_stops() {
    let (mut watching, editor_end) = editor_and_link();
    watching.play(editor_end);
    let watch = watching.select_zone();
    let node = watching.zone_node();

    let drifted = watching.zone_in_game(node, [5.0, 0.5, 0.0]);
    watching.send_watched(watch, vec![drifted.clone()]);
    watching.until(|world| !transform_chips(world).is_empty());
    assert_eq!(
        transform_chips(watching.app.world_mut()),
        vec![("translation".to_string(), "5, 0.5, 0".to_string())],
        "only the drifted field carries a chip"
    );
    assert_eq!(
        jackdaw::inspector::live_values::live_card_markers(watching.app.world_mut()),
        1,
        "the collapsed Transform card is marked"
    );

    let agreeing = watching.zone_in_game(node, [0.0, 0.0, 0.0]);
    watching.send_watched(watch, vec![agreeing]);
    watching.until(|world| transform_chips(world).is_empty());
    assert_eq!(
        jackdaw::inspector::live_values::live_card_markers(watching.app.world_mut()),
        0
    );

    watching.send_watched(watch, vec![drifted]);
    watching.until(|world| !transform_chips(world).is_empty());
    jackdaw::pie::handle_stop(watching.app.world_mut());
    for _ in 0..3 {
        watching.app.update();
    }
    assert!(
        transform_chips(watching.app.world_mut()).is_empty(),
        "stopping takes the chip away"
    );
}

/// The editor's own copy of an entity can hold what the document does not, as
/// a camera the editor keeps switched off. The chip compares with the document.
#[test]
fn the_chip_compares_the_game_with_the_document_not_the_editor() {
    let (mut watching, editor_end) = editor_and_link();
    watching.play(editor_end);
    let watch = watching.select_zone();
    let node = watching.zone_node();
    let zone = watching.zone;
    watching
        .app
        .world_mut()
        .get_mut::<Transform>(zone)
        .expect("a Transform")
        .bypass_change_detection()
        .translation = Vec3::splat(9.0);

    let agreeing = watching.zone_in_game(node, [0.0, 0.0, 0.0]);
    watching.send_watched(watch, vec![agreeing]);
    watching.until(|world| {
        world
            .resource::<jackdaw::game_query::GameQueries>()
            .watch_answered()
    });
    for _ in 0..4 {
        watching.app.update();
    }
    assert!(
        transform_chips(watching.app.world_mut()).is_empty(),
        "the game agrees with the document"
    );
}

#[test]
fn an_answer_that_is_not_the_selected_node_shows_no_chip() {
    let (mut watching, editor_end) = editor_and_link();
    watching.play(editor_end);
    let watch = watching.select_zone();
    let node = watching.zone_node();

    let elsewhere = watching.zone_in_game(node + 1, [5.0, 0.0, 0.0]);
    watching.send_watched(watch, vec![elsewhere]);
    let drifted = watching.zone_in_game(node, [5.0, 0.0, 0.0]);
    let twice = vec![drifted.clone(), drifted.clone()];
    watching.send_watched(watch, twice);
    watching.until(|world| {
        world
            .resource::<jackdaw::game_query::GameQueries>()
            .watched()
            .len()
            == 2
    });
    for _ in 0..4 {
        watching.app.update();
    }
    assert!(
        transform_chips(watching.app.world_mut()).is_empty(),
        "another node, or one node spawned twice, is not compared"
    );

    watching.send_watched(watch, vec![drifted]);
    watching.until(|world| !transform_chips(world).is_empty());
}

#[test]
fn a_picked_entity_the_game_despawned_says_so() {
    let (mut watching, editor_end) = editor_and_link();
    watching.play(editor_end);
    watching.pick(42);
    let watch = watching.next(|event| match event {
        ControlEvent::Watch {
            watch,
            target: Some(GameEntityRef::Entity(42)),
        } => Some(watch),
        _ => None,
    });
    watching.send_watched(watch, Vec::new());
    watching.until(|world| {
        jackdaw::inspector::runtime_card::runtime_card_text(world)
            .is_some_and(|lines| lines.iter().any(|line| line == "No longer in the game"))
    });
}

#[test]
fn focusing_another_game_forgets_the_pick_and_the_runtime_rows() {
    let (mut watching, editor_end) = editor_and_link();
    watching.play(editor_end);
    watching.open_group(true);
    watching.answer_runtime(runtime_answer());
    watching.until(|world| runtime_group_labels(world).is_some_and(|labels| labels.len() > 1));
    watching.pick(42);

    let (server, name) = jackdaw_pie_protocol::serve().expect("a rendezvous");
    let accept = std::thread::spawn(move || server.accept());
    let mut other = jackdaw_pie_protocol::connect(&name).expect("the game connects");
    let editor_end = accept
        .join()
        .expect("accept ran")
        .expect("the editor accepts");
    let child = std::process::Command::new("sleep")
        .arg("600")
        .spawn()
        .expect("a child process");
    let second = InstanceKey {
        config: "Game".to_string(),
        instance: 2,
    };
    jackdaw::pie::connect_test_game(watching.app.world_mut(), second, child, editor_end)
        .expect("a link");
    watching.app.update();
    watching.app.update();

    let world = watching.app.world_mut();
    assert_eq!(
        world
            .resource::<jackdaw::game_query::GameQueries>()
            .runtime_pick(),
        None
    );
    assert_ne!(runtime_group_labels(world).unwrap_or_default().len(), 4);
    let until = Instant::now() + GUARD;
    let mut asked = false;
    while !asked {
        assert!(Instant::now() < until, "the new game was never asked");
        watching.app.update();
        for (_, bytes) in other.drain_received() {
            match from_bytes::<ControlEvent>(&bytes).expect("a control event") {
                ControlEvent::Query {
                    query: GameQuery::Runtime,
                    ..
                } => asked = true,
                ControlEvent::Watch {
                    target: Some(GameEntityRef::Entity(_)),
                    ..
                } => panic!("the other game's pick reached this one"),
                _ => {}
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
