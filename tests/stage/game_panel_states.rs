//! What the Game panel shows when there is no frame: Play while stopped, and
//! why the last launch ended when it went wrong.

use std::time::{Duration, Instant};

use crate::util::{self, OperatorResultExt as _};

use bevy::prelude::*;
use jackdaw::game_panel::{GAME_WINDOW_ID, GameIdlePart, GamePanelIdleLabel, GamePanelStderrText};
use jackdaw::pie::InstanceKey;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_panels::tree::{DockTree, NodeId};

fn call(app: &mut App, id: &str) -> OperatorResult {
    let result = app
        .world_mut()
        .operator(id.to_string())
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .call()
        .expect("dispatch");
    app.update();
    app.update();
    result
}

fn key() -> InstanceKey {
    InstanceKey {
        config: "Client".to_string(),
        instance: 1,
    }
}

/// An editor on a project with one run configuration, the Build panel in
/// front of the dock it shares with the Game panel.
fn editor_on_a_project() -> (App, NodeId, tempfile::TempDir) {
    let project = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        project.path().join("jackdaw.toml"),
        "[[run]]\nname = \"Client\"\n",
    )
    .expect("a manifest");
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: project.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    for _ in 0..3 {
        app.update();
    }
    let leaf = {
        let mut tree = app.world_mut().resource_mut::<DockTree>();
        let leaf = tree
            .find_leaf_with_window("jackdaw.build")
            .expect("a Build panel");
        assert_eq!(
            tree.find_leaf_with_window(GAME_WINDOW_ID),
            Some(leaf),
            "the Game panel shares the Build panel's dock"
        );
        let build_tab = tree
            .get(leaf)
            .and_then(|node| node.as_leaf())
            .and_then(|dock| dock.tabs().find(|(window, _)| *window == "jackdaw.build"))
            .map(|(_, tab)| tab)
            .expect("a Build tab");
        tree.set_active(leaf, build_tab);
        leaf
    };
    app.update();
    assert_eq!(front_window(&app, leaf).as_deref(), Some("jackdaw.build"));
    (app, leaf, project)
}

fn front_window(app: &App, leaf: NodeId) -> Option<String> {
    let tree = app.world().resource::<DockTree>();
    let leaf = tree.get(leaf)?.as_leaf()?;
    let active = leaf.active?;
    leaf.tabs()
        .find_map(|(window, tab)| (tab == active).then(|| window.to_string()))
}

fn label(app: &mut App) -> String {
    app.world_mut()
        .query_filtered::<&Text, With<GamePanelIdleLabel>>()
        .single(app.world())
        .expect("one idle label")
        .0
        .clone()
}

fn stderr(app: &mut App) -> String {
    app.world_mut()
        .query_filtered::<&Text, With<GamePanelStderrText>>()
        .single(app.world())
        .expect("one stderr block")
        .0
        .clone()
}

/// The parts of the idle column on show.
fn shown(app: &mut App) -> Vec<GameIdlePart> {
    let mut parts: Vec<GameIdlePart> = app
        .world_mut()
        .query::<(&GameIdlePart, &Node)>()
        .iter(app.world())
        .filter(|(_, node)| node.display != Display::None)
        .map(|(part, _)| *part)
        .collect();
    parts.sort_by_key(|part| *part as u8);
    parts
}

/// The operator the Restart button runs.
fn restart_operator(app: &mut App) -> String {
    app.world_mut()
        .query::<(&GameIdlePart, &ButtonOperatorCall)>()
        .iter(app.world())
        .find(|(part, _)| **part == GameIdlePart::Restart)
        .map(|(_, call)| call.id.to_string())
        .expect("a Restart button")
}

/// Update until `done` holds, failing after a few seconds.
fn update_until(app: &mut App, what: &str, mut done: impl FnMut(&mut App) -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !done(app) {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        app.update();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn exit_recorded(app: &mut App) -> bool {
    app.world()
        .resource::<jackdaw::pie::LastGameExit>()
        .0
        .is_some()
}

#[test]
fn stopped_offers_play_with_the_input_hint() {
    let (mut app, _leaf, _project) = editor_on_a_project();
    assert_eq!(shown(&mut app), [GameIdlePart::Play, GameIdlePart::Hint]);
}

#[test]
fn a_game_that_crashes_shows_its_exit_code_and_restart_launches_it_again() {
    let (mut app, leaf, _project) = editor_on_a_project();
    let (server, name) = jackdaw_pie_protocol::serve().expect("a rendezvous");
    let accept = std::thread::spawn(move || server.accept());
    let _game = jackdaw_pie_protocol::connect(&name).expect("the game connects");
    let editor_end = accept
        .join()
        .expect("accept ran")
        .expect("the editor accepts");
    let child = std::process::Command::new("sh")
        .args(["-c", "echo 'the game panicked' >&2; sleep 0.3; exit 3"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("a child process");
    jackdaw::pie::connect_test_game(app.world_mut(), key(), child, editor_end).expect("a link");
    for _ in 0..2 {
        app.update();
    }
    assert_eq!(front_window(&app, leaf).as_deref(), Some(GAME_WINDOW_ID));

    update_until(&mut app, "the exit to be recorded", exit_recorded);
    update_until(&mut app, "the stderr lines", |app| {
        stderr(app).contains("the game panicked")
    });
    assert_eq!(label(&mut app), "Game exited (code 3)");
    assert_eq!(
        shown(&mut app),
        [
            GameIdlePart::Label,
            GameIdlePart::Stderr,
            GameIdlePart::Restart
        ]
    );
    assert_eq!(
        front_window(&app, leaf).as_deref(),
        Some(GAME_WINDOW_ID),
        "the panel stays in front so the exit is seen"
    );

    jackdaw::pie::insert_build_in_progress(app.world_mut());
    let restart = restart_operator(&mut app);
    call(&mut app, &restart).assert_finished();
    assert_eq!(
        jackdaw::pie::launches_waiting_on_build(app.world()).len(),
        1,
        "Restart launches the game again"
    );
    assert!(!exit_recorded(&mut app));
    assert_eq!(label(&mut app), "Building the game...");
    assert_eq!(shown(&mut app), [GameIdlePart::Label]);
}

#[test]
fn a_game_that_never_connects_is_told_to_add_jackdaw_plugin() {
    let (mut app, leaf, _project) = editor_on_a_project();
    jackdaw::pie::set_connect_timeout(app.world_mut(), Duration::from_millis(200));
    let child = std::process::Command::new("sleep")
        .arg("600")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("a child process");
    jackdaw::pie::connecting_test_game(app.world_mut(), key(), child);
    app.update();
    assert_eq!(label(&mut app), "Starting the game...");
    assert_eq!(front_window(&app, leaf).as_deref(), Some(GAME_WINDOW_ID));

    update_until(&mut app, "the connect timeout", exit_recorded);
    app.update();
    let message = label(&mut app);
    assert!(message.contains("JackdawPlugin"), "{message}");
    assert_eq!(
        shown(&mut app),
        [GameIdlePart::Label, GameIdlePart::Restart]
    );
    assert_eq!(front_window(&app, leaf).as_deref(), Some(GAME_WINDOW_ID));
}

#[test]
fn the_game_panel_comes_forward_during_the_build_and_stays_when_it_fails() {
    let (mut app, leaf, _project) = editor_on_a_project();
    jackdaw::pie::insert_build_in_progress(app.world_mut());
    call(&mut app, "pie.play").assert_finished();
    assert_eq!(front_window(&app, leaf).as_deref(), Some(GAME_WINDOW_ID));
    assert_eq!(label(&mut app), "Building the game...");

    jackdaw::pie::fail_builds_in_progress(app.world_mut());
    for _ in 0..2 {
        app.update();
    }
    assert_eq!(front_window(&app, leaf).as_deref(), Some(GAME_WINDOW_ID));
    assert!(label(&mut app).contains("build failed"));
    assert_eq!(shown(&mut app), [GameIdlePart::Label]);
}
