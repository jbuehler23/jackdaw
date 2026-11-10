//! The Game panel header: the instance picker, the view size, mute, and the
//! game taking input on its first frame.

use std::time::{Duration, Instant};

use crate::util::{self, OperatorResultExt as _};

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::input::mouse::MouseButtonInput;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowFocused};
use jackdaw::game_panel_header::{GameInstancePicker, named_instances};
use jackdaw::pie::InstanceKey;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_feathers::button::ButtonContentText;
use jackdaw_pie_protocol::event::from_bytes;
use jackdaw_pie_protocol::{
    ControlEvent, IpcChannelTransport, PieChannel, PieTransport as _, encode_frame,
};

const GUARD: Duration = Duration::from_secs(20);

struct Project {
    app: App,
    _dir: tempfile::TempDir,
}

impl Drop for Project {
    fn drop(&mut self) {
        jackdaw::pie::handle_stop(self.app.world_mut());
    }
}

/// An editor on a project with a Server and two Client run configurations.
fn editor_on_a_project() -> Project {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("jackdaw.toml"),
        "[[run]]\nname = \"Server\"\n\n[[run]]\nname = \"Client\"\ninstances = 2\n",
    )
    .expect("a manifest");
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: dir.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    for _ in 0..3 {
        app.update();
    }
    Project { app, _dir: dir }
}

fn key(config: &str, instance: u32) -> InstanceKey {
    InstanceKey {
        config: config.to_string(),
        instance,
    }
}

fn call(app: &mut App, id: &str, params: &[(&'static str, &str)]) -> OperatorResult {
    let mut builder = app
        .world_mut()
        .operator(id.to_string())
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        });
    for (name, value) in params {
        builder = builder.param(*name, value.to_string());
    }
    let result = builder.call().expect("dispatch");
    app.update();
    app.update();
    result
}

/// Connect a game as `key`, returning the game's end of the link.
fn connect(app: &mut App, key: InstanceKey) -> IpcChannelTransport {
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
    jackdaw::pie::connect_test_game(app.world_mut(), key, child, editor_end).expect("a link");
    app.update();
    game
}

fn received(game: &mut IpcChannelTransport) -> Vec<ControlEvent> {
    game.drain_received()
        .into_iter()
        .filter(|(channel, _)| *channel == PieChannel::Reliable)
        .filter_map(|(_, bytes)| from_bytes::<ControlEvent>(&bytes).ok())
        .collect()
}

/// Run frames until the game receives an event `pick` accepts.
fn next<T>(
    app: &mut App,
    game: &mut IpcChannelTransport,
    mut pick: impl FnMut(ControlEvent) -> Option<T>,
) -> T {
    let until = Instant::now() + GUARD;
    loop {
        assert!(
            Instant::now() < until,
            "nothing of the kind reached the game"
        );
        app.update();
        for event in received(game) {
            if let Some(found) = pick(event) {
                return found;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn picker_caption(app: &mut App) -> Option<String> {
    let picker = app
        .world_mut()
        .query_filtered::<(Entity, &Node), With<GameInstancePicker>>()
        .single(app.world())
        .ok()
        .filter(|(_, node)| node.display != Display::None)
        .map(|(entity, _)| entity)?;
    let mut stack = vec![picker];
    while let Some(entity) = stack.pop() {
        if app.world().get::<ButtonContentText>(entity).is_some() {
            return app.world().get::<Text>(entity).map(|text| text.0.clone());
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    None
}

#[test]
fn the_instance_picker_shows_with_two_instances_and_focuses_the_one_chosen() {
    let mut project = editor_on_a_project();
    let app = &mut project.app;
    let _server = connect(app, key("Server", 1));
    app.update();
    assert_eq!(picker_caption(app), None, "one instance needs no picker");

    let _client = connect(app, key("Client", 2));
    app.update();
    assert_eq!(picker_caption(app).as_deref(), Some("Client 2"));

    call(
        app,
        "pie.focus_instance",
        &[("config", "Server"), ("instance", "1")],
    )
    .assert_finished();
    assert_eq!(
        app.world()
            .resource::<jackdaw::pie::PieInstances>()
            .focused
            .as_ref(),
        Some(&key("Server", 1))
    );
    assert_eq!(picker_caption(app).as_deref(), Some("Server"));

    let runs = app
        .world()
        .resource::<jackdaw::run_config::RunConfigs>()
        .manifest
        .runs
        .clone();
    let names: Vec<String> = named_instances(&runs, &[key("Client", 2), key("Server", 1)])
        .into_iter()
        .map(|(_, name)| name)
        .collect();
    assert_eq!(names, ["Server", "Client 2"], "the picker's list");
}

#[test]
fn the_view_size_persists_and_sets_the_size_asked_of_the_game() {
    let mut project = editor_on_a_project();
    let root = project._dir.path().to_path_buf();
    let app = &mut project.app;
    let mut game = connect(app, key("Server", 1));
    next(app, &mut game, |event| match event {
        ControlEvent::StartFrameStream { .. } => Some(()),
        _ => None,
    });

    call(app, "pie.view_aspect", &[("value", "16:9")]).assert_finished();
    let (width, height) = next(app, &mut game, |event| match event {
        ControlEvent::StartFrameStream { width, height } if width * 9 == height * 16 => {
            Some((width, height))
        }
        _ => None,
    });
    assert_eq!(width % 64, 0, "{width}x{height}");

    call(app, "pie.view_aspect", &[("value", "1280x720")]).assert_finished();
    next(app, &mut game, |event| match event {
        ControlEvent::StartFrameStream {
            width: 1280,
            height: 720,
        } => Some(()),
        _ => None,
    });

    let settings: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(jackdaw::project_settings::settings_path(&root))
            .expect("the settings file"),
    )
    .expect("settings json");
    assert_eq!(settings["play"]["view"], "1280x720");
}

#[test]
fn choosing_an_aspect_near_the_panel_size_still_asks_for_it() {
    let mut project = editor_on_a_project();
    let app = &mut project.app;
    let surface = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::game_panel::GamePanelSurface>>()
        .single(app.world())
        .expect("the Game panel surface");
    {
        let mut node = app
            .world_mut()
            .get_mut::<Node>(surface)
            .expect("the surface node");
        node.flex_grow = 0.0;
        node.flex_shrink = 0.0;
        node.width = px(712.0);
        node.height = px(400.0);
    }
    let mut game = connect(app, key("Server", 1));
    let free = next(app, &mut game, |event| match event {
        ControlEvent::StartFrameStream { width, height } => Some(UVec2::new(width, height)),
        _ => None,
    });
    let wide = jackdaw::play_settings::GameView::Wide.stream_size(free);
    assert!(
        free.x.abs_diff(wide.x) <= 32 && free.y.abs_diff(wide.y) <= 32,
        "the panel ({free}) is already close to 16:9 ({wide})"
    );

    call(app, "pie.view_aspect", &[("value", "16:9")]).assert_finished();
    next(app, &mut game, |event| {
        (event
            == ControlEvent::StartFrameStream {
                width: wide.x,
                height: wide.y,
            })
        .then_some(())
    });
}

#[test]
fn mute_is_sent_to_a_game_as_it_connects_and_when_it_changes() {
    let mut project = editor_on_a_project();
    let app = &mut project.app;
    call(app, "pie.mute", &[]).assert_finished();
    assert!(
        app.world()
            .resource::<jackdaw::play_settings::PlaySettings>()
            .muted
    );

    let mut game = connect(app, key("Server", 1));
    next(app, &mut game, |event| {
        (event == ControlEvent::SetMuted(true)).then_some(())
    });

    call(app, "pie.mute", &[]).assert_finished();
    next(app, &mut game, |event| {
        (event == ControlEvent::SetMuted(false)).then_some(())
    });
}

/// What happens in the editor while the game builds.
#[derive(Clone, Copy)]
enum Meanwhile {
    Nothing,
    Click,
    KeyPress,
    FocusLost,
}

/// Press Play with the build still running, then connect the game and stream
/// frames for a few seconds, returning whether the game took input. What
/// `meanwhile` names lands in the editor after Play.
fn play_and_stream(meanwhile: Meanwhile) -> bool {
    let mut project = editor_on_a_project();
    let app = &mut project.app;
    jackdaw::pie::insert_build_in_progress(app.world_mut());
    call(app, "pie.play", &[]).assert_finished();
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("the editor window");
    match meanwhile {
        Meanwhile::Nothing => {}
        Meanwhile::Click => {
            app.world_mut().write_message(MouseButtonInput {
                button: MouseButton::Left,
                state: ButtonState::Pressed,
                window,
            });
        }
        Meanwhile::KeyPress => {
            app.world_mut().write_message(KeyboardInput {
                key_code: KeyCode::KeyW,
                logical_key: Key::Character("w".into()),
                state: ButtonState::Pressed,
                text: None,
                repeat: false,
                window,
            });
        }
        Meanwhile::FocusLost => {
            app.world_mut().write_message(WindowFocused {
                window,
                focused: false,
            });
        }
    }
    app.update();

    let mut game = connect(app, key("Server", 1));
    let until = Instant::now() + Duration::from_secs(3);
    let mut seq = 0;
    while Instant::now() < until {
        seq += 1;
        let pixels = vec![128; 64 * 64 * 4];
        game.send(PieChannel::Frames, &encode_frame(64, 64, seq, &pixels));
        app.update();
        if app
            .world()
            .resource::<jackdaw::live_input::LiveInputCapture>()
            .active
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

#[test]
fn the_game_takes_input_on_its_first_frame_when_the_editor_was_left_alone() {
    assert!(play_and_stream(Meanwhile::Nothing));
}

#[test]
fn a_click_in_the_editor_while_the_game_starts_keeps_input_with_the_editor() {
    assert!(!play_and_stream(Meanwhile::Click));
}

#[test]
fn a_key_press_in_the_editor_while_the_game_starts_keeps_input_with_the_editor() {
    assert!(!play_and_stream(Meanwhile::KeyPress));
}

#[test]
fn leaving_the_editor_window_while_the_game_starts_keeps_input_with_the_editor() {
    assert!(!play_and_stream(Meanwhile::FocusLost));
}

#[test]
fn a_game_that_connects_while_play_is_paused_is_paused_too() {
    let mut project = editor_on_a_project();
    let app = &mut project.app;
    let _server = connect(app, key("Server", 1));
    app.update();
    app.update();
    call(app, "pie.pause", &[]).assert_finished();
    assert_eq!(
        *app.world()
            .resource::<State<jackdaw_api::pie::PlayState>>()
            .get(),
        jackdaw_api::pie::PlayState::Paused
    );

    let mut client = connect(app, key("Client", 1));
    next(app, &mut client, |event| {
        (event == ControlEvent::Pause).then_some(())
    });
}

#[cfg(unix)]
fn display_of<T: Component>(app: &mut App) -> Option<Display> {
    app.world_mut()
        .query_filtered::<&Node, With<T>>()
        .iter(app.world())
        .next()
        .map(|node| node.display)
}

#[cfg(unix)]
#[test]
fn a_popped_out_game_shows_where_it_went_instead_of_its_last_frame() {
    use jackdaw::game_panel::{GameNoSignalChip, GamePanelIdleLabel, GamePanelImage};
    use std::os::unix::fs::PermissionsExt;

    let mut project = editor_on_a_project();
    let binary = project._dir.path().join("game.sh");
    std::fs::write(&binary, "#!/bin/sh\nexec sleep 600\n").expect("a game script");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
        .expect("an executable game");
    let app = &mut project.app;
    let mut game = connect(app, key("Server", 1));
    jackdaw::pie::insert_built_game(app.world_mut(), binary);
    let pixels = vec![128; 64 * 64 * 4];
    game.send(PieChannel::Frames, &encode_frame(64, 64, 1, &pixels));
    let until = Instant::now() + GUARD;
    while display_of::<GamePanelImage>(app) != Some(Display::Flex) {
        assert!(Instant::now() < until, "the frame never showed");
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }

    call(app, "pie.pop_out", &[]).assert_finished();
    app.update();
    assert_eq!(display_of::<GamePanelImage>(app), Some(Display::None));
    assert_eq!(display_of::<GameNoSignalChip>(app), Some(Display::None));
    let label = app
        .world_mut()
        .query_filtered::<&Text, With<GamePanelIdleLabel>>()
        .iter(app.world())
        .next()
        .map(|text| text.0.clone());
    assert_eq!(label.as_deref(), Some("Running in its own window"));
}

fn launch_mode(app: &App, key: &InstanceKey) -> Option<jackdaw::pie::PieWindowMode> {
    app.world()
        .non_send::<jackdaw::pie::PieSession>()
        .launch_mode(key)
}

fn window_mode(app: &App) -> jackdaw::pie::PieWindowMode {
    *app.world().resource::<jackdaw::pie::PieWindowMode>()
}

#[cfg(unix)]
fn focused(app: &App) -> Option<InstanceKey> {
    app.world()
        .resource::<jackdaw::pie::PieInstances>()
        .focused
        .clone()
}

/// A built game that starts and never connects, held as the project's build.
#[cfg(unix)]
fn insert_idle_built_game(project: &mut Project) {
    use std::os::unix::fs::PermissionsExt;
    let binary = project._dir.path().join("game.sh");
    std::fs::write(&binary, "#!/bin/sh\nexec sleep 600\n").expect("a game script");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
        .expect("an executable game");
    jackdaw::pie::insert_built_game(project.app.world_mut(), binary);
}

/// Server launched in the Game panel and Client in its own window, both live.
#[cfg(unix)]
fn server_in_panel_client_in_window(app: &mut App) -> (IpcChannelTransport, IpcChannelTransport) {
    let server = connect(app, key("Server", 1));
    call(app, "pie.window_mode_toggle", &[]).assert_finished();
    let client = connect(app, key("Client", 1));
    app.update();
    app.update();
    (server, client)
}

#[test]
fn popping_out_while_the_game_builds_is_refused_and_play_carries_on() {
    let mut project = editor_on_a_project();
    let app = &mut project.app;
    let _game = connect(app, key("Server", 1));
    app.update();
    app.update();
    jackdaw::pie::insert_build_in_progress(app.world_mut());

    call(app, "pie.pop_out", &[]).assert_finished();
    app.update();
    assert_eq!(window_mode(app), jackdaw::pie::PieWindowMode::Embedded);
    assert_eq!(
        launch_mode(app, &key("Server", 1)),
        Some(jackdaw::pie::PieWindowMode::Embedded),
        "the game keeps running where it was"
    );
    assert_eq!(jackdaw::pie::play_status(app.world()), "running");
}

#[test]
fn a_relaunch_that_fails_to_start_keeps_the_old_game_running() {
    let mut project = editor_on_a_project();
    let missing = project._dir.path().join("no-such-game");
    let app = &mut project.app;
    let _game = connect(app, key("Server", 1));
    app.update();
    app.update();
    jackdaw::pie::insert_built_game(app.world_mut(), missing);

    call(app, "pie.pop_out", &[]).assert_finished();
    app.update();
    assert_eq!(
        launch_mode(app, &key("Server", 1)),
        Some(jackdaw::pie::PieWindowMode::Embedded)
    );
    assert_eq!(jackdaw::pie::play_status(app.world()), "running");
}

#[cfg(unix)]
#[test]
fn the_game_brought_into_the_panel_keeps_the_focus_and_takes_input() {
    let mut project = editor_on_a_project();
    insert_idle_built_game(&mut project);
    let app = &mut project.app;
    let _games = server_in_panel_client_in_window(app);
    call(
        app,
        "pie.focus_instance",
        &[("config", "Client"), ("instance", "1")],
    )
    .assert_finished();

    call(app, "pie.pop_out", &[]).assert_finished();
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(
        launch_mode(app, &key("Client", 1)),
        Some(jackdaw::pie::PieWindowMode::Embedded)
    );
    assert_eq!(
        focused(app),
        Some(key("Client", 1)),
        "its relaunch keeps the focus"
    );
    assert!(
        app.world()
            .resource::<jackdaw::live_input::FirstFrameCapture>()
            .is_armed()
    );
}

#[cfg(unix)]
#[test]
fn bringing_another_game_into_the_panel_leaves_the_focused_one_without_capture() {
    let mut project = editor_on_a_project();
    insert_idle_built_game(&mut project);
    let app = &mut project.app;
    let _games = server_in_panel_client_in_window(app);
    call(
        app,
        "pie.focus_instance",
        &[("config", "Server"), ("instance", "1")],
    )
    .assert_finished();

    call(app, "pie.pop_out", &[]).assert_finished();
    assert_eq!(
        launch_mode(app, &key("Client", 1)),
        Some(jackdaw::pie::PieWindowMode::Embedded)
    );
    assert_eq!(focused(app), Some(key("Server", 1)));
    assert!(
        !app.world()
            .resource::<jackdaw::live_input::FirstFrameCapture>()
            .is_armed()
    );
}
