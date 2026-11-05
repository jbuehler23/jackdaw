//! Under Play, a scene root asks the editor for its document before it spawns.
//!
//! [`install_link`](crate::pie) installs [`SceneRequests`], so a game with no
//! editor link never reaches any of this. A root on a default-source asset path
//! sends one [`StateEvent::RequestScene`] and is held; roots on the same path
//! share a request. The answer either carries the text to spawn or sends the
//! root to its file. The main thread never waits: a root is only looked at again
//! the next update, and after [`ANSWER_TIMEOUT`] of real time without an answer
//! it loads its file and a late answer is ignored.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use bevy::asset::io::AssetSourceId;
use bevy::asset::{AssetPath, AssetServer};
use bevy::prelude::*;
use jackdaw_pie_protocol::SceneAnswer;
use jackdaw_pie_protocol::event::{PieChannel, StateEvent, to_bytes};

use crate::pie::PieTransportRes;
use crate::{JackdawScene, JackdawSceneRoot};

/// How long a root waits for the editor before it loads its file.
pub(crate) const ANSWER_TIMEOUT: Duration = Duration::from_secs(10);

/// The requests sent to the editor and the answers not yet taken.
#[derive(Resource, Default)]
pub(crate) struct SceneRequests {
    next: u64,
    open: HashMap<u64, OpenRequest>,
    /// Answers that arrived this update, taken by the roots waiting on them in
    /// the next spawn pass.
    answered: HashMap<u64, SceneAnswer>,
}

struct OpenRequest {
    path: String,
    asked_at: Duration,
}

/// A root waiting on the request it holds.
#[derive(Component, Clone, Copy)]
struct AwaitingDocument(u64);

/// A root told to load its file, by the editor or by its silence.
#[derive(Component)]
struct SceneFromFile;

/// What a spawn pass does with a root.
pub(crate) enum Gate {
    /// Spawn from the handle's loaded asset, as a game with no link does.
    File,
    /// Look again next update.
    Hold,
    /// Spawn this text.
    Document {
        text: String,
        dir: String,
        stem: String,
    },
}

fn real_now(world: &World) -> Duration {
    world
        .get_resource::<Time<Real>>()
        .map_or(Duration::ZERO, Time::elapsed)
}

/// The path a handle can be asked for by: default source, no label. Other
/// handles load the way they always did.
fn servable_path(handle: &Handle<JackdawScene>) -> Option<String> {
    let path = handle.path()?;
    if *path.source() != AssetSourceId::Default || path.label().is_some() {
        return None;
    }
    Some(path.path().to_string_lossy().into_owned())
}

/// Give up on requests that went unanswered too long, so the roots waiting on
/// them load their files in the pass that follows.
pub(crate) fn begin_pass(world: &mut World) {
    let now = real_now(world);
    let Some(mut requests) = world.get_resource_mut::<SceneRequests>() else {
        return;
    };
    requests.open.retain(|_, open| {
        let alive = now.saturating_sub(open.asked_at) < ANSWER_TIMEOUT;
        if !alive {
            warn!(
                "PIE: the editor did not answer for scene {} within {} s; loading its file",
                open.path,
                ANSWER_TIMEOUT.as_secs()
            );
        }
        alive
    });
}

/// Every root waiting on an answer took it during the pass, so none are kept.
pub(crate) fn end_pass(world: &mut World) {
    if let Some(mut requests) = world.get_resource_mut::<SceneRequests>() {
        requests.answered.clear();
    }
}

/// Decide what the spawn pass does with `root`.
pub(crate) fn gate(world: &mut World, root: Entity, handle: &Handle<JackdawScene>) -> Gate {
    if !world.contains_resource::<SceneRequests>() || world.get::<SceneFromFile>(root).is_some() {
        return Gate::File;
    }
    let Some(path) = servable_path(handle) else {
        return Gate::File;
    };
    let Some(&AwaitingDocument(request)) = world.get::<AwaitingDocument>(root) else {
        let request = ask(world, &path);
        world.entity_mut(root).insert(AwaitingDocument(request));
        return Gate::Hold;
    };

    let requests = world.resource::<SceneRequests>();
    let answer = requests.answered.get(&request).cloned();
    if answer.is_none() && requests.open.contains_key(&request) {
        return Gate::Hold;
    }
    let mut root_entity = world.entity_mut(root);
    root_entity.remove::<AwaitingDocument>();
    match answer {
        Some(SceneAnswer::Document { text, dir, stem }) => Gate::Document { text, dir, stem },
        Some(SceneAnswer::UseFile { path: file }) if file != path => {
            root_entity.insert(SceneFromFile);
            let handle = world
                .resource::<AssetServer>()
                .load(AssetPath::from_path(Path::new(&file)).into_owned());
            if let Some(mut scene_root) = world.get_mut::<JackdawSceneRoot>(root) {
                scene_root.0 = handle;
            }
            Gate::Hold
        }
        Some(SceneAnswer::UseFile { .. }) | None => {
            root_entity.insert(SceneFromFile);
            Gate::File
        }
    }
}

/// The request for `path`: the one already open, or a new one sent now.
fn ask(world: &mut World, path: &str) -> u64 {
    let now = real_now(world);
    let mut requests = world.resource_mut::<SceneRequests>();
    if let Some((&request, _)) = requests.open.iter().find(|(_, open)| open.path == path) {
        return request;
    }
    let request = requests.next;
    requests.next += 1;
    requests.open.insert(
        request,
        OpenRequest {
            path: path.to_string(),
            asked_at: now,
        },
    );
    let event = StateEvent::RequestScene {
        request,
        path: path.to_string(),
    };
    match to_bytes(&event) {
        Ok(bytes) => {
            if let Some(mut transport) = world.get_non_send_mut::<PieTransportRes>() {
                transport.0.send(PieChannel::Reliable, &bytes);
            }
        }
        Err(err) => error!("PIE: failed to encode a scene request: {err}"),
    }
    request
}

/// Take the editor's answer to `request`. One that gave up already is ignored.
pub(crate) fn receive(world: &mut World, request: u64, answer: SceneAnswer) {
    let Some(mut requests) = world.get_resource_mut::<SceneRequests>() else {
        return;
    };
    if requests.open.remove(&request).is_none() {
        debug!("PIE: ignoring a late answer to scene request {request}");
        return;
    }
    requests.answered.insert(request, answer);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Instant;

    use bevy::ecs::message::Messages;
    use jackdaw_pie_protocol::event::{ControlEvent, from_bytes};
    use jackdaw_pie_protocol::{LoopbackTransport, PieTransport, connect, serve};
    use jackdaw_scene_types::SceneNodeId;

    use super::*;
    use crate::pie::install_link;
    use crate::{JackdawPlugin, ServedScene};

    const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
    const NAME: &str = "bevy_ecs::name::Name";
    const NODE: u64 = 21;

    /// One root node with a stable id at `x`.
    fn scene_text(x: f32) -> String {
        format!(
            "jackdaw_scene_types::node_id::SceneNodeId({NODE})\n\
             bevy_transform::components::transform::Transform {{\n    \
             translation: glam::Vec3 {{ x: {x:?}, y: 0.0, z: 0.0 }},\n}}\n"
        )
    }

    fn transform_json(x: f32) -> serde_json::Value {
        serde_json::json!({
            "translation": [x, 0.0, 0.0],
            "rotation": [0.0, 0.0, 0.0, 1.0],
            "scale": [1.0, 1.0, 1.0]
        })
    }

    fn base_app(dir: &Path) -> App {
        base_app_with(dir, None)
    }

    fn base_app_with(dir: &Path, play_scene: Option<crate::PieSceneOverride>) -> App {
        let mut app = App::new();
        if let Some(play_scene) = play_scene {
            app.insert_resource(play_scene);
        }
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::transform::TransformPlugin);
        app.add_plugins(bevy::asset::AssetPlugin {
            file_path: dir.to_string_lossy().into_owned(),
            ..Default::default()
        });
        app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
        app.add_plugins(JackdawPlugin);
        app.register_type::<Name>();
        app
    }

    /// A game on the link, its asset folder holding `scene.bsn` at x = 1 and
    /// `other.bsn` at x = 2, and the editor's end of the link.
    fn game() -> (App, LoopbackTransport, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("scene.bsn"), scene_text(1.0)).expect("a scene");
        std::fs::write(dir.path().join("other.bsn"), scene_text(2.0)).expect("a scene");
        let (editor, game) = LoopbackTransport::pair();
        let mut app = base_app(dir.path());
        install_link(&mut app, Box::new(game));
        (app, editor, dir)
    }

    fn spawn_root(app: &mut App, path: &str) -> Entity {
        let handle: Handle<JackdawScene> =
            app.world().resource::<AssetServer>().load(path.to_string());
        app.world_mut().spawn(JackdawSceneRoot(handle)).id()
    }

    fn requests_from(editor: &mut impl PieTransport) -> Vec<(u64, String)> {
        editor
            .drain_received()
            .into_iter()
            .filter_map(|(_, bytes)| match from_bytes(&bytes) {
                Ok(StateEvent::RequestScene { request, path }) => Some((request, path)),
                _ => None,
            })
            .collect()
    }

    fn send(editor: &mut impl PieTransport, event: &ControlEvent) {
        editor.send(PieChannel::Reliable, &to_bytes(event).expect("encode"));
    }

    fn document(request: u64, text: String) -> ControlEvent {
        ControlEvent::SceneDocument {
            request,
            answer: SceneAnswer::Document {
                text,
                dir: String::new(),
                stem: "scene".to_string(),
            },
        }
    }

    fn use_file(request: u64, path: &str) -> ControlEvent {
        ControlEvent::SceneDocument {
            request,
            answer: SceneAnswer::UseFile {
                path: path.to_string(),
            },
        }
    }

    fn node_edit(x: f32) -> ControlEvent {
        ControlEvent::NodeComponents {
            node: NODE,
            set: vec![(TRANSFORM.to_string(), transform_json(x))],
            removed: Vec::new(),
        }
    }

    fn node_xs(app: &mut App) -> Vec<f32> {
        let mut query = app.world_mut().query::<(&SceneNodeId, &Transform)>();
        let mut xs: Vec<f32> = query
            .iter(app.world())
            .filter(|(id, _)| id.0 == NODE)
            .map(|(_, transform)| transform.translation.x)
            .collect();
        xs.sort_by(f32::total_cmp);
        xs
    }

    fn node_entities(app: &mut App) -> Vec<Entity> {
        let mut query = app
            .world_mut()
            .query_filtered::<Entity, With<SceneNodeId>>();
        query.iter(app.world()).collect()
    }

    /// Update until the scene's node exists, with a bound on the file load.
    fn update_until_spawned(app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while node_xs(app).is_empty() {
            assert!(Instant::now() < deadline, "the scene never spawned");
            app.update();
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn a_root_asks_without_blocking_and_spawns_the_answer_over_its_file() {
        let (mut app, mut editor, dir) = game();
        let root = spawn_root(&mut app, "scene.bsn");
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(
            requests_from(&mut editor),
            vec![(0, "scene.bsn".to_string())],
            "one request, sent once"
        );
        assert!(node_xs(&mut app).is_empty(), "held, whatever the file does");

        send(&mut editor, &document(0, scene_text(5.0)));
        app.update();

        assert_eq!(node_xs(&mut app), vec![5.0]);
        assert!(app.world().get::<ServedScene>(root).is_some());
        assert_eq!(
            app.world().get::<Name>(root).map(Name::as_str),
            Some("scene")
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("scene.bsn")).expect("the file"),
            scene_text(1.0),
            "nothing was written"
        );
    }

    #[test]
    fn roots_on_one_path_share_a_request_and_other_paths_ask_their_own() {
        let (mut app, mut editor, _dir) = game();
        spawn_root(&mut app, "scene.bsn");
        spawn_root(&mut app, "scene.bsn");
        spawn_root(&mut app, "other.bsn");
        app.update();
        let mut asked = requests_from(&mut editor);
        asked.sort_by(|a, b| a.1.cmp(&b.1));
        assert_eq!(asked.len(), 2, "{asked:?}");
        assert_eq!(asked[0].1, "other.bsn");
        assert_eq!(asked[1].1, "scene.bsn");

        send(&mut editor, &document(asked[1].0, scene_text(5.0)));
        send(&mut editor, &document(asked[0].0, scene_text(6.0)));
        app.update();
        assert_eq!(node_xs(&mut app), vec![5.0, 5.0, 6.0]);
    }

    /// A game told to play a scene asks for that scene, not its usual one.
    #[test]
    fn a_redirected_root_asks_for_the_scene_it_was_told() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let (mut editor, game) = LoopbackTransport::pair();
        let mut app = base_app_with(
            dir.path(),
            Some(crate::PieSceneOverride::new(
                "levels/meadow.bsn",
                "scene.bsn",
            )),
        );
        install_link(&mut app, Box::new(game));
        spawn_root(&mut app, "scene.bsn");
        spawn_root(&mut app, "hud.bsn");
        app.update();
        let mut asked: Vec<String> = requests_from(&mut editor)
            .into_iter()
            .map(|(_, path)| path)
            .collect();
        asked.sort();
        assert_eq!(asked, vec!["hud.bsn", "levels/meadow.bsn"]);
    }

    #[test]
    fn use_file_loads_the_file_named() {
        let (mut app, mut editor, _dir) = game();
        let same = spawn_root(&mut app, "scene.bsn");
        app.update();
        let (request, _) = requests_from(&mut editor)[0];
        send(&mut editor, &use_file(request, "scene.bsn"));
        update_until_spawned(&mut app);
        assert_eq!(node_xs(&mut app), vec![1.0]);
        assert!(app.world().get::<ServedScene>(same).is_none());

        let (mut app, mut editor, _dir) = game();
        let moved = spawn_root(&mut app, "scene.bsn");
        app.update();
        let (request, _) = requests_from(&mut editor)[0];
        send(&mut editor, &use_file(request, "other.bsn"));
        update_until_spawned(&mut app);
        assert_eq!(node_xs(&mut app), vec![2.0], "the root follows the answer");
        assert_eq!(
            app.world()
                .get::<JackdawSceneRoot>(moved)
                .and_then(|root| root.0.path())
                .map(ToString::to_string),
            Some("other.bsn".to_string())
        );
    }

    #[test]
    fn with_no_answer_in_time_the_file_loads_and_a_late_answer_is_ignored() {
        let (mut app, mut editor, _dir) = game();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            ANSWER_TIMEOUT / 4,
        ));
        let root = spawn_root(&mut app, "scene.bsn");
        app.update();
        app.update();
        let (request, _) = requests_from(&mut editor)[0];
        assert!(node_xs(&mut app).is_empty(), "still inside the wait");
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            ANSWER_TIMEOUT,
        ));
        update_until_spawned(&mut app);
        assert_eq!(node_xs(&mut app), vec![1.0], "the file");

        send(&mut editor, &document(request, scene_text(9.0)));
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(
            node_xs(&mut app),
            vec![1.0],
            "the late answer changes nothing"
        );
        assert!(app.world().get::<ServedScene>(root).is_none());
    }

    #[test]
    fn a_pathless_handle_never_asks() {
        let (mut app, mut editor, _dir) = game();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<JackdawScene>>()
            .add(JackdawScene::new(scene_text(3.0), PathBuf::new()));
        app.world_mut().spawn(JackdawSceneRoot(handle));
        app.update();
        assert!(requests_from(&mut editor).is_empty());
        assert_eq!(node_xs(&mut app), vec![3.0]);
    }

    #[test]
    fn with_no_link_the_path_loads_as_it_always_did() {
        let dir = tempfile::tempdir().expect("a temp dir");
        std::fs::write(dir.path().join("scene.bsn"), scene_text(1.0)).expect("a scene");
        let mut app = base_app(dir.path());
        assert!(!app.world().contains_resource::<SceneRequests>());
        let root = spawn_root(&mut app, "scene.bsn");
        update_until_spawned(&mut app);
        assert_eq!(node_xs(&mut app), vec![1.0]);
        assert!(app.world().get::<AwaitingDocument>(root).is_none());
        assert!(app.world().get::<ServedScene>(root).is_none());
    }

    #[test]
    fn an_edit_between_the_reply_and_the_spawn_is_applied() {
        let (mut app, mut editor, _dir) = game();
        spawn_root(&mut app, "scene.bsn");
        app.update();
        let (request, _) = requests_from(&mut editor)[0];
        send(&mut editor, &document(request, scene_text(5.0)));
        send(&mut editor, &node_edit(8.0));
        app.update();
        assert_eq!(node_xs(&mut app), vec![8.0]);
        assert!(
            app.world()
                .resource::<crate::node_sync::PendingNodeChanges>()
                .is_empty()
        );
    }

    /// A second root of a file that spawned earlier: an edit made while it
    /// waits reaches both, and the earlier root keeps what the game did to
    /// the components the edit did not touch.
    #[test]
    fn a_later_root_of_a_spawned_file_has_the_edit_and_the_first_keeps_runtime_state() {
        let (mut app, mut editor, _dir) = game();
        spawn_root(&mut app, "scene.bsn");
        app.update();
        let (first, _) = requests_from(&mut editor)[0];
        send(&mut editor, &document(first, scene_text(1.0)));
        app.update();
        let earlier = node_entities(&mut app)[0];
        app.world_mut()
            .get_mut::<Transform>(earlier)
            .expect("a transform")
            .translation
            .y = 40.0;

        spawn_root(&mut app, "scene.bsn");
        app.update();
        let (second, _) = requests_from(&mut editor)[0];
        assert_ne!(first, second, "a new root asks again");
        send(&mut editor, &document(second, scene_text(1.0)));
        send(
            &mut editor,
            &ControlEvent::NodeComponents {
                node: NODE,
                set: vec![(NAME.to_string(), serde_json::json!("Rock"))],
                removed: Vec::new(),
            },
        );
        app.update();

        let entities = node_entities(&mut app);
        assert_eq!(entities.len(), 2, "the first root was not respawned");
        assert!(entities.contains(&earlier));
        for entity in entities {
            assert_eq!(
                app.world().get::<Name>(entity).map(Name::as_str),
                Some("Rock")
            );
        }
        assert_eq!(
            app.world()
                .get::<Transform>(earlier)
                .expect("a transform")
                .translation
                .y,
            40.0,
            "runtime state stays"
        );
    }

    /// The edit made long ago is in the document the later scene is served.
    #[test]
    fn an_edit_long_before_a_later_scene_is_present_when_it_spawns() {
        let (mut app, mut editor, _dir) = game();
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            Duration::from_secs(1),
        ));
        send(&mut editor, &node_edit(8.0));
        for _ in 0..32 {
            app.update();
        }
        spawn_root(&mut app, "other.bsn");
        app.update();
        let (request, _) = requests_from(&mut editor)[0];
        send(&mut editor, &document(request, scene_text(8.0)));
        app.update();
        assert_eq!(node_xs(&mut app), vec![8.0]);
    }

    /// The file finishing its load, or changing on disk, after the document
    /// spawned does not despawn and respawn the scene.
    #[test]
    fn a_served_root_spawns_once_when_its_file_loads_or_changes_later() {
        let (mut app, mut editor, _dir) = game();
        let root = spawn_root(&mut app, "scene.bsn");
        app.update();
        let (request, _) = requests_from(&mut editor)[0];
        send(&mut editor, &document(request, scene_text(5.0)));
        app.update();
        let spawned = node_entities(&mut app);
        assert_eq!(spawned.len(), 1);

        let handle = app
            .world()
            .get::<JackdawSceneRoot>(root)
            .expect("the root")
            .0
            .clone();
        let deadline = Instant::now() + Duration::from_secs(20);
        while !app
            .world()
            .resource::<Assets<JackdawScene>>()
            .contains(&handle)
        {
            assert!(Instant::now() < deadline, "the file never loaded");
            app.update();
            std::thread::sleep(Duration::from_millis(2));
        }
        for event in [
            AssetEvent::LoadedWithDependencies { id: handle.id() },
            AssetEvent::Modified { id: handle.id() },
        ] {
            app.world_mut()
                .resource_mut::<Messages<AssetEvent<JackdawScene>>>()
                .write(event);
            app.update();
            app.update();
        }
        assert_eq!(node_entities(&mut app), spawned, "the same entity");
        assert_eq!(node_xs(&mut app), vec![5.0], "and the document's value");
    }

    /// A multi-megabyte reply sent while the game streams a large state to the
    /// editor: each side sends from a thread of its own the way the editor's
    /// outbox does, and the game's main thread keeps going.
    #[test]
    fn a_large_reply_while_the_game_streams_completes() {
        const LARGE: usize = 8 * 1024 * 1024;
        let (server, name) = serve().expect("a rendezvous");
        let (spawned, done) = mpsc::channel();

        let game = std::thread::spawn(move || {
            let dir = tempfile::tempdir().expect("a temp dir");
            let mut app = base_app(dir.path());
            app.register_type::<Name>();
            app.register_type::<Transform>();
            let transport = connect(&name).expect("the game connects");
            install_link(&mut app, Box::new(transport));
            let streamed = app
                .world_mut()
                .spawn((Name::new("x".repeat(LARGE)), Transform::default()))
                .id();
            spawn_root(&mut app, "scene.bsn");
            for frame in 0..10_000 {
                app.update();
                if frame < 4 {
                    app.world_mut()
                        .entity_mut(streamed)
                        .insert(Name::new(format!("{frame}").repeat(LARGE)));
                }
                if !node_xs(&mut app).is_empty() {
                    spawned.send(node_xs(&mut app)).ok();
                    return;
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        });

        let stop = Arc::new(AtomicBool::new(false));
        let editor = std::thread::spawn({
            let stop = stop.clone();
            move || {
                let mut transport = server.accept().expect("the editor accepts");
                let padding = format!("#Pad{}\n", "x".repeat(LARGE));
                let mut writers = Vec::new();
                while !stop.load(Ordering::Acquire) {
                    for (request, _) in requests_from(&mut transport) {
                        let reply = document(request, format!("{padding}{}", scene_text(5.0)));
                        let bytes = to_bytes(&reply).expect("encode");
                        let lane = transport.lane_sender(PieChannel::Reliable);
                        writers.push(std::thread::spawn(move || lane.send(bytes)));
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                writers
            }
        });

        let xs = done
            .recv_timeout(Duration::from_secs(60))
            .expect("the scene spawned with no deadlock");
        assert_eq!(xs, vec![5.0]);
        stop.store(true, Ordering::Release);
        game.join().expect("the game ran");
        editor.join().expect("the editor ran");
    }
}
