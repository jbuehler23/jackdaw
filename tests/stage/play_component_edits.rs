//! A game component added or removed while the game plays authors the
//! document: the game takes it, a save writes it, and it stays after Stop.

use std::time::{Duration, Instant};

use bevy::prelude::*;
use jackdaw::boot_ops::run_op_clause;
use jackdaw::pie::InstanceKey;
use jackdaw::selection::Selection;
use jackdaw_api::pie::PlayState;
use jackdaw_api::prelude::*;
use jackdaw_pie_protocol::event::from_bytes;
use jackdaw_pie_protocol::{ControlEvent, IpcChannelTransport, PieTransport as _};

use crate::util::{self, OperatorResultExt as _};

const GUARD: Duration = Duration::from_secs(30);
const SPINNER: &str = "mygame::motion::Spinner";

struct Playing {
    app: App,
    game: IpcChannelTransport,
    zone: Entity,
    dir: tempfile::TempDir,
}

impl Drop for Playing {
    fn drop(&mut self) {
        jackdaw::pie::handle_stop(self.app.world_mut());
    }
}

/// The game's schema: one component with a field, which the editor has no
/// registration for.
fn schema() -> jackdaw_schema::ProjectSchema {
    jackdaw_schema::ProjectSchema {
        project_crates: vec!["mygame".to_string()],
        components: vec![jackdaw_schema::TypeSchema {
            type_path: SPINNER.to_string(),
            short_name: "Spinner".to_string(),
            module_path: String::new(),
            category: String::new(),
            description: String::new(),
            editor_description: String::new(),
            hidden: false,
            asset: false,
            preview: String::new(),
            default_constructible: true,
            fields: vec![jackdaw_schema::FieldSchema {
                name: "speed".to_string(),
                type_path: "f32".to_string(),
                item_type_path: String::new(),
                asset_type_path: String::new(),
            }],
            kind: jackdaw_schema::TypeKind::Struct,
            default: None,
            variants: Vec::new(),
            entity_fields: Vec::new(),
            fills_gaps: true,
        }],
        resources: Vec::new(),
        events: Vec::new(),
        functions: Vec::new(),
        assets: Vec::new(),
        field_types: Vec::new(),
    }
}

/// An editor on a project whose game knows [`SPINNER`], with `zone.bsn` open and
/// a game connected and playing it.
fn playing() -> Playing {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("assets")).expect("assets folder");
    std::fs::write(
        dir.path().join("assets/zone.bsn"),
        "#Zone\nbevy_transform::components::transform::Transform\n",
    )
    .expect("scene written");
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: dir.path().to_path_buf(),
            config: default(),
        });
    let native = jackdaw::project_types::native_type_paths(
        &app.world().resource::<AppTypeRegistry>().read(),
    );
    app.world_mut()
        .resource_mut::<jackdaw::project_types::ProjectTypes>()
        .update(&schema(), &native);
    jackdaw::project_types::publish_document_only_types(app.world_mut());
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
    app.world_mut()
        .resource_scope(|world, mut selection: Mut<Selection>| {
            let mut commands = world.commands();
            selection.select_single(&mut commands, zone);
        });
    app.update();

    let (server, name) = jackdaw_pie_protocol::serve().expect("a rendezvous");
    let accept = std::thread::spawn(move || server.accept());
    let game = jackdaw_pie_protocol::connect(&name).expect("the game connects");
    let editor_end = accept
        .join()
        .expect("accept ran")
        .expect("the editor accepts");
    let child = std::process::Command::new("sleep")
        .arg("600")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("a child process");
    let key = InstanceKey {
        config: "Game".to_string(),
        instance: 1,
    };
    jackdaw::pie::connect_test_game(app.world_mut(), key.clone(), child, editor_end)
        .expect("a link");
    jackdaw::play_scene::note_test_asked_scene(app.world_mut(), &key, "zone.bsn");
    for _ in 0..3 {
        app.update();
    }
    let mut playing = Playing {
        app,
        game,
        zone,
        dir,
    };
    assert_eq!(playing.play_state(), PlayState::Playing);
    playing.game.drain_received();
    playing
}

impl Playing {
    fn play_state(&self) -> PlayState {
        self.app
            .world()
            .resource::<State<PlayState>>()
            .get()
            .clone()
    }

    fn component(&mut self, op: &'static str) {
        self.app
            .world_mut()
            .operator(op)
            .param("entity", self.zone)
            .param("type_path", SPINNER.to_string())
            .call()
            .expect("the operator dispatches")
            .assert_finished();
        self.app.update();
    }

    /// Run frames until the game receives a node update `pick` accepts, as
    /// `(set, removed)` type paths.
    fn next_node_update(&mut self, pick: impl Fn(&[String], &[String]) -> bool) {
        let until = Instant::now() + GUARD;
        loop {
            assert!(Instant::now() < until, "the edit never reached the game");
            self.app.update();
            for (_, bytes) in self.game.drain_received() {
                let event = from_bytes::<ControlEvent>(&bytes).expect("a control event");
                if let ControlEvent::NodeComponents { set, removed, .. } = event {
                    let set: Vec<String> = set.into_iter().map(|(path, _)| path).collect();
                    if pick(&set, &removed) {
                        return;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn in_document(&self) -> bool {
        let ast = self.app.world().resource::<jackdaw_bsn::SceneBsnAst>();
        ast.ast_for(self.zone)
            .is_some_and(|node| ast.find_patch_by_type_path(node, SPINNER).is_some())
    }

    fn save(&mut self) -> String {
        run_op_clause(self.app.world_mut(), "scene.save")
            .expect("the save dispatches")
            .assert_finished();
        for _ in 0..3 {
            self.app.update();
        }
        std::fs::read_to_string(self.dir.path().join("assets/zone.bsn")).expect("zone read")
    }

    fn stop(&mut self) {
        run_op_clause(self.app.world_mut(), "pie.stop")
            .expect("the stop dispatches")
            .assert_finished();
        for _ in 0..3 {
            self.app.update();
        }
        assert_eq!(self.play_state(), PlayState::Stopped);
    }
}

#[test]
fn a_game_component_added_during_play_reaches_the_game_the_file_and_outlives_stop() {
    let mut playing = playing();

    playing.component("component.add");
    assert!(playing.in_document(), "the add authors the document");
    playing.next_node_update(|set, _| set.iter().any(|path| path == SPINNER));
    assert!(playing.save().contains(SPINNER), "a save writes the add");

    playing.stop();
    assert!(playing.in_document(), "the add stays after Stop");
}

#[test]
fn a_game_component_removed_during_play_leaves_the_game_the_file_and_stays_gone() {
    let mut playing = playing();
    playing.component("component.add");
    playing.next_node_update(|set, _| set.iter().any(|path| path == SPINNER));

    playing.component("component.remove");
    assert!(!playing.in_document(), "the remove authors the document");
    playing.next_node_update(|_, removed| removed.iter().any(|path| path == SPINNER));
    assert!(
        !playing.save().contains(SPINNER),
        "a save writes the remove"
    );

    playing.stop();
    assert!(!playing.in_document(), "the remove holds after Stop");
}
