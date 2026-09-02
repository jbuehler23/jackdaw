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

/// The text a row is drawing, and the tooltip hung off it.
fn label_of(app: &mut App, panel: Entity, source: Entity) -> (String, Option<String>) {
    use jackdaw_feathers::tooltip::Tooltip;
    use jackdaw_widgets::tree_view::{TreeRowContent, TreeRowLabel};

    let row = app
        .world()
        .resource::<TreeIndex>()
        .get(panel, source)
        .expect("the outliner shows a row for the entity");
    let world = app.world();
    let child_with = |parent: Entity, has: &dyn Fn(Entity) -> bool| -> Option<Entity> {
        world
            .get::<Children>(parent)?
            .iter()
            .find(|&child| has(child))
    };
    let content =
        child_with(row, &|e| world.get::<TreeRowContent>(e).is_some()).expect("a row has content");
    let label = child_with(content, &|e| world.get::<TreeRowLabel>(e).is_some())
        .expect("a row has a label");
    (
        world
            .get::<Text>(label)
            .expect("the label holds text")
            .0
            .clone(),
        world.get::<Tooltip>(label).map(|tip| tip.title.clone()),
    )
}

/// A name too long for the panel is cut down to what the row can show,
/// and the whole of it is a hover away.
///
/// Uncut, the label was the row's own minimum width: it pushed the lock
/// and the eye out of the panel and left the rename entry a few pixels
/// wide, so a renamed node could not be read back at all.
#[test]
fn a_long_row_name_is_cut_down_with_the_whole_of_it_in_a_tooltip() {
    let (mut app, panel, _node) = outliner_app();
    let long = "AVeryLongWidgetNameThatNoOutlinerPanelCouldEverShowInFull";
    let wordy = app
        .world_mut()
        .spawn((Name::new(long.to_string()), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), wordy);
    let brief = app
        .world_mut()
        .spawn((Name::new("Short"), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), brief);
    settle(&mut app);

    let (shown, tooltip) = label_of(&mut app, panel, wordy);
    assert!(
        shown.len() < long.len(),
        "the name is cut to fit the row: {shown}",
    );
    assert!(shown.ends_with("..."), "and says so: {shown}");
    assert!(
        long.starts_with(shown.trim_end_matches('.')),
        "what is left is the front of the name: {shown}",
    );
    assert_eq!(
        tooltip.as_deref(),
        Some(long),
        "and the whole of it is on the tooltip",
    );

    let (shown, tooltip) = label_of(&mut app, panel, brief);
    assert_eq!(shown, "Short", "a name that fits is left alone");
    assert_eq!(tooltip, None, "and carries no tooltip repeating itself");
}

/// Resting the pointer on a cut label brings the whole name up.
///
/// The tooltip is a dwell, not a move: it only appears once the pointer
/// has been still on the row for long enough, which is what
/// `input.pointer action=rest` is for.
#[test]
fn resting_on_a_cut_label_shows_the_whole_name() {
    use bevy::ui::UiGlobalTransform;
    use jackdaw_feathers::popover::EditorPopover;
    use jackdaw_widgets::tree_view::{TreeRowContent, TreeRowLabel};

    let (mut app, panel, _node) = outliner_app();
    let long = "AVeryLongWidgetNameThatNoOutlinerPanelCouldEverShowInFull";
    let wordy = app
        .world_mut()
        .spawn((Name::new(long.to_string()), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), wordy);
    settle(&mut app);

    let row = app
        .world()
        .resource::<TreeIndex>()
        .get(panel, wordy)
        .expect("the long name has a row");
    let label = {
        let world = app.world();
        let child_with = |parent: Entity, has: &dyn Fn(Entity) -> bool| -> Option<Entity> {
            world
                .get::<Children>(parent)?
                .iter()
                .find(|&child| has(child))
        };
        let content = child_with(row, &|e| world.get::<TreeRowContent>(e).is_some())
            .expect("a row has content");
        child_with(content, &|e| world.get::<TreeRowLabel>(e).is_some()).expect("a row has a label")
    };
    let at = {
        let transform = app
            .world()
            .get::<UiGlobalTransform>(label)
            .expect("the label is placed");
        let computed = app
            .world()
            .get::<ComputedNode>(label)
            .expect("the label is laid out");
        transform.translation
            * computed.inverse_scale_factor()
            * app.world().resource::<UiScale>().0
    };

    let popovers = |app: &mut App| {
        app.world_mut()
            .query_filtered::<Entity, With<EditorPopover>>()
            .iter(app.world())
            .count()
    };
    let hovered = |app: &App| {
        app.world()
            .get::<bevy::picking::hover::Hovered>(label)
            .map(|hovered| hovered.0)
    };
    assert_eq!(hovered(&app), Some(false), "nothing is hovered yet");
    assert_eq!(popovers(&mut app), 0);

    run(
        &mut app,
        &format!("input.pointer x={} y={} action=move", at.x, at.y),
    );
    run(&mut app, "input.pointer action=rest steps=8 frames=2");

    assert_eq!(
        hovered(&app),
        Some(true),
        "the label the whole name hangs off is what the pointer rests on",
    );
    assert_eq!(
        app.world()
            .get::<jackdaw_feathers::tooltip::Tooltip>(label)
            .map(|tip| tip.title.clone()),
        Some(long.to_string()),
        "and what it is resting on carries the name in full",
    );
}
