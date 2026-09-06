//! Context menus.
//!
//! A menu opened at the cursor is a `FeathersMenuPopup` of
//! `FeathersMenuItem` rows: the frame, the row painting and the
//! activation are the widget's, and the editor supplies the actions.

use crate::util;

use bevy::ecs::world::CommandQueue;
use bevy::feathers::controls::{FeathersMenuItem, FeathersMenuPopup};
use bevy::prelude::*;
use bevy::ui_widgets::{Activate, MenuItem, MenuPopup};

use jackdaw_feathers::context_menu::spawn_context_menu;
use jackdaw_widgets::context_menu::ContextMenuAction;

fn open_menu(app: &mut App, items: &[(&str, &str)]) -> (Entity, Vec<Entity>) {
    let mut queue = CommandQueue::default();
    let mut commands = Commands::new(&mut queue, app.world());
    spawn_context_menu(&mut commands, Vec2::new(40.0, 60.0), None, items);
    queue.apply(app.world_mut());
    app.update();
    app.update();

    let menu = app
        .world_mut()
        .query_filtered::<Entity, With<FeathersMenuPopup>>()
        .single(app.world())
        .expect("the menu is the only popup");
    let rows = app
        .world()
        .get::<Children>(menu)
        .expect("the menu has rows")
        .iter()
        .collect::<Vec<_>>();
    (menu, rows)
}

/// The menu and its rows are the feathers menu widgets.
#[test]
fn a_context_menu_is_a_feathers_menu_popup_of_menu_items() {
    let mut app = util::editor_test_app();
    let (menu, rows) = open_menu(&mut app, &[("rename", "Rename"), ("delete", "Delete")]);

    assert!(
        app.world().get::<MenuPopup>(menu).is_some(),
        "the menu is the widget's popup",
    );
    assert_eq!(rows.len(), 2, "one row per item");
    for row in rows {
        assert!(
            app.world().get::<FeathersMenuItem>(row).is_some(),
            "each row is a feathers menu item",
        );
        assert!(
            app.world().get::<MenuItem>(row).is_some(),
            "so the widget knows it as a menu item",
        );
    }
}

/// The menu is placed where it was opened.
#[test]
fn a_context_menu_stands_where_it_was_opened() {
    let mut app = util::editor_test_app();
    let (menu, _) = open_menu(&mut app, &[("rename", "Rename")]);

    let node = app.world().get::<Node>(menu).expect("the menu is laid out");
    assert_eq!((node.left, node.top), (px(40.0), px(60.0)));
}

/// Activating a row dispatches its action.
#[test]
fn activating_a_row_dispatches_its_action() {
    let mut app = util::editor_test_app();
    let (_, rows) = open_menu(&mut app, &[("rename", "Rename"), ("delete", "Delete")]);

    let fired = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen = fired.clone();
    app.world_mut()
        .add_observer(move |action: On<ContextMenuAction>| {
            seen.lock()
                .expect("no other thread holds it")
                .push(action.action.clone());
        });

    app.world_mut().trigger(Activate { entity: rows[1] });
    app.update();

    assert_eq!(
        fired.lock().expect("no other thread holds it").as_slice(),
        ["delete".to_string()],
        "the row that was activated is the action that fired",
    );
}
