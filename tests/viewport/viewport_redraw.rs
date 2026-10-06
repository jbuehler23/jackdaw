//! Realtime off: the window waits for events, frames are asked for only
//! while something is still moving, and the toolbar says the viewport is
//! not redrawing.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use bevy::window::RequestRedraw;
use bevy::winit::{UpdateMode, WinitSettings};
use jackdaw::viewport::CameraFlyActive;
use jackdaw::viewport_settings::ViewportSettingsFile;
use jackdaw::viewport_settings_menu::RealtimeOffIndicator;

/// How many redraws were asked for since the counter was last cleared. The
/// editor asks in `Last`, so a request shows up here the frame after.
#[derive(Resource, Default)]
struct Redraws(usize);

fn count_redraws(mut asked: MessageReader<RequestRedraw>, mut redraws: ResMut<Redraws>) {
    redraws.0 += asked.read().count();
}

fn app_with_viewport(dir: &std::path::Path) -> App {
    let mut app = util::editor_test_app();
    app.init_resource::<WinitSettings>()
        .init_resource::<Redraws>()
        .add_systems(PostUpdate, count_redraws);
    app.world_mut().resource_mut::<ViewportSettingsFile>().path = Some(dir.join("viewport.json"));
    let root = app.world_mut().spawn(Node::default()).id();
    jackdaw::viewport::build_viewport_panel(app.world_mut(), root);
    for _ in 0..4 {
        app.update();
    }
    app
}

fn set_realtime(app: &mut App, on: bool) {
    jackdaw::boot_ops::run_op_clause(
        app.world_mut(),
        &format!("viewport.realtime.toggle on={on}"),
    )
    .expect("the clause dispatches")
    .assert_finished();
    for _ in 0..2 {
        app.update();
    }
}

/// Update until a stretch of frames longer than the input settle asks for no
/// redraw, and say whether one came.
fn falls_quiet(app: &mut App) -> bool {
    let started = std::time::Instant::now();
    let mut quiet_since = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(20) {
        app.world_mut().resource_mut::<Redraws>().0 = 0;
        app.update();
        if app.world().resource::<Redraws>().0 > 0 {
            quiet_since = std::time::Instant::now();
        } else if quiet_since.elapsed() > jackdaw::viewport_redraw::INPUT_SETTLE * 2 {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    false
}

#[test]
fn realtime_off_lets_the_window_wait_for_events() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_viewport(dir.path());
    assert_eq!(
        app.world().resource::<WinitSettings>().focused_mode,
        UpdateMode::Continuous
    );

    set_realtime(&mut app, false);
    let winit = app.world().resource::<WinitSettings>();
    assert_eq!(
        winit.focused_mode,
        UpdateMode::reactive(jackdaw::viewport_redraw::IDLE_REDRAW)
    );
    assert_eq!(winit.unfocused_mode, winit.focused_mode);

    set_realtime(&mut app, true);
    assert_eq!(
        app.world().resource::<WinitSettings>().focused_mode,
        UpdateMode::Continuous
    );
}

#[test]
fn an_idle_editor_stops_asking_for_frames_and_held_input_asks_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_viewport(dir.path());
    set_realtime(&mut app, false);
    assert!(
        falls_quiet(&mut app),
        "an idle editor stops asking for frames"
    );

    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .press(KeyCode::KeyW);
    app.world_mut().resource_mut::<Redraws>().0 = 0;
    app.update();
    app.update();
    assert!(
        app.world().resource::<Redraws>().0 > 0,
        "a held key keeps the frames coming"
    );
    app.world_mut()
        .resource_mut::<ButtonInput<KeyCode>>()
        .release(KeyCode::KeyW);
    assert!(falls_quiet(&mut app), "and they stop once it is let go");
}

#[test]
fn a_click_pressed_and_released_within_one_frame_asks_for_frames() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_viewport(dir.path());
    set_realtime(&mut app, false);
    assert!(falls_quiet(&mut app));

    let window = app
        .world_mut()
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .single(app.world())
        .expect("a primary window");
    for state in [
        bevy::input::ButtonState::Pressed,
        bevy::input::ButtonState::Released,
    ] {
        app.world_mut()
            .write_message(bevy::input::mouse::MouseButtonInput {
                button: MouseButton::Left,
                state,
                window,
            });
    }
    app.world_mut().resource_mut::<Redraws>().0 = 0;
    app.update();
    app.update();
    assert!(
        app.world().resource::<Redraws>().0 > 0,
        "a click over by the time the frame runs still counts as input"
    );
}

#[test]
fn realtime_on_never_asks_for_frames_it_already_draws() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_viewport(dir.path());
    app.world_mut().resource_mut::<CameraFlyActive>().0 = true;
    app.world_mut().resource_mut::<Redraws>().0 = 0;
    app.update();
    app.update();
    assert_eq!(app.world().resource::<Redraws>().0, 0);
}

#[test]
fn the_toolbar_says_realtime_is_off_while_it_is() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_viewport(dir.path());
    let shown = |app: &mut App| {
        app.world_mut()
            .query_filtered::<&Node, With<RealtimeOffIndicator>>()
            .iter(app.world())
            .map(|node| node.display != Display::None)
            .collect::<Vec<_>>()
    };
    assert_eq!(shown(&mut app), vec![false], "one note per toolbar, hidden");
    set_realtime(&mut app, false);
    assert_eq!(shown(&mut app), vec![true]);
    set_realtime(&mut app, true);
    assert_eq!(shown(&mut app), vec![false]);
}

#[test]
fn a_long_task_keeps_the_frames_coming_while_it_runs() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_viewport(dir.path());
    set_realtime(&mut app, false);
    assert!(falls_quiet(&mut app));

    jackdaw::progress::begin_progress(app.world_mut(), "test.task", "Working", false);
    app.world_mut().resource_mut::<Redraws>().0 = 0;
    app.update();
    app.update();
    assert!(app.world().resource::<Redraws>().0 > 0);

    jackdaw::progress::finish_progress(app.world_mut(), "test.task");
    assert!(falls_quiet(&mut app));
}

#[test]
fn meshes_arriving_keep_the_frames_coming() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_viewport(dir.path());
    set_realtime(&mut app, false);
    assert!(falls_quiet(&mut app));

    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Cuboid::default());
    app.world_mut().spawn(Mesh3d(mesh));
    app.world_mut().resource_mut::<Redraws>().0 = 0;
    app.update();
    app.update();
    assert!(app.world().resource::<Redraws>().0 > 0);
}

#[test]
fn a_scripted_gesture_keeps_the_frames_coming_until_it_has_played() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_viewport(dir.path());
    set_realtime(&mut app, false);
    assert!(falls_quiet(&mut app));

    jackdaw::boot_ops::run_op_clause(app.world_mut(), "input.pointer x=40 y=40 action=click")
        .expect("the clause dispatches")
        .assert_finished();
    app.world_mut().resource_mut::<Redraws>().0 = 0;
    app.update();
    app.update();
    assert!(app.world().resource::<Redraws>().0 > 0);
    assert!(falls_quiet(&mut app), "and stop once it has played");
}
