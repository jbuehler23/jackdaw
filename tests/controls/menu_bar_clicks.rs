//! Clicking a menu row, with the mouse.
//!
//! A dropdown row is a `FeathersButton`, activating on the release a frame or
//! more after the press, so the close pass leaves the menu standing for a press
//! on one of its own rows. The pointer is driven through the window's own event
//! streams, because triggering `Activate` by hand bypasses that path.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::{
    prelude::*,
    ui::UiGlobalTransform,
    window::{PrimaryWindow, WindowResolution},
};
use jackdaw::{test_input::SyntheticInput, view_modes::ViewModeSettings};
use jackdaw_feathers::menu_bar::{checked_row, menu_bar_shell, populate_menu_bar, submenu_row};
use jackdaw_widgets::menu_bar::{MenuBarDropdownItem, MenuBarState};

/// A parameterless operator whose whole effect is one bool, so an
/// assertion is about the row having been clicked and not about what the
/// operator does.
const TOGGLE_WIREFRAME: &str = "view.toggle_wireframe";
const TOGGLE_ROW: &str = "op:view.toggle_wireframe";

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

/// Advance until the queued gesture has been delivered, plus a settle.
fn play(app: &mut App) {
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

fn run(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    play(app);
}

/// An editor with a window big enough to hold a bar and a dropdown under
/// it.
fn menu_app() -> App {
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
    app
}

/// A menu bar carrying `menus`, laid out and ready to be clicked.
fn bar_with(app: &mut App, menus: Vec<(String, Vec<(String, String)>)>) -> Entity {
    let bar = app.world_mut().spawn(menu_bar_shell()).id();
    populate_menu_bar(app.world_mut(), bar, menus);
    settle(app);
    bar
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

/// The menu-bar item labelled `label`.
fn item_labelled(app: &mut App, label: &str) -> Entity {
    app.world_mut()
        .query::<(Entity, &jackdaw_widgets::menu_bar::MenuBarItem)>()
        .iter(app.world())
        .find(|(_, item)| item.label == label)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("the bar carries an item labelled {label:?}"))
}

/// The open dropdown's row dispatching `action`.
fn row_for(app: &mut App, action: &str) -> Entity {
    app.world_mut()
        .query::<(Entity, &MenuBarDropdownItem)>()
        .iter(app.world())
        .find(|(_, row)| row.action == action)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("the open menu has a row for {action:?}"))
}

fn click(app: &mut App, at: Vec2) {
    run(
        app,
        &format!("input.pointer x={} y={} action=click", at.x, at.y),
    );
}

fn menu_is_open(app: &App) -> bool {
    app.world().resource::<MenuBarState>().open_menu.is_some()
}

fn wireframe(app: &App) -> bool {
    app.world().resource::<ViewModeSettings>().wireframe
}

/// Open `label`'s menu by clicking it, the way a user does.
fn open_by_clicking(app: &mut App, label: &str) {
    let item = item_labelled(app, label);
    let at = centre_of(app, item);
    click(app, at);
    assert!(menu_is_open(app), "the click on {label:?} opened its menu");
}

/// The whole chain: a click on the bar opens the menu, a click on a row
/// runs the operator the row names, and the menu goes down behind it.
#[test]
fn a_click_on_a_plain_row_runs_its_operator_and_closes_the_menu() {
    let mut app = menu_app();
    bar_with(
        &mut app,
        vec![(
            "View".to_string(),
            vec![(TOGGLE_ROW.to_string(), "Toggle Wireframe".to_string())],
        )],
    );

    let before = wireframe(&app);
    open_by_clicking(&mut app, "View");

    let row = row_for(&mut app, TOGGLE_ROW);
    let at = centre_of(&app, row);
    click(&mut app, at);

    assert_eq!(
        wireframe(&app),
        !before,
        "the click reached the row and the row ran its operator",
    );
    assert!(!menu_is_open(&app), "and the menu is done");
}

/// A row that only flips a box is a setting, and settings are read and
/// changed in runs, so the menu stays up.
#[test]
fn a_click_on_a_checked_row_leaves_the_menu_open() {
    let mut app = menu_app();
    let before = wireframe(&app);
    bar_with(
        &mut app,
        vec![(
            "View".to_string(),
            vec![checked_row(before, TOGGLE_ROW, "Wireframe")],
        )],
    );

    open_by_clicking(&mut app, "View");
    let row = row_for(&mut app, TOGGLE_ROW);
    let at = centre_of(&app, row);
    click(&mut app, at);

    assert_eq!(wireframe(&app), !before, "the box was flipped");
    assert!(
        menu_is_open(&app),
        "and the menu stayed up to show the new state",
    );
}

/// A press that lands outside the bar and its dropdown closes the menu,
/// and runs nothing.
#[test]
fn a_press_outside_the_menu_closes_it() {
    let mut app = menu_app();
    bar_with(
        &mut app,
        vec![(
            "View".to_string(),
            vec![(TOGGLE_ROW.to_string(), "Toggle Wireframe".to_string())],
        )],
    );

    let before = wireframe(&app);
    open_by_clicking(&mut app, "View");

    // Far from the bar, which sits at the top left.
    run(&mut app, "input.pointer x=1400 y=900 action=press");
    assert!(!menu_is_open(&app), "the press outside took the menu down");
    run(&mut app, "input.pointer x=1400 y=900 action=release");
    assert_eq!(wireframe(&app), before, "and it ran nothing");
}

/// A row inside an expanded group is clicked the same way. The dwell
/// that expands a group is a wall-clock timer, so the group is expanded
/// through `menu.hover` -- the click on the row it reveals is the real
/// one.
#[test]
fn a_click_on_a_submenu_row_runs_its_operator() {
    let mut app = menu_app();
    let rows = submenu_row(
        "Rendering",
        vec![(TOGGLE_ROW.to_string(), "Toggle Wireframe".to_string())],
    );
    bar_with(&mut app, vec![("View".to_string(), rows)]);

    let before = wireframe(&app);
    open_by_clicking(&mut app, "View");
    run(&mut app, "menu.hover name=Rendering");

    let row = row_for(&mut app, TOGGLE_ROW);
    let at = centre_of(&app, row);
    click(&mut app, at);

    assert_eq!(
        wireframe(&app),
        !before,
        "the click on the group's row ran its operator",
    );
    assert!(!menu_is_open(&app), "and took the whole menu down");
}

/// A panel header's own menu button is a menu-bar item standing on its
/// own, so it opens and its rows are clicked exactly as the top bar's
/// are. This is the Snap menu.
#[test]
fn a_click_on_a_panel_header_menu_row_runs_its_operator() {
    use std::sync::Arc;

    let mut app = menu_app();
    app.world_mut()
        .spawn(jackdaw_feathers::menu_bar::menu_button(
            "Snap",
            jackdaw_feathers::icons::Icon::Magnet,
            Arc::new(|_: &World| vec![(TOGGLE_ROW.to_string(), "Toggle Wireframe".to_string())]),
        ));
    settle(&mut app);

    let before = wireframe(&app);
    open_by_clicking(&mut app, "Snap");

    let row = row_for(&mut app, TOGGLE_ROW);
    let at = centre_of(&app, row);
    click(&mut app, at);

    assert_eq!(
        wireframe(&app),
        !before,
        "a header menu's row answers a click like any other",
    );
    assert!(!menu_is_open(&app), "and the menu is done");
}

/// The operator dispatched is the row's own, so a bar with two rows
/// answers whichever one the pointer is over.
#[test]
fn the_row_under_the_pointer_is_the_one_that_runs() {
    let mut app = menu_app();
    bar_with(
        &mut app,
        vec![(
            "View".to_string(),
            vec![
                ("op:view.toggle_x_ray".to_string(), "X-Ray".to_string()),
                (TOGGLE_ROW.to_string(), "Toggle Wireframe".to_string()),
            ],
        )],
    );

    let before = wireframe(&app);
    let x_ray_before = app.world().resource::<ViewModeSettings>().x_ray;
    open_by_clicking(&mut app, "View");

    let row = row_for(&mut app, TOGGLE_ROW);
    let at = centre_of(&app, row);
    click(&mut app, at);

    assert_eq!(wireframe(&app), !before, "the second row ran");
    assert_eq!(
        app.world().resource::<ViewModeSettings>().x_ray,
        x_ray_before,
        "and the first one did not",
    );
    let _ = TOGGLE_WIREFRAME;
}
