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

fn settings() -> CallOperatorSettings {
    CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: false,
    }
}

fn call(app: &mut App, id: &str) -> OperatorResult {
    let result = app
        .world_mut()
        .operator(id.to_string())
        .settings(settings())
        .call()
        .expect("dispatch");
    app.update();
    app.update();
    result
}

/// Run what the button for `part` runs, with its parameters, as a click does.
fn press(app: &mut App, part: GameIdlePart) -> OperatorResult {
    let pressed = app
        .world_mut()
        .query::<(&GameIdlePart, &ButtonOperatorCall)>()
        .iter(app.world())
        .find(|(found, _)| **found == part)
        .map(|(_, call)| call.clone())
        .expect("a button for the part");
    let mut builder = app
        .world_mut()
        .operator(pressed.id.to_string())
        .settings(settings());
    for (key, value) in pressed.params {
        builder = builder.param(key, value);
    }
    let result = builder.call().expect("dispatch");
    app.update();
    app.update();
    result
}

fn key(config: &str) -> InstanceKey {
    InstanceKey {
        config: config.to_string(),
        instance: 1,
    }
}

/// An editor on a project with a Client and a Server run configuration, the
/// Build panel in front of the dock it shares with the Game panel.
fn editor_on_a_project() -> (App, NodeId, tempfile::TempDir) {
    let project = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        project.path().join("jackdaw.toml"),
        "[[run]]\nname = \"Client\"\n\n[[run]]\nname = \"Server\"\n",
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

/// Update until `done` holds, failing after a few seconds.
fn update_until(app: &mut App, what: &str, mut done: impl FnMut(&mut App) -> bool) {
    let until = Instant::now() + Duration::from_secs(10);
    while !done(app) {
        assert!(Instant::now() < until, "timed out waiting for {what}");
        app.update();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn recorded_exit(app: &App) -> Option<InstanceKey> {
    app.world()
        .resource::<jackdaw::pie::LastGameExit>()
        .0
        .as_ref()
        .map(|exit| exit.key.clone())
}

/// A game process that writes `stderr` and exits with `code` once `go` exists,
/// connected to the editor as `key`. The returned link end must outlive it.
#[cfg(unix)]
fn connect_game(
    app: &mut App,
    key: InstanceKey,
    go: &std::path::Path,
    stderr: &str,
    code: i32,
) -> jackdaw_pie_protocol::IpcChannelTransport {
    let (server, name) = jackdaw_pie_protocol::serve().expect("a rendezvous");
    let accept = std::thread::spawn(move || server.accept());
    let game = jackdaw_pie_protocol::connect(&name).expect("the game connects");
    let editor_end = accept
        .join()
        .expect("accept ran")
        .expect("the editor accepts");
    let script = format!(
        "while [ ! -e \"$1\" ]; do sleep 0.01; done; printf '%s\\n' \"$2\" >&2; exit {code}"
    );
    let child = std::process::Command::new("sh")
        .arg("-c")
        .arg(script)
        .arg("sh")
        .arg(go)
        .arg(stderr)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("a child process");
    jackdaw::pie::connect_test_game(app.world_mut(), key, child, editor_end).expect("a link");
    game
}

#[cfg(unix)]
fn signal(go: &std::path::Path) {
    std::fs::write(go, "").expect("the signal file");
}

#[cfg(unix)]
fn signal_file(project: &tempfile::TempDir, name: &str) -> std::path::PathBuf {
    project.path().join(name)
}

#[test]
fn stopped_offers_play_with_the_input_hint() {
    let (mut app, _leaf, _project) = editor_on_a_project();
    assert_eq!(shown(&mut app), [GameIdlePart::Play, GameIdlePart::Hint]);
}

#[cfg(unix)]
#[test]
fn a_game_that_crashes_shows_its_exit_and_restart_launches_that_game_again() {
    let (mut app, leaf, project) = editor_on_a_project();
    let go = signal_file(&project, "crash");
    let _link = connect_game(&mut app, key("Server"), &go, "the server panicked", 3);
    for _ in 0..2 {
        app.update();
    }
    assert_eq!(front_window(&app, leaf).as_deref(), Some(GAME_WINDOW_ID));

    signal(&go);
    update_until(&mut app, "the exit to be recorded", |app| {
        recorded_exit(app).is_some()
    });
    app.update();
    assert_eq!(label(&mut app), "Game exited (code 3)");
    assert_eq!(stderr(&mut app), "the server panicked");
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
    press(&mut app, GameIdlePart::Restart).assert_finished();
    let waiting: Vec<String> = jackdaw::pie::launches_waiting_on_build(app.world())
        .into_iter()
        .map(|(run, _)| run.label().to_string())
        .collect();
    assert_eq!(
        waiting,
        ["Server"],
        "Restart launches the game that crashed"
    );
    assert_eq!(recorded_exit(&app), None);
    assert_eq!(label(&mut app), "Building the game...");
    assert_eq!(shown(&mut app), [GameIdlePart::Label]);
}

#[cfg(unix)]
#[test]
fn another_game_exiting_cleanly_keeps_the_crash_on_show() {
    let (mut app, leaf, project) = editor_on_a_project();
    let crash = signal_file(&project, "crash");
    let quit = signal_file(&project, "quit");
    let _client = connect_game(&mut app, key("Client"), &crash, "the client panicked", 3);
    let _server = connect_game(&mut app, key("Server"), &quit, "bye", 0);
    for _ in 0..2 {
        app.update();
    }

    signal(&crash);
    update_until(&mut app, "the crash to be recorded", |app| {
        recorded_exit(app).is_some()
    });
    signal(&quit);
    update_until(&mut app, "the other game to be reaped", |app| {
        jackdaw::pie::play_status(app.world()) == "stopped"
    });
    app.update();

    assert_eq!(recorded_exit(&app), Some(key("Client")));
    assert_eq!(label(&mut app), "Game exited (code 3)");
    assert_eq!(front_window(&app, leaf).as_deref(), Some(GAME_WINDOW_ID));
}

#[cfg(unix)]
#[test]
fn a_game_that_never_connects_is_told_to_add_jackdaw_plugin() {
    let (mut app, leaf, _project) = editor_on_a_project();
    jackdaw::pie::set_connect_timeout(app.world_mut(), Duration::from_secs(600));
    let child = std::process::Command::new("sleep")
        .arg("600")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("a child process");
    jackdaw::pie::connecting_test_game(app.world_mut(), key("Client"), child);
    app.update();
    assert_eq!(label(&mut app), "Starting the game...");
    assert_eq!(front_window(&app, leaf).as_deref(), Some(GAME_WINDOW_ID));

    jackdaw::pie::set_connect_timeout(app.world_mut(), Duration::ZERO);
    update_until(&mut app, "the connect timeout", |app| {
        recorded_exit(app).is_some()
    });
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
fn the_game_panel_comes_forward_during_the_build_and_offers_its_errors_when_it_fails() {
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
    assert_eq!(label(&mut app), "The build failed.");
    assert_eq!(
        shown(&mut app),
        [GameIdlePart::Label, GameIdlePart::BuildErrors]
    );

    press(&mut app, GameIdlePart::BuildErrors).assert_finished();
    assert_eq!(front_window(&app, leaf).as_deref(), Some("jackdaw.build"));
}
