//! Lists and scrolling containers.
//!
//! A list is `ListBox`, a row is `FeathersListRow`, and a scrolling pair
//! is `ScrollArea` beside `Scrollbar`: the wheel and the thumb are the
//! widgets', not the editor's.

use crate::util;

use bevy::camera::{NormalizedRenderTarget, RenderTarget};
use bevy::feathers::controls::FeathersListRow;
use bevy::picking::{
    backend::HitData,
    events::{Pointer, Scroll},
    pointer::{Location, PointerId},
};
use bevy::prelude::*;
use bevy::ui_widgets::{ListBox, ListItem, ScrollArea, Scrollbar, ScrollbarThumb};
use bevy::window::{PrimaryWindow, WindowRef};

use jackdaw_feathers::list_view::{list_row, list_view};
use jackdaw_feathers::scroll::{scroll_area, scrollbar};

fn window_target(app: &mut App) -> NormalizedRenderTarget {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    RenderTarget::Window(WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes")
}

/// A list container is the list widget, and a row spawned into it comes
/// back as a feathers list row: a `ListItem` the widget paints.
#[test]
fn a_list_is_a_list_box_and_its_rows_are_list_items() {
    let mut app = util::editor_test_app();

    let list = app.world_mut().spawn(list_view()).id();
    let row = app
        .world_mut()
        .spawn((list_row(), Node::default(), ChildOf(list)))
        .id();
    app.update();
    app.update();

    assert!(
        app.world().get::<ListBox>(list).is_some(),
        "the container is the list widget",
    );
    assert!(
        app.world().get::<FeathersListRow>(row).is_some(),
        "the row's scene landed on the row",
    );
    assert!(
        app.world().get::<ListItem>(row).is_some(),
        "so the row is a list item the widget knows",
    );
    assert!(
        app.world().get::<Interaction>(row).is_none(),
        "and not a hand-rolled interaction control",
    );
}

/// A row keeps the layout it was spawned with. The scene writes a row
/// `Node` of its own over the entity, and a caller that sized or padded
/// its row means that.
#[test]
fn a_list_row_keeps_the_layout_it_was_spawned_with() {
    let mut app = util::editor_test_app();

    let row = app
        .world_mut()
        .spawn((
            list_row(),
            Node {
                column_gap: px(17.0),
                ..default()
            },
            children![Text::new("one")],
        ))
        .id();
    app.update();
    app.update();

    assert_eq!(
        app.world().get::<Node>(row).map(|node| node.column_gap),
        Some(px(17.0)),
        "the caller's layout outlives the scene's",
    );
    let children = app
        .world()
        .get::<Children>(row)
        .expect("the row still has its caption");
    assert!(
        children
            .iter()
            .any(|child| app.world().get::<Text>(child).is_some()),
        "and so does its caption",
    );
}

/// The bar beside a scrolling container is the scrollbar widget, aimed
/// at that container, with the widget's own draggable thumb inside it.
#[test]
fn a_scrollbar_is_the_widget_aimed_at_its_container() {
    let mut app = util::editor_test_app();

    let container = app
        .world_mut()
        .spawn((
            Node {
                overflow: Overflow::scroll_y(),
                ..default()
            },
            scroll_area(),
        ))
        .id();
    let bar = app.world_mut().spawn(scrollbar(container)).id();
    app.update();

    let widget = app
        .world()
        .get::<Scrollbar>(bar)
        .expect("the bar is the scrollbar widget");
    assert_eq!(widget.target, container, "and it scrolls its container");
    let thumb = app
        .world()
        .get::<Children>(bar)
        .expect("a scrollbar holds a thumb")
        .iter()
        .next()
        .expect("a scrollbar holds a thumb");
    assert!(
        app.world().get::<ScrollbarThumb>(thumb).is_some(),
        "the thumb is the widget's, so it can be dragged",
    );
    assert!(
        app.world().get::<ScrollArea>(container).is_some(),
        "and the wheel over the container is the widget's too",
    );
}

/// A wheel over the container moves it, through the scroll area rather
/// than through the editor's own handler.
#[test]
fn scrolling_a_scroll_area_moves_its_scroll_position() {
    let mut app = util::editor_test_app();

    let container = app
        .world_mut()
        .spawn((
            Node {
                width: px(100.0),
                height: px(100.0),
                overflow: Overflow::scroll_y(),
                ..default()
            },
            scroll_area(),
            children![Node {
                width: px(100.0),
                height: px(1000.0),
                ..default()
            }],
        ))
        .id();
    app.update();
    app.update();

    let target = window_target(&mut app);
    app.world_mut().trigger(Pointer::new(
        PointerId::Mouse,
        Location {
            target,
            position: Vec2::ZERO,
        },
        Scroll {
            unit: bevy::input::mouse::MouseScrollUnit::Pixel,
            x: 0.0,
            y: -40.0,
            hit: HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            phase: bevy::input::touch::TouchPhase::Moved,
        },
        container,
    ));
    app.update();

    let position = app
        .world()
        .get::<ScrollPosition>(container)
        .expect("a scroll area carries its position");
    assert!(
        position.y > 0.0,
        "the wheel moved the scroll area, got {}",
        position.y,
    );
}
