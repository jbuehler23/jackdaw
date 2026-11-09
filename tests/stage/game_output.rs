//! The Game Output window: what a game printed, kept per instance and opened
//! from the Game panel.

use bevy::prelude::*;
use jackdaw::game_output::{GameOutput, OutputStream};
use jackdaw::game_output_window::{GAME_OUTPUT_WINDOW_ID, GameOutputRow};
use jackdaw::game_panel::GameIdlePart;
use jackdaw_panels::tree::DockTree;

use super::game_panel_states::{
    editor_on_a_project, key, press, recorded_exit, shown, update_until,
};
use crate::util::OperatorResultExt as _;

/// The lines `config`'s instance printed, with their streams.
fn buffered(app: &App, config: &str) -> Vec<(OutputStream, String)> {
    app.world()
        .resource::<GameOutput>()
        .tail(&key(config), None, "", usize::MAX)
        .map(|page| {
            page.lines
                .into_iter()
                .map(|line| (line.stream, line.text))
                .collect()
        })
        .unwrap_or_default()
}

/// The rows the Game Output window shows, in order.
fn rows(app: &mut App) -> Vec<(OutputStream, String)> {
    let mut rows: Vec<(Entity, OutputStream, String)> = app
        .world_mut()
        .query::<(Entity, &GameOutputRow, &Text)>()
        .iter(app.world())
        .map(|(entity, row, text)| (entity, row.0, text.0.clone()))
        .collect();
    rows.sort_by_key(|(entity, ..)| *entity);
    rows.into_iter()
        .map(|(_, stream, text)| (stream, text))
        .collect()
}

fn window_is_front(app: &App) -> bool {
    let tree = app.world().resource::<DockTree>();
    let Some(leaf) = tree.find_leaf_with_window(GAME_OUTPUT_WINDOW_ID) else {
        return false;
    };
    let Some(dock) = tree.get(leaf).and_then(|node| node.as_leaf()) else {
        return false;
    };
    dock.tabs()
        .any(|(window, tab)| window == GAME_OUTPUT_WINDOW_ID && dock.active == Some(tab))
}

/// Hold `script` as the project's built game.
#[cfg(unix)]
fn built_game(app: &mut App, project: &tempfile::TempDir, script: &str) {
    use std::os::unix::fs::PermissionsExt;

    let binary = project.path().join("game.sh");
    std::fs::write(&binary, format!("#!/bin/sh\n{script}")).expect("a game script");
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))
        .expect("an executable game");
    jackdaw::pie::insert_built_game(app.world_mut(), binary);
}

/// Press Play and wait for the game to exit.
#[cfg(unix)]
fn play_until_exit(app: &mut App) {
    super::game_panel_states::call(app, "pie.play").assert_finished();
    update_until(app, "the exit to be recorded", |app| {
        recorded_exit(app).is_some()
    });
    app.update();
}

#[cfg(unix)]
#[test]
fn a_game_printing_to_both_streams_shows_both_in_the_buffer_and_the_window() {
    let (mut app, _leaf, project) = editor_on_a_project();
    built_game(
        &mut app,
        &project,
        "echo 'hello from stdout'\necho '\u{1b}[31mhello from stderr\u{1b}[0m' >&2\nexit 3\n",
    );
    play_until_exit(&mut app);
    let expected = vec![
        (OutputStream::Stdout, "hello from stdout".to_string()),
        (OutputStream::Stderr, "hello from stderr".to_string()),
    ];
    let mut lines = buffered(&app, "Client");
    lines.sort();
    assert_eq!(lines, expected);

    press(&mut app, GameIdlePart::ShowOutput).assert_finished();
    update_until(&mut app, "the window to show the lines", |app| {
        rows(app).len() == 2
    });
    let mut shown_rows = rows(&mut app);
    shown_rows.sort();
    assert_eq!(shown_rows, expected);
}

#[cfg(unix)]
#[test]
fn show_output_opens_the_game_output_window() {
    let (mut app, _leaf, project) = editor_on_a_project();
    built_game(
        &mut app,
        &project,
        "echo 'the level is missing' >&2\nexit 3\n",
    );
    play_until_exit(&mut app);
    assert!(shown(&mut app).contains(&GameIdlePart::ShowOutput));
    assert!(!window_is_front(&app));

    press(&mut app, GameIdlePart::ShowOutput).assert_finished();
    assert!(
        window_is_front(&app),
        "Show output brings the window forward"
    );
}

#[cfg(unix)]
#[test]
fn output_survives_the_exit_and_the_next_play_clears_it() {
    let (mut app, _leaf, project) = editor_on_a_project();
    built_game(&mut app, &project, "echo 'first run'\nexit 3\n");
    play_until_exit(&mut app);
    assert_eq!(
        buffered(&app, "Client"),
        [(OutputStream::Stdout, "first run".to_string())],
        "the output stays after the game exits"
    );
    press(&mut app, GameIdlePart::ShowOutput).assert_finished();
    update_until(&mut app, "the window to show the first run", |app| {
        rows(app).len() == 1
    });

    built_game(&mut app, &project, "echo 'second run'\nexit 3\n");
    play_until_exit(&mut app);
    assert_eq!(
        buffered(&app, "Client"),
        [(OutputStream::Stdout, "second run".to_string())]
    );
    update_until(&mut app, "the window to show only the second run", |app| {
        rows(app) == [(OutputStream::Stdout, "second run".to_string())]
    });
}
