//! With the project set to revert them, the edits made during Play are undone
//! when the last game stops, and no redo brings them back. Left at its default,
//! Play keeps them.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::boot_ops::{run_op_clause, run_op_clause_as_user};
use jackdaw::pie::InstanceKey;
use jackdaw::play_settings::{PlayEdits, PlaySettings};
use jackdaw_api::pie::PlayState;
use jackdaw_pie_protocol::IpcChannelTransport;

use crate::util::{self, OperatorResultExt as _};

const ZONE_SCENE: &str = "#Zone\nbevy_transform::components::transform::Transform\n";
const YARD_SCENE: &str = "#Yard\nbevy_transform::components::transform::Transform\n";

struct Editor {
    app: App,
    /// The games' ends of their links, held open while they play.
    games: Vec<IpcChannelTransport>,
    dir: tempfile::TempDir,
}

impl Drop for Editor {
    fn drop(&mut self) {
        jackdaw::pie::handle_stop(self.app.world_mut());
    }
}

/// An editor on a project holding two saved scenes, with `zone.bsn` open.
fn editor() -> Editor {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("assets")).expect("assets folder");
    std::fs::write(dir.path().join("assets/zone.bsn"), ZONE_SCENE).expect("zone written");
    std::fs::write(dir.path().join("assets/yard.bsn"), YARD_SCENE).expect("yard written");
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
    let mut editor = Editor {
        app,
        games: Vec::new(),
        dir,
    };
    editor.op("scene.open path=assets/zone.bsn");
    editor
}

impl Editor {
    fn op(&mut self, clause: &str) {
        run_op_clause(self.app.world_mut(), clause)
            .expect("the operator dispatches")
            .assert_finished();
        self.settle();
    }

    fn edit(&mut self, clause: &str) {
        run_op_clause_as_user(self.app.world_mut(), clause)
            .expect("the edit dispatches")
            .assert_finished();
        self.settle();
    }

    fn settle(&mut self) {
        for _ in 0..3 {
            self.app.update();
        }
    }

    fn revert_on_stop(&mut self) {
        self.op("play.edits mode=revert");
    }

    /// Connect game instance `instance`, as if it had launched.
    fn play(&mut self, instance: u32) {
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
        jackdaw::pie::connect_test_game(self.app.world_mut(), key(instance), child, editor_end)
            .expect("a link");
        self.games.push(game);
        self.settle();
        assert_ne!(self.play_state(), PlayState::Stopped, "the game plays");
    }

    fn stop(&mut self) {
        self.op("pie.stop");
        assert_eq!(self.play_state(), PlayState::Stopped);
    }

    fn stop_instance(&mut self, instance: u32) {
        self.op(&format!(
            "pie.toggle_instance config=Game instance={instance}"
        ));
    }

    fn play_state(&self) -> PlayState {
        self.app
            .world()
            .resource::<State<PlayState>>()
            .get()
            .clone()
    }

    fn has(&mut self, name: &str) -> bool {
        self.app
            .world_mut()
            .query::<&Name>()
            .iter(self.app.world())
            .any(|held| held.as_str() == name)
    }

    fn modified(&self) -> bool {
        jackdaw::scenes::operators::active_tab_modified(self.app.world())
    }

    fn scene(&self, file: &str) -> PathBuf {
        self.dir.path().join("assets").join(file)
    }

    fn switch_to(&mut self, file: &str) {
        let path = self.scene(file);
        let index = self
            .app
            .world()
            .resource::<jackdaw::scenes::Scenes>()
            .tabs
            .iter()
            .position(|tab| tab.path.as_deref().is_some_and(|held| same(held, &path)))
            .expect("the scene is open");
        self.op(&format!("scene.switch tab={index}"));
    }
}

fn same(a: &Path, b: &Path) -> bool {
    a.canonicalize().ok() == b.canonicalize().ok()
}

fn key(instance: u32) -> InstanceKey {
    InstanceKey {
        config: "Game".to_string(),
        instance,
    }
}

#[test]
fn by_default_the_edits_made_during_play_stay_after_stop() {
    let mut editor = editor();

    editor.play(1);
    editor.edit("entity.add.group name=Crates parent=Zone");
    editor.stop();

    assert!(editor.has("Crates"));
    assert!(editor.modified());
}

#[test]
fn with_revert_an_edit_made_during_play_is_gone_after_stop_and_no_redo_brings_it_back() {
    let mut editor = editor();
    editor.revert_on_stop();

    editor.play(1);
    editor.edit("entity.add.group name=Crates parent=Zone");
    assert!(editor.has("Crates"), "the edit authors the document");
    editor.stop();

    assert!(!editor.has("Crates"), "the edit is undone");
    assert!(!editor.modified(), "the document is as its file holds it");
    editor.op("history.redo");
    assert!(!editor.has("Crates"), "no redo brings it back");
    editor.op("history.undo");
    editor.op("history.redo");
    assert!(!editor.has("Crates"));
}

#[test]
fn an_unsaved_edit_made_before_play_stays_and_keeps_the_tab_modified() {
    let mut editor = editor();
    editor.revert_on_stop();
    editor.edit("entity.add.group name=Props parent=Zone");

    editor.play(1);
    editor.edit("entity.add.group name=Crates parent=Zone");
    editor.stop();

    assert!(editor.has("Props"));
    assert!(!editor.has("Crates"));
    assert!(
        editor.modified(),
        "the edit from before Play is still unsaved"
    );
}

#[test]
fn a_save_during_play_stays_on_disk_and_the_reverted_document_reads_modified() {
    let mut editor = editor();
    editor.revert_on_stop();

    editor.play(1);
    editor.edit("entity.add.group name=Crates parent=Zone");
    editor.op("scene.save");
    let saved = std::fs::read_to_string(editor.scene("zone.bsn")).expect("zone read");
    assert!(
        saved.contains("Crates"),
        "the save writes what is on screen"
    );
    editor.stop();

    assert!(!editor.has("Crates"), "the document is reverted");
    assert_eq!(
        std::fs::read_to_string(editor.scene("zone.bsn")).expect("zone read"),
        saved,
        "and the file left as saved"
    );
    assert!(editor.modified(), "so the document differs from its file");
}

#[test]
fn with_two_games_the_revert_waits_for_the_last_to_stop() {
    let mut editor = editor();
    editor.revert_on_stop();

    editor.play(1);
    editor.play(2);
    editor.edit("entity.add.group name=Crates parent=Zone");
    editor.stop_instance(1);
    assert!(editor.has("Crates"), "one game still plays");
    assert_ne!(editor.play_state(), PlayState::Stopped);

    editor.stop_instance(2);
    editor.settle();
    assert_eq!(editor.play_state(), PlayState::Stopped);
    assert!(!editor.has("Crates"));
}

#[test]
fn an_edit_in_another_tab_during_play_is_reverted_too() {
    let mut editor = editor();
    editor.revert_on_stop();
    editor.op("scene.open path=assets/yard.bsn");
    editor.switch_to("zone.bsn");

    editor.play(1);
    editor.switch_to("yard.bsn");
    editor.edit("entity.add.group name=Crates parent=Yard");
    editor.switch_to("zone.bsn");
    editor.stop();

    assert!(!editor.modified());
    editor.switch_to("yard.bsn");
    assert!(!editor.has("Crates"), "the other tab is reverted");
    assert!(!editor.modified(), "and reads as its file");
}

#[test]
fn a_scene_opened_during_play_returns_to_how_it_opened() {
    let mut editor = editor();
    editor.revert_on_stop();

    editor.play(1);
    editor.op("scene.open path=assets/yard.bsn");
    editor.edit("entity.add.group name=Crates parent=Yard");
    editor.stop();

    assert!(!editor.has("Crates"));
    assert!(!editor.modified());
}

#[test]
fn undoing_past_the_start_of_play_then_editing_keeps_the_work_from_before_play() {
    let mut editor = editor();
    editor.revert_on_stop();
    editor.edit("entity.add.group name=Props parent=Zone");
    editor.edit("entity.add.group name=Barrels parent=Zone");

    editor.play(1);
    editor.op("history.undo");
    editor.op("history.undo");
    assert!(!editor.has("Props"));
    editor.edit("entity.add.group name=Crates parent=Zone");
    editor.stop();

    assert!(editor.has("Props"), "the work from before Play is back");
    assert!(editor.has("Barrels"));
    assert!(!editor.has("Crates"));
    assert!(editor.modified());
    editor.op("history.undo");
    assert!(!editor.has("Barrels"), "and undoes as it did before Play");
}

#[test]
fn a_tab_modified_for_another_reason_stays_modified_after_the_revert() {
    let mut editor = editor();
    editor.revert_on_stop();
    {
        let mut scenes = editor
            .app
            .world_mut()
            .resource_mut::<jackdaw::scenes::Scenes>();
        let active = scenes.active;
        scenes.tabs[active].dirty = true;
    }

    editor.play(1);
    editor.edit("entity.add.group name=Crates parent=Zone");
    editor.stop();

    assert!(!editor.has("Crates"));
    assert!(editor.modified());
}

/// Play pressed while the game builds, so the launch waits on the build.
fn play_behind_a_build(editor: &mut Editor) {
    jackdaw::run_config::read_run_configs(editor.app.world_mut());
    jackdaw::pie::insert_build_in_progress(editor.app.world_mut());
    editor.op("pie.play");
    assert_eq!(
        jackdaw::pie::launches_waiting_on_build(editor.app.world()).len(),
        1
    );
}

#[test]
fn a_launch_whose_build_fails_reverts_nothing() {
    let mut editor = editor();
    editor.revert_on_stop();

    play_behind_a_build(&mut editor);
    editor.edit("entity.add.group name=Crates parent=Zone");
    jackdaw::pie::fail_builds_in_progress(editor.app.world_mut());
    editor.settle();

    assert_eq!(jackdaw::pie::play_status(editor.app.world()), "failed");
    assert!(editor.has("Crates"), "no game ran, so the edit stays");
}

#[test]
fn stopping_while_the_game_builds_reverts_nothing() {
    let mut editor = editor();
    editor.revert_on_stop();

    play_behind_a_build(&mut editor);
    editor.edit("entity.add.group name=Crates parent=Zone");
    editor.op("pie.stop");

    assert!(jackdaw::pie::launches_waiting_on_build(editor.app.world()).is_empty());
    assert!(editor.has("Crates"));
}

#[test]
fn the_revert_setting_is_kept_with_the_project() {
    let mut editor = editor();

    editor.revert_on_stop();

    let read: PlaySettings = jackdaw::project_settings::load_section(
        editor.dir.path(),
        jackdaw::project_settings::Section::Key("play"),
    );
    assert_eq!(read.edits, PlayEdits::Revert);
    editor.op("play.edits");
    assert_eq!(
        editor.app.world().resource::<PlaySettings>().edits,
        PlayEdits::Keep,
        "with no mode named the setting switches"
    );
}

#[cfg(unix)]
#[test]
fn popping_the_game_out_keeps_the_play_edits_and_relaunches_it_in_a_window() {
    use jackdaw::pie::{PieSession, PieWindowMode};
    use std::os::unix::fs::PermissionsExt;

    let mut editor = editor();
    editor.revert_on_stop();
    std::fs::write(
        editor.dir.path().join("jackdaw.toml"),
        "[[run]]\nname = \"Game\"\n",
    )
    .expect("a manifest");
    jackdaw::run_config::read_run_configs(editor.app.world_mut());
    let binary = editor.dir.path().join("game.sh");
    std::fs::write(&binary, "#!/bin/sh\nexec sleep 600\n").expect("a game script");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
        .expect("an executable game");

    editor.play(1);
    jackdaw::pie::insert_built_game(editor.app.world_mut(), binary);
    editor.edit("entity.add.group name=Crates parent=Zone");
    editor.op("pie.pop_out");

    {
        let session = editor.app.world().non_send::<PieSession>();
        assert!(session.is_running(&key(1)), "the game is back");
        assert_eq!(session.launch_mode(&key(1)), Some(PieWindowMode::Windowed));
    }
    assert_ne!(editor.play_state(), PlayState::Stopped);
    assert!(
        editor.has("Crates"),
        "the move is no stop, so the edit stays"
    );
    assert!(
        jackdaw::pie::launches_waiting_on_build(editor.app.world()).is_empty(),
        "relaunched from the game already built"
    );

    editor.stop();
    assert!(!editor.has("Crates"), "stopping Play still reverts");
}
