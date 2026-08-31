//! Comboboxes.
//!
//! The list a combobox opens is a `FeathersMenuPopup` of
//! `FeathersMenuItem` rows placed against the trigger; the trigger
//! itself is the editor's button, which is a feathers button.

use bevy::feathers::controls::{FeathersMenuItem, FeathersMenuPopup};
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{Activate, MenuItem};

use jackdaw_feathers::button::ButtonClickEvent;
use jackdaw_feathers::combobox::{ComboBoxChangeEvent, ComboBoxTrigger, combobox_with_selected};

mod util;

fn descendants(world: &mut World, root: Entity) -> Vec<Entity> {
    let mut stack = vec![root];
    let mut out = Vec::new();
    while let Some(entity) = stack.pop() {
        out.push(entity);
        if let Some(children) = world.get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    out
}

/// A combobox with its list open, and the rows it opened.
fn open_list(selected: usize) -> (App, Entity, Entity, Vec<Entity>) {
    let mut app = util::editor_test_app();
    let combobox = app
        .world_mut()
        .spawn(combobox_with_selected(vec!["px", "%", "auto"], selected))
        .id();
    app.update();
    app.update();

    let trigger = app
        .world_mut()
        .query_filtered::<Entity, With<ComboBoxTrigger>>()
        .single(app.world())
        .expect("the combobox has one trigger");
    app.world_mut()
        .trigger(ButtonClickEvent { entity: trigger });
    app.update();
    app.update();

    let popup = app
        .world_mut()
        .query_filtered::<Entity, With<FeathersMenuPopup>>()
        .single(app.world())
        .expect("the list is the only popup");
    let rows: Vec<Entity> = app
        .world()
        .get::<Children>(popup)
        .expect("the list has rows")
        .iter()
        .filter(|entity| app.world().get::<FeathersMenuItem>(*entity).is_some())
        .collect();
    (app, combobox, popup, rows)
}

/// The list and its rows are the feathers menu widgets, and the list
/// hangs off the trigger it was opened from.
#[test]
fn a_combobox_opens_a_feathers_menu_popup_of_menu_items() {
    let (app, _, popup, rows) = open_list(0);

    assert_eq!(rows.len(), 3, "one row per option");
    for row in &rows {
        assert!(
            app.world().get::<MenuItem>(*row).is_some(),
            "each row is a menu item the widget knows",
        );
        assert!(
            app.world().get::<Interaction>(*row).is_none(),
            "and not a hand-rolled interaction control",
        );
    }
    let anchor = app
        .world()
        .get::<ChildOf>(popup)
        .expect("the list hangs off something")
        .parent();
    assert!(
        app.world().get::<ComboBoxTrigger>(anchor).is_some(),
        "the list is placed against the trigger it was opened from",
    );
}

/// The row for the option the combobox is showing carries a ticked box.
#[test]
fn the_picked_option_is_the_row_with_a_ticked_box() {
    let (mut app, _, _, rows) = open_list(1);

    let ticked: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            descendants(app.world_mut(), **row)
                .into_iter()
                .any(|entity| app.world().get::<Checked>(entity).is_some())
        })
        .map(|(index, _)| index)
        .collect();
    assert_eq!(ticked, vec![1], "only the picked row is ticked");
}

/// Activating a row picks that option and closes the list.
#[test]
fn activating_a_row_picks_its_option() {
    let (mut app, combobox, popup, rows) = open_list(0);

    let picked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(Entity, usize)>::new()));
    let seen = picked.clone();
    app.world_mut()
        .add_observer(move |change: On<ComboBoxChangeEvent>| {
            seen.lock()
                .expect("no other thread holds it")
                .push((change.entity, change.selected));
        });

    app.world_mut().trigger(Activate { entity: rows[2] });
    app.update();
    app.update();

    assert_eq!(
        picked.lock().expect("no other thread holds it").as_slice(),
        [(combobox, 2)],
        "the row that was activated is the option that was picked",
    );
    assert!(
        app.world().get_entity(popup).is_err(),
        "and the list closed behind it",
    );
}
