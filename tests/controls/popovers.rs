//! Popovers.
//!
//! A popover is placed by `bevy_ui_widgets`: it hangs off what it points
//! at and the widget picks the side with room. The editor supplies the
//! frame, the header and the dismissal.

use crate::util;

use bevy::prelude::*;
use bevy::ui_widgets::popover::{Popover, PopoverAlign, PopoverSide};

use jackdaw_feathers::popover::{PopoverPlacement, PopoverProps, popover};

/// A popover carries the widget's placement component and is parented to
/// its anchor, which is the rectangle the widget places it against.
#[test]
fn a_popover_is_placed_by_the_widget_against_its_anchor() {
    let mut app = util::editor_test_app();

    let anchor = app.world_mut().spawn(Node::default()).id();
    let popover_entity = app
        .world_mut()
        .spawn(popover(
            PopoverProps::new(anchor).with_placement(PopoverPlacement::RightStart),
        ))
        .id();
    app.update();
    app.update();

    let placement = app
        .world()
        .get::<Popover>(popover_entity)
        .expect("the popover is placed by the widget");
    let first = placement
        .positions
        .first()
        .expect("the placement asked for is offered first");
    assert_eq!(first.side, PopoverSide::Right, "the side asked for");
    assert_eq!(first.align, PopoverAlign::Start, "and the alignment");
    assert_eq!(
        placement.positions.get(1).map(|position| position.side),
        Some(PopoverSide::Left),
        "with the mirror offered as the fallback",
    );

    assert_eq!(
        app.world()
            .get::<ChildOf>(popover_entity)
            .map(ChildOf::parent),
        Some(anchor),
        "and it hangs off the anchor it points at",
    );
}

/// A popover asked for at a free position gets a node of its own there
/// to point at, since the widget places against a rectangle.
#[test]
fn a_popover_asked_for_at_a_position_is_anchored_there() {
    let mut app = util::editor_test_app();

    let anchor = app.world_mut().spawn(Node::default()).id();
    let popover_entity = app
        .world_mut()
        .spawn(popover(
            PopoverProps::new(anchor).with_position(Vec2::new(120.0, 48.0)),
        ))
        .id();
    app.update();
    app.update();

    let point = app
        .world()
        .get::<ChildOf>(popover_entity)
        .expect("the popover hangs off something")
        .parent();
    assert_ne!(point, anchor, "not the anchor, which has its own place");
    let node = app
        .world()
        .get::<Node>(point)
        .expect("the point is laid out");
    assert_eq!((node.left, node.top), (px(120.0), px(48.0)));
}
