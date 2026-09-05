//! Where a dropped outliner row is allowed to land.
//!
//! A drop is a reparent, and some drops are not reparents at all: a row
//! dropped on itself, on one of its own descendants, or on the parent it
//! already has. The last of those still reached the reparent command,
//! which removed the child from a parent and added it back inside one
//! entry, leaving the panel to rebuild a row that had not moved.

use crate::util;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw_widgets::tree_view::TreeRowDropped;

/// One parent holding one child, both known to the document.
fn parent_and_child(app: &mut App) -> (Entity, Entity) {
    let world = app.world_mut();
    let parent = world.spawn((Name::new("Parent"), Node::default())).id();
    jackdaw::scene_io::register_entity_in_ast(world, parent);
    let child = world
        .spawn((Name::new("Child"), Node::default(), ChildOf(parent)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, child);
    app.update();
    (parent, child)
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn drop_row_on(app: &mut App, dragged: Entity, target: Entity) {
    app.world_mut().trigger(TreeRowDropped {
        entity: dragged,
        dragged_source: dragged,
        target_source: target,
    });
    app.update();
    app.update();
}

#[test]
fn a_row_dropped_on_the_parent_it_already_has_stays_put() {
    let mut app = util::editor_test_app();
    let (parent, child) = parent_and_child(&mut app);
    let entries = history_len(&app);

    drop_row_on(&mut app, child, parent);

    assert_eq!(
        app.world().get::<ChildOf>(child).map(|c| c.0),
        Some(parent),
        "the child kept its parent"
    );
    assert_eq!(
        history_len(&app),
        entries,
        "a drop that moves nothing records no undo entry"
    );
}

#[test]
fn a_row_dropped_on_itself_stays_put() {
    let mut app = util::editor_test_app();
    let (parent, child) = parent_and_child(&mut app);
    let entries = history_len(&app);

    drop_row_on(&mut app, child, child);

    assert_eq!(
        app.world().get::<ChildOf>(child).map(|c| c.0),
        Some(parent),
        "the child kept its parent"
    );
    assert_eq!(history_len(&app), entries);
}

/// The drop that does move something still moves it, so the guards
/// above are not standing in front of the whole gesture.
#[test]
fn a_row_dropped_on_another_parent_moves_under_it() {
    let mut app = util::editor_test_app();
    let (parent, child) = parent_and_child(&mut app);
    let other = {
        let world = app.world_mut();
        let other = world.spawn((Name::new("Other"), Node::default())).id();
        jackdaw::scene_io::register_entity_in_ast(world, other);
        other
    };
    app.update();

    drop_row_on(&mut app, child, other);

    assert_eq!(
        app.world().get::<ChildOf>(child).map(|c| c.0),
        Some(other),
        "the child moved under the drop target"
    );
    assert_ne!(parent, other);
}
