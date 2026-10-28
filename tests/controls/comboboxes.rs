//! Comboboxes.
//!
//! A combobox is a `FeathersSelect`: its trigger is the select's menu
//! button and its list is the select's popup of `FeathersListRow`s. The
//! pointer and keys are driven through the window's own event streams, so
//! the open, pick and dismiss paths are the ones a user takes.

use crate::util;
use crate::util::OperatorResultExt as _;

use std::sync::{Arc, Mutex};

use bevy::feathers::controls::{FeathersListRow, FeathersMenuButton, FeathersSelect, OptionIndex};
use bevy::prelude::*;
use bevy::ui::{Selected, UiGlobalTransform};
use bevy::ui_widgets::ListItem;
use bevy::window::{PrimaryWindow, WindowResolution};

use jackdaw::test_input::SyntheticInput;
use jackdaw_feathers::combobox::{
    ComboBoxChangeEvent, ComboBoxPopover, ComboBoxTrigger, combobox_with_selected,
};

type Picks = Arc<Mutex<Vec<(Entity, usize)>>>;

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

fn run(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    for _ in 0..300 {
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

fn click(app: &mut App, entity: Entity) {
    let at = centre_of(app, entity);
    run(
        app,
        &format!("input.pointer x={} y={} action=click", at.x, at.y),
    );
}

fn key(app: &mut App, name: &str) {
    run(app, &format!("input.key key={name}"));
}

/// Where `entity` is drawn, in the window logical pixels `input.pointer`
/// takes.
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

/// A laid-out combobox showing `selected`, and the change events it sends.
fn combobox_app(selected: usize) -> (App, Entity, Picks) {
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
    // A panel filling the window, as the editor's layout does, so a press
    // away from the list lands on something.
    let panel = app
        .world_mut()
        .spawn(Node {
            width: percent(100),
            height: percent(100),
            ..default()
        })
        .id();
    let column = app
        .world_mut()
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(200),
                top: px(200),
                width: px(240),
                ..default()
            },
            ChildOf(panel),
        ))
        .id();
    let combobox = app
        .world_mut()
        .spawn((
            combobox_with_selected(vec!["px", "%", "auto"], selected),
            ChildOf(column),
        ))
        .id();

    let picks = Picks::default();
    let sink = picks.clone();
    app.world_mut()
        .add_observer(move |change: On<ComboBoxChangeEvent>| {
            sink.lock()
                .expect("no other thread holds it")
                .push((change.entity, change.selected));
        });
    settle(&mut app);
    (app, combobox, picks)
}

fn trigger(app: &mut App, combobox: Entity) -> Entity {
    app.world_mut()
        .query_filtered::<(Entity, &ComboBoxTrigger), With<FeathersMenuButton>>()
        .iter(app.world())
        .find(|(_, trigger)| trigger.0 == combobox)
        .map(|(entity, _)| entity)
        .expect("the trigger is the select's menu button")
}

fn list_is_open(app: &mut App, combobox: Entity) -> bool {
    let popup = app
        .world_mut()
        .query::<(Entity, &ComboBoxPopover)>()
        .iter(app.world())
        .find(|(_, popover)| popover.0 == combobox)
        .map(|(entity, _)| entity)
        .expect("the combobox has its list");
    app.world().get::<Visibility>(popup) == Some(&Visibility::Visible)
}

fn is_inside(app: &App, entity: Entity, ancestor: Entity) -> bool {
    let mut current = entity;
    while let Some(parent) = app.world().get::<ChildOf>(current) {
        if parent.parent() == ancestor {
            return true;
        }
        current = parent.parent();
    }
    false
}

/// The list's rows, in option order.
fn rows(app: &mut App, combobox: Entity) -> Vec<Entity> {
    let mut rows: Vec<(usize, Entity)> = app
        .world_mut()
        .query_filtered::<(Entity, &OptionIndex), With<FeathersListRow>>()
        .iter(app.world())
        .filter(|(row, _)| is_inside(app, *row, combobox))
        .map(|(row, index)| (index.0, row))
        .collect();
    rows.sort();
    rows.into_iter().map(|(_, row)| row).collect()
}

/// The text the trigger draws.
fn caption(app: &mut App, combobox: Entity) -> String {
    let mut stack = vec![trigger(app, combobox)];
    while let Some(entity) = stack.pop() {
        if let Some(text) = app.world().get::<Text>(entity) {
            return text.0.clone();
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter().rev());
        }
    }
    panic!("the trigger draws a caption");
}

fn open_by_clicking(app: &mut App, combobox: Entity) {
    let trigger = trigger(app, combobox);
    click(app, trigger);
    assert!(
        list_is_open(app, combobox),
        "a click on the trigger opened the list"
    );
}

/// The combobox is the feathers select, its rows are list rows, and the
/// option it shows is the selected row.
#[test]
fn a_combobox_is_a_feathers_select_of_list_rows() {
    let (mut app, combobox, _) = combobox_app(1);

    assert!(app.world().get::<FeathersSelect>(combobox).is_some());
    let rows = rows(&mut app, combobox);
    assert_eq!(rows.len(), 3, "one row per option");
    for row in &rows {
        assert!(
            app.world().get::<ListItem>(*row).is_some(),
            "each row is an item the list widget knows",
        );
    }
    let selected: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| app.world().get::<Selected>(**row).is_some())
        .map(|(index, _)| index)
        .collect();
    assert_eq!(selected, vec![1], "only the shown option's row is selected");
    assert_eq!(caption(&mut app, combobox), "%");
    assert!(!list_is_open(&mut app, combobox), "the list starts closed");

    let mut sizes: Vec<FontSize> = Vec::new();
    let mut stack = vec![combobox];
    while let Some(entity) = stack.pop() {
        if app.world().get::<Text>(entity).is_some() {
            sizes.push(
                app.world()
                    .get::<TextFont>(entity)
                    .expect("text has a font")
                    .font_size,
            );
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter());
        }
    }
    assert_eq!(sizes.len(), 4, "the caption and three row labels");
    assert!(
        sizes
            .iter()
            .all(|size| *size == jackdaw_feathers::tokens::TEXT_SIZE),
        "the dropdown reads at the editor's text size: {sizes:?}",
    );
}

/// A click on the trigger opens the list, and a click on a row picks that
/// option, closes the list and shows the option on the trigger.
#[test]
fn clicking_a_row_picks_its_option() {
    let (mut app, combobox, picks) = combobox_app(0);

    open_by_clicking(&mut app, combobox);
    let row = rows(&mut app, combobox)[2];
    click(&mut app, row);

    assert_eq!(
        picks.lock().expect("no other thread holds it").as_slice(),
        [(combobox, 2)],
        "the row that was clicked is the option that was picked",
    );
    assert!(
        !list_is_open(&mut app, combobox),
        "the list closed behind it"
    );
    assert_eq!(caption(&mut app, combobox), "auto");
}

/// Clicking the option already shown closes the list without a change.
#[test]
fn clicking_the_shown_option_changes_nothing() {
    let (mut app, combobox, picks) = combobox_app(1);

    open_by_clicking(&mut app, combobox);
    let row = rows(&mut app, combobox)[1];
    click(&mut app, row);

    assert!(picks.lock().expect("no other thread holds it").is_empty());
    assert!(!list_is_open(&mut app, combobox));
}

/// A press away from the list closes it and picks nothing.
#[test]
fn a_press_outside_the_list_closes_it() {
    let (mut app, combobox, picks) = combobox_app(0);

    open_by_clicking(&mut app, combobox);
    run(&mut app, "input.pointer x=60 y=60 action=click");

    assert!(
        !list_is_open(&mut app, combobox),
        "the press outside closed it"
    );
    assert!(picks.lock().expect("no other thread holds it").is_empty());
}

/// The arrow keys walk the open list and Enter picks the row they reach.
#[test]
fn the_keyboard_walks_and_picks_in_the_open_list() {
    let (mut app, combobox, picks) = combobox_app(0);

    open_by_clicking(&mut app, combobox);
    key(&mut app, "ArrowDown");
    key(&mut app, "ArrowDown");
    key(&mut app, "Enter");

    assert_eq!(
        picks.lock().expect("no other thread holds it").as_slice(),
        [(combobox, 2)],
    );
    assert!(!list_is_open(&mut app, combobox));
    assert_eq!(caption(&mut app, combobox), "auto");
}
