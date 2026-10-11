//! The Play, Pause and Stop controls follow the play state, which follows
//! the games actually running. The launch itself is `bsn_game_run`'s.

use crate::util::{self, OperatorResultExt as _};

use bevy::prelude::*;
use jackdaw_api::pie::PlayState;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};

fn call(app: &mut App, id: &'static str) -> OperatorResult {
    let result = app
        .world_mut()
        .operator(id)
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

fn play_state(app: &App) -> PlayState {
    app.world().resource::<State<PlayState>>().get().clone()
}

#[test]
fn with_no_game_running_pause_and_stop_are_refused_and_the_editor_settles_on_stopped() {
    let project = tempfile::tempdir().expect("tempdir");
    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: project.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    let lamp = app
        .world_mut()
        .spawn((Name::new("Lamp"), Transform::from_xyz(1.0, 2.0, 3.0)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), lamp);
    app.update();

    call(&mut app, "pie.pause").assert_cancelled();
    call(&mut app, "pie.stop").assert_cancelled();

    app.world_mut()
        .resource_mut::<NextState<PlayState>>()
        .set(PlayState::Playing);
    for _ in 0..4 {
        app.update();
    }

    assert_eq!(
        play_state(&app),
        PlayState::Stopped,
        "a play state with no game behind it returns to authoring"
    );
    assert_eq!(
        app.world().get::<Transform>(lamp),
        Some(&Transform::from_xyz(1.0, 2.0, 3.0)),
        "with the authored entity where it was authored"
    );
    call(&mut app, "pie.pause").assert_cancelled();
}

/// The window whose tab is in front of `leaf`.
fn front_window(app: &App, leaf: jackdaw_panels::tree::NodeId) -> Option<String> {
    let tree = app.world().resource::<jackdaw_panels::tree::DockTree>();
    let leaf = tree.get(leaf)?.as_leaf()?;
    let active = leaf.active?;
    leaf.tabs()
        .find_map(|(window, tab)| (tab == active).then(|| window.to_string()))
}

/// A bottom dock holding the Build and Game panels, Build in front.
fn bottom_dock(app: &mut App) -> jackdaw_panels::tree::NodeId {
    use jackdaw_panels::{
        DockAreaStyle,
        tree::{DockLeaf, DockTree},
    };

    let leaf = app.world_mut().resource_mut::<DockTree>().set_root_leaf(
        DockLeaf::new("bottom_dock", DockAreaStyle::default()).with_windows(vec![
            "jackdaw.build".to_string(),
            jackdaw::game_panel::GAME_WINDOW_ID.to_string(),
        ]),
    );
    app.update();
    assert_eq!(front_window(app, leaf).as_deref(), Some("jackdaw.build"));
    leaf
}

#[test]
fn the_game_panel_comes_forward_when_play_starts_and_steps_back_when_it_ends() {
    let mut app = util::editor_test_app();
    let leaf = bottom_dock(&mut app);

    app.world_mut()
        .resource_mut::<NextState<PlayState>>()
        .set(PlayState::Playing);
    app.update();
    assert_eq!(
        front_window(&app, leaf).as_deref(),
        Some(jackdaw::game_panel::GAME_WINDOW_ID),
        "the running game is in front, so it is laid out and streams"
    );

    app.world_mut()
        .resource_mut::<NextState<PlayState>>()
        .set(PlayState::Stopped);
    app.update();
    assert_eq!(
        front_window(&app, leaf).as_deref(),
        Some("jackdaw.build"),
        "and the panel that was in front before Play is back after it"
    );
}

#[test]
fn a_game_in_its_own_window_leaves_the_dock_alone() {
    let mut app = util::editor_test_app();
    let leaf = bottom_dock(&mut app);
    *app.world_mut()
        .resource_mut::<jackdaw::pie::PieWindowMode>() = jackdaw::pie::PieWindowMode::Windowed;

    app.world_mut()
        .resource_mut::<NextState<PlayState>>()
        .set(PlayState::Playing);
    app.update();

    assert_eq!(play_state(&app), PlayState::Playing);
    assert_eq!(front_window(&app, leaf).as_deref(), Some("jackdaw.build"));
}
