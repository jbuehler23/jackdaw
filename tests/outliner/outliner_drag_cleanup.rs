//! What a finished outliner drag leaves behind.
//!
//! A drag paints as it travels: the row under the pointer takes a drop
//! tint, the gap it would land in takes a line, and the list itself takes
//! a wash meaning "release here and the entity leaves its parent". All
//! three are meant to be gone the moment the gesture ends.
//!
//! They were not. `DragEnter` bubbles and the gap strips over every row
//! stop their `DragLeave` and `DragDrop` but not their `DragEnter`, so a
//! drag that merely crossed a row washed the whole panel green and
//! nothing ever painted it back. The pointer is driven through the
//! window's own event streams here, because that asymmetry only exists
//! along the real propagation path: a test that triggers `DragDrop` on
//! the zone by hand never reaches the container's observers at all.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::{
    prelude::*,
    ui::UiGlobalTransform,
    window::{PrimaryWindow, WindowResolution},
};
use jackdaw::hierarchy::{HierarchyShowAll, HierarchyTreeContainer};
use jackdaw::selection::Selection;
use jackdaw::test_input::SyntheticInput;
use jackdaw_widgets::tree_view::{
    TreeDropLine, TreeIndex, TreeNodeExpanded, TreeRowContent, TreeSpringLoad,
};

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

/// An editor with one outliner list, laid out at the top left of a window
/// big enough to drag across.
fn outliner_app() -> (App, Entity) {
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
            jackdaw_feathers::tree_view::tree_container_drop_observers(),
        ))
        .id();
    settle(&mut app);
    (app, panel)
}

/// One column with three named children, registered in the document the
/// way a load leaves it, with the column opened so all four have rows.
fn column_of_three(app: &mut App, panel: Entity) -> Vec<Entity> {
    let world = app.world_mut();
    let column = world
        .spawn((
            Name::new("Column"),
            Node {
                flex_direction: FlexDirection::Column,
                ..default()
            },
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, column);
    let children: Vec<Entity> = ["First", "Second", "Third"]
        .into_iter()
        .map(|name| {
            let child = world
                .spawn((Name::new(name), Node::default(), ChildOf(column)))
                .id();
            jackdaw::scene_io::register_entity_in_ast(world, child);
            child
        })
        .collect();
    settle(app);
    let row = app
        .world()
        .resource::<TreeIndex>()
        .get(panel, column)
        .expect("the column has a row");
    app.world_mut()
        .entity_mut(row)
        .insert(TreeNodeExpanded(true));
    settle(app);
    children
}

/// Where `entity` is drawn, in the window logical pixels
/// `input.pointer` takes.
fn centre_of(app: &App, entity: Entity) -> Vec2 {
    let transform = app
        .world()
        .get::<UiGlobalTransform>(entity)
        .expect("the node is placed");
    let computed = app
        .world()
        .get::<ComputedNode>(entity)
        .expect("the node is laid out");
    transform.translation * computed.inverse_scale_factor() * app.world().resource::<UiScale>().0
}

/// The clickable content of the row standing for `source`.
fn row_content(app: &mut App, panel: Entity, source: Entity) -> Entity {
    let row = app
        .world()
        .resource::<TreeIndex>()
        .get(panel, source)
        .expect("the outliner shows a row for the entity");
    app.world()
        .get::<Children>(row)
        .expect("a row has children")
        .iter()
        .find(|child| app.world().get::<TreeRowContent>(*child).is_some())
        .expect("a row has content")
}

/// A row's drawn height, in the logical pixels `input.pointer` takes.
fn row_height(app: &App, content: Entity) -> f32 {
    let computed = app
        .world()
        .get::<ComputedNode>(content)
        .expect("the row is laid out");
    computed.size().y * computed.inverse_scale_factor()
}

fn container_colour(app: &App, panel: Entity) -> Color {
    app.world()
        .get::<BackgroundColor>(panel)
        .expect("the list paints a background")
        .0
}

/// A drag across the list and a drop on a row leaves nothing painted and
/// nothing armed, and the click that follows selects.
#[test]
fn a_finished_drag_leaves_the_outliner_clean_and_answering() {
    let (mut app, panel) = outliner_app();
    let children = column_of_three(&mut app, panel);
    settle(&mut app);

    let first = row_content(&mut app, panel, children[0]);
    let third = row_content(&mut app, panel, children[2]);
    let from = centre_of(&app, first);
    // Released on the gap above the last row, which is where the strips
    // stop the container's own `DragLeave` and `DragDrop`: with the
    // enter unguarded, that is a wash nothing ever paints back.
    let to = centre_of(&app, third) - Vec2::new(0.0, row_height(&app, third) * 0.4);

    run(
        &mut app,
        &format!("input.pointer x={} y={} action=move", from.x, from.y),
    );
    run(
        &mut app,
        &format!("input.pointer x={} y={} action=drag_to steps=8", to.x, to.y),
    );

    assert_eq!(
        container_colour(&app, panel),
        Color::NONE,
        "the list is not left washed green",
    );
    assert!(
        app.world().resource::<TreeSpringLoad>().row.is_none(),
        "nothing is left resting under a drag that is over",
    );
    assert!(
        app.world().resource::<TreeDropLine>().zone.is_none(),
        "and no drop line is left drawn",
    );

    // And the panel still answers a pointer.
    let second = row_content(&mut app, panel, children[1]);
    let at = centre_of(&app, second);
    run(
        &mut app,
        &format!("input.pointer x={} y={} action=click", at.x, at.y),
    );
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![children[1]],
        "the click after the drag selected the row it landed on",
    );
}

/// A gap strip covers the top and the bottom third of every row, so most
/// of a row is gap. A click there is a click on the row: without that,
/// only the middle third of the outliner answered a pointer at all.
#[test]
fn a_click_on_the_gap_over_a_row_selects_that_row() {
    let (mut app, panel) = outliner_app();
    let children = column_of_three(&mut app, panel);
    settle(&mut app);

    let content = row_content(&mut app, panel, children[1]);
    let centre = centre_of(&app, content);
    let height = row_height(&app, content);

    // A whisker below the middle, which is the after-gap's strip.
    let at = centre + Vec2::new(0.0, height * 0.4);
    run(
        &mut app,
        &format!("input.pointer x={} y={} action=click", at.x, at.y),
    );
    assert_eq!(
        app.world().resource::<Selection>().entities,
        vec![children[1]],
        "the gap over a row belongs to the row",
    );
}

/// The colour a row's content is painted, which is what says whether a
/// drag is still hanging over it.
fn row_colour(app: &App, content: Entity) -> Color {
    app.world()
        .get::<BackgroundColor>(content)
        .expect("a row paints a background")
        .0
}

/// Escape during a drag calls it off: the row it was hanging over is
/// painted back, no drop line is left drawn, and the release that follows
/// moves nothing.
#[test]
fn escape_during_a_drag_calls_it_off() {
    let (mut app, panel) = outliner_app();
    let children = column_of_three(&mut app, panel);
    settle(&mut app);

    let first = row_content(&mut app, panel, children[0]);
    let third = row_content(&mut app, panel, children[2]);
    let from = centre_of(&app, first);
    let to = centre_of(&app, third);
    let column = app
        .world()
        .get::<ChildOf>(children[0])
        .expect("the rows have a parent")
        .parent();
    let order_before = child_names(&app, column);

    run(
        &mut app,
        &format!("input.pointer x={} y={} action=move", from.x, from.y),
    );
    run(
        &mut app,
        &format!("input.pointer x={} y={} action=press", from.x, from.y),
    );
    run(
        &mut app,
        &format!("input.pointer x={} y={} action=move", to.x, to.y),
    );
    assert_eq!(
        row_colour(&app, third),
        jackdaw_feathers::tokens::DROP_TARGET_BG,
        "the row under the drag is tinted while it is over it",
    );

    run(&mut app, "input.key key=Escape");

    assert_ne!(
        row_colour(&app, third),
        jackdaw_feathers::tokens::DROP_TARGET_BG,
        "Escape paints the row back",
    );
    assert!(
        app.world().resource::<TreeDropLine>().zone.is_none(),
        "and leaves no drop line drawn",
    );

    run(
        &mut app,
        &format!("input.pointer x={} y={} action=release", to.x, to.y),
    );
    assert_eq!(
        child_names(&app, column),
        order_before,
        "and the release that follows a cancelled drag moves nothing",
    );
}

/// The names of an entity's children, in order.
fn child_names(app: &App, entity: Entity) -> Vec<String> {
    app.world()
        .get::<Children>(entity)
        .map(|children| {
            children
                .iter()
                .filter_map(|child| app.world().get::<Name>(child))
                .map(|name| name.as_str().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// A drag begun on the list's own empty space paints nothing.
///
/// The wash means "release here and the entity leaves its parent". A
/// press on the empty space below the rows is holding no entity at all,
/// and the whole panel turning green until the button came back up said
/// otherwise.
#[test]
fn a_drag_from_empty_space_paints_nothing() {
    let (mut app, panel) = outliner_app();
    let children = column_of_three(&mut app, panel);
    settle(&mut app);

    // Below the last row, which is the container's own space.
    let last = row_content(&mut app, panel, children[2]);
    let empty = centre_of(&app, last) + Vec2::new(0.0, row_height(&app, last) * 4.0);

    run(
        &mut app,
        &format!("input.pointer x={} y={} action=move", empty.x, empty.y),
    );
    run(
        &mut app,
        &format!("input.pointer x={} y={} action=press", empty.x, empty.y),
    );
    run(
        &mut app,
        &format!(
            "input.pointer x={} y={} action=move",
            empty.x + 40.0,
            empty.y - 10.0
        ),
    );
    assert_eq!(
        container_colour(&app, panel),
        Color::NONE,
        "a drag holding no row washes nothing",
    );

    run(
        &mut app,
        &format!(
            "input.pointer x={} y={} action=release",
            empty.x + 40.0,
            empty.y - 10.0
        ),
    );
    assert_eq!(
        container_colour(&app, panel),
        Color::NONE,
        "and the release leaves it as it was",
    );
}

/// A row dragged into a second list does paint that list, and the
/// release paints it back: the gate is what is being dragged, not the
/// wash itself.
#[test]
fn a_row_dragged_into_another_list_paints_it_and_clears() {
    let (mut app, panel) = outliner_app();
    let children = column_of_three(&mut app, panel);
    let second = app
        .world_mut()
        .spawn((
            HierarchyTreeContainer,
            Node {
                position_type: PositionType::Absolute,
                left: px(400),
                top: px(0),
                width: px(320),
                height: px(600),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            BackgroundColor(Color::NONE),
            jackdaw_feathers::tree_view::tree_container_drop_observers(),
        ))
        .id();
    settle(&mut app);

    let first = row_content(&mut app, panel, children[0]);
    let from = centre_of(&app, first);
    // The second list's own space, below every row it drew.
    let over = centre_of(&app, second) + Vec2::new(0.0, 200.0);

    run(
        &mut app,
        &format!("input.pointer x={} y={} action=move", from.x, from.y),
    );
    run(
        &mut app,
        &format!("input.pointer x={} y={} action=press", from.x, from.y),
    );
    run(
        &mut app,
        &format!("input.pointer x={} y={} action=move", over.x, over.y),
    );
    assert_eq!(
        container_colour(&app, second),
        jackdaw_feathers::tokens::CONTAINER_DROP_TARGET_BG,
        "a row over another list's own space is a drop out of its parent",
    );

    run(
        &mut app,
        &format!("input.pointer x={} y={} action=release", over.x, over.y),
    );
    assert_eq!(
        container_colour(&app, second),
        Color::NONE,
        "and the release paints it back",
    );
}
