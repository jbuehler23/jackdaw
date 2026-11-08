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

/// Whether the transport button running `op_id` can be clicked.
fn enabled(app: &mut App, op_id: &str) -> bool {
    let mut buttons = app.world_mut().query::<(
        &jackdaw_feathers::button::ButtonOperatorCall,
        Has<bevy::ui::InteractionDisabled>,
    )>();
    let found: Vec<bool> = buttons
        .iter(app.world())
        .filter(|(call, _)| call.id == op_id)
        .map(|(_, disabled)| !disabled)
        .collect();
    assert!(!found.is_empty(), "the toolbar has a {op_id} button");
    found.into_iter().all(|enabled| enabled)
}

fn play_caption(app: &mut App) -> String {
    let button = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::layout::PlayButton>>()
        .single(app.world())
        .expect("one Play button");
    let mut stack = vec![button];
    while let Some(entity) = stack.pop() {
        if app
            .world()
            .get::<jackdaw_feathers::button::ButtonContentText>(entity)
            .is_some()
        {
            return app
                .world()
                .get::<Text>(entity)
                .expect("a caption")
                .0
                .clone();
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    panic!("the Play button has a caption");
}

fn footer(app: &mut App) -> String {
    app.world_mut()
        .query_filtered::<&Text, With<jackdaw_feathers::status_bar::StatusBarCenter>>()
        .single(app.world())
        .expect("one footer centre")
        .0
        .clone()
}

/// Whether the footer's idle readouts, "Ready" on the left and the remote
/// connection indicator, are showing.
fn idle_readouts_shown(app: &mut App) -> [bool; 2] {
    let mut left = app
        .world_mut()
        .query_filtered::<&Visibility, With<jackdaw_feathers::status_bar::StatusBarLeft>>();
    let left = left.single(app.world()).expect("one left slot") != Visibility::Hidden;
    let mut slot = app
        .world_mut()
        .query_filtered::<&Visibility, With<jackdaw::remote_mode::ConnectionIndicatorSlot>>();
    let slot = slot.single(app.world()).expect("one connection slot") != Visibility::Hidden;
    [left, slot]
}

fn footer_right(app: &mut App) -> String {
    app.world_mut()
        .query_filtered::<&Text, With<jackdaw_feathers::status_bar::StatusBarRight>>()
        .single(app.world())
        .expect("one footer right")
        .0
        .clone()
}

/// (Play, Pause, Step, Stop) as clickable or not.
fn transport(app: &mut App) -> [bool; 4] {
    ["pie.play", "pie.pause", "pie.step", "pie.stop"].map(|id| enabled(app, id))
}

#[test]
fn the_transport_buttons_follow_a_launch_from_stopped_through_building_and_back() {
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

    assert_eq!(
        transport(&mut app),
        [true, false, false, false],
        "stopped: Play only"
    );
    assert_eq!(play_caption(&mut app), "Play");
    assert!(footer(&mut app).starts_with("Jackdaw v"));
    assert_eq!(idle_readouts_shown(&mut app), [true, true]);

    jackdaw::pie::insert_build_in_progress(app.world_mut());
    call(&mut app, "pie.play").assert_finished();
    app.update();
    assert_eq!(
        transport(&mut app),
        [false, false, false, true],
        "building: Stop cancels, nothing else"
    );
    assert_eq!(play_caption(&mut app), "Building...");
    assert_eq!(
        footer(&mut app).split(" | ").next(),
        Some("Building: Client"),
        "the footer names the run and nothing more"
    );
    assert_eq!(
        idle_readouts_shown(&mut app),
        [false, false],
        "no Ready and no connection state while Play is under way"
    );
    let right = footer_right(&mut app);
    assert!(
        !right.contains("Ready") && !right.contains("Disconnected"),
        "{right}"
    );
    call(&mut app, "pie.play").assert_cancelled();

    call(&mut app, "pie.stop").assert_finished();
    app.update();
    assert_eq!(transport(&mut app), [true, false, false, false]);
    assert_eq!(play_caption(&mut app), "Play");
    assert!(jackdaw::pie::launches_waiting_on_build(app.world()).is_empty());
    assert!(footer(&mut app).starts_with("Jackdaw v"));
    assert_eq!(idle_readouts_shown(&mut app), [true, true]);
}

/// The transport buttons take the feathers text colour but keep their fonts,
/// so the icon glyphs still draw from the icon font.
#[test]
fn the_transport_icons_keep_the_icon_font() {
    let mut app = util::editor_test_app();
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    for _ in 0..4 {
        app.update();
    }
    let icon_font = app
        .world()
        .resource::<jackdaw_feathers::icons::IconFont>()
        .0
        .clone();
    let mut buttons = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::layout::TransportButton>>();
    let buttons: Vec<Entity> = buttons.iter(app.world()).collect();
    assert_eq!(
        buttons.len(),
        5,
        "Play, the menu caret, Pause, Step and Stop"
    );
    for button in buttons {
        let glyph = app
            .world()
            .get::<Children>(button)
            .into_iter()
            .flat_map(Children::iter)
            .find(|child| app.world().get::<Text>(*child).is_some())
            .expect("an icon glyph");
        let font = app.world().get::<TextFont>(glyph).expect("a font");
        assert_eq!(font.font, icon_font.clone().into(), "{button:?}");
    }
}
