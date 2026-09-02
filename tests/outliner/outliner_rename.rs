//! F2 renames what is selected.
//!
//! The chord is pressed through the window's own key stream, because the
//! question is whether the outliner answers a keypress at all: an operator
//! called by hand always finds its target, and the friction was that a
//! bare F2 found nothing unless a row had been clicked first.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::{
    prelude::*,
    window::{PrimaryWindow, WindowResolution},
};
use jackdaw::hierarchy::{HierarchyShowAll, HierarchyTreeContainer};
use jackdaw::test_input::SyntheticInput;
use jackdaw_widgets::tree_view::TreeIndex;

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

fn play(app: &mut App) {
    for _ in 0..600 {
        app.update();
        if app.world().resource::<SyntheticInput>().is_idle() {
            break;
        }
    }
    assert!(
        app.world().resource::<SyntheticInput>().is_idle(),
        "the gesture drained",
    );
    settle(app);
}

fn run(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    play(app);
}

/// An editor with one outliner list and one named node in the document.
fn outliner_app() -> (App, Entity, Entity) {
    let mut app = util::editor_test_app();
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows
            .single_mut(app.world_mut())
            .expect("headless apps still have a primary window");
        window.resolution = WindowResolution::new(1600, 1000);
    }
    app.world_mut().insert_resource(HierarchyShowAll(true));
    let panel = app
        .world_mut()
        .spawn((
            HierarchyTreeContainer,
            Node {
                width: px(320),
                height: px(600),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .id();
    let node = app
        .world_mut()
        .spawn((Name::new("Panel"), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), node);
    settle(&mut app);
    (app, panel, node)
}

/// Whether the outliner is showing an open rename entry.
fn entry_open(app: &mut App) -> bool {
    use jackdaw_widgets::tree_view::TreeRowInlineRename;
    app.world_mut()
        .query_filtered::<Entity, With<TreeRowInlineRename>>()
        .iter(app.world())
        .next()
        .is_some()
}

/// F2 renames the primary selection, with nothing clicked in the outliner
/// first. Selecting a node is how a user says which one they mean; making
/// them click its row again to say it a second time is the friction.
#[test]
fn f2_opens_the_entry_on_the_selected_row() {
    let (mut app, panel, node) = outliner_app();
    jackdaw::selection::select_only(app.world_mut(), node);
    settle(&mut app);
    assert!(
        app.world()
            .resource::<TreeIndex>()
            .get(panel, node)
            .is_some(),
        "the selected node has a row to rename",
    );

    run(&mut app, "input.key key=F2");

    assert!(
        entry_open(&mut app),
        "F2 opens the entry on the row the selection names",
    );
}

/// The same chord on a selection whose row is not on screen yet.
///
/// A node selected on the canvas, or one just added under a closed
/// parent, has no row until the outliner is opened down to it. The rename
/// looked the row up and gave up when it found none, so F2 did nothing at
/// all -- and the only way out was to hunt the row down and click it,
/// which is the click the chord exists to avoid.
#[test]
fn f2_reaches_a_selection_whose_row_is_not_open_yet() {
    let (mut app, panel, parent) = outliner_app();
    let child = app
        .world_mut()
        .spawn((Name::new("Child"), Node::default(), ChildOf(parent)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), child);
    settle(&mut app);
    assert!(
        app.world()
            .resource::<TreeIndex>()
            .get(panel, child)
            .is_none(),
        "the parent is closed, so the child has no row",
    );

    jackdaw::selection::select_only(app.world_mut(), child);
    settle(&mut app);
    run(&mut app, "input.key key=F2");

    assert!(
        entry_open(&mut app),
        "F2 opens the entry on the selected node, opening the outliner down to it",
    );
}
