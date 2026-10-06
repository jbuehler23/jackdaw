//! The viewport toolbar's settings menu, driven with the pointer: it opens
//! from its button, its rows change the viewport settings, and a row that
//! picks or flips a setting leaves the menu up showing the new state.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::{
    prelude::*,
    ui::UiGlobalTransform,
    window::{PrimaryWindow, WindowResolution},
};
use jackdaw::lod_bar::LodColorView;
use jackdaw::test_input::SyntheticInput;
use jackdaw::view_modes::ViewModeSettings;
use jackdaw::viewport_settings::{QualityPreset, ViewportSettings, ViewportSettingsFile};
use jackdaw::viewport_settings_menu::VIEWPORT_SETTINGS_MENU;
use jackdaw_feathers::menu_bar::{MenuCheckedRow, MenuRadioRow, SubmenuDropdown, SubmenuRow};
use jackdaw_widgets::menu_bar::{MenuBarDropdown, MenuBarDropdownItem, MenuBarItem, MenuBarState};

const WINDOW: (u32, u32) = (1280, 720);

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

fn play(app: &mut App) {
    for _ in 0..300 {
        app.update();
        if app.world().resource::<SyntheticInput>().is_idle() {
            break;
        }
    }
    settle(app);
}

fn run(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    play(app);
}

/// An editor with one viewport panel filling a 1280x720 window, keeping its
/// viewport settings in `dir`.
fn app_with_panel(dir: &std::path::Path) -> App {
    app_with_panel_in(dir, WINDOW)
}

fn app_with_panel_in(dir: &std::path::Path, size: (u32, u32)) -> App {
    let mut app = util::editor_test_app();
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows
            .single_mut(app.world_mut())
            .expect("headless apps still have a primary window");
        window.resolution = WindowResolution::new(size.0, size.1);
    }
    app.world_mut().resource_mut::<ViewportSettingsFile>().path = Some(dir.join("viewport.json"));
    let root = app
        .world_mut()
        .spawn(Node {
            width: percent(100),
            height: percent(100),
            ..default()
        })
        .id();
    jackdaw::viewport::build_viewport_panel(app.world_mut(), root);
    settle(&mut app);
    app
}

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

fn click(app: &mut App, at: Vec2) {
    run(
        app,
        &format!("input.pointer x={} y={} action=click", at.x, at.y),
    );
}

fn settings_button(app: &mut App) -> Entity {
    app.world_mut()
        .query::<(Entity, &MenuBarItem)>()
        .iter(app.world())
        .find(|(_, item)| item.label == VIEWPORT_SETTINGS_MENU)
        .map(|(entity, _)| entity)
        .expect("the toolbar carries the viewport settings menu")
}

fn open_settings(app: &mut App) {
    let button = settings_button(app);
    let at = centre_of(app, button);
    click(app, at);
    assert!(menu_is_open(app), "a click on the button opens the menu");
}

/// Rest the pointer on the group row labelled `label` until its submenu
/// opens. The dwell is a wall-clock timer, so the frames are paced.
fn expand(app: &mut App, label: &str) {
    let row = app
        .world_mut()
        .query::<(Entity, &SubmenuRow)>()
        .iter(app.world())
        .find(|(_, row)| row.label == label)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("the menu has a {label:?} group"));
    let at = centre_of(app, row);
    run(
        app,
        &format!("input.pointer x={} y={} action=move", at.x, at.y),
    );
    for _ in 0..200 {
        let open = app
            .world_mut()
            .query::<&SubmenuDropdown>()
            .iter(app.world())
            .any(|dropdown| dropdown.row == row);
        if open {
            settle(app);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        app.update();
    }
    panic!("resting on {label:?} did not open its submenu");
}

fn menu_is_open(app: &App) -> bool {
    app.world().resource::<MenuBarState>().open_menu.is_some()
}

fn row_for(app: &mut App, action: &str) -> Entity {
    app.world_mut()
        .query::<(Entity, &MenuBarDropdownItem)>()
        .iter(app.world())
        .find(|(_, row)| row.action == action)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("the open menu has a row for {action:?}"))
}

fn click_row(app: &mut App, action: &str) {
    let row = row_for(app, action);
    let at = centre_of(app, row);
    click(app, at);
}

fn selected(app: &mut App, action: &str) -> bool {
    let row = row_for(app, action);
    app.world()
        .get::<MenuRadioRow>(row)
        .expect("a radio row")
        .selected
}

fn checked(app: &mut App, action: &str) -> bool {
    let row = row_for(app, action);
    app.world()
        .get::<MenuCheckedRow>(row)
        .expect("a checked row")
        .checked
}

fn settings(app: &App) -> &ViewportSettings {
    app.world().resource::<ViewportSettings>()
}

const LOW: &str = "op:viewport.quality.preset?preset=low";
const HIGH: &str = "op:viewport.quality.preset?preset=high";

#[test]
fn a_preset_picked_in_the_quality_menu_holds_and_the_menu_shows_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_panel(dir.path());
    open_settings(&mut app);
    expand(&mut app, "Quality");
    assert!(selected(&mut app, HIGH), "the defaults are the High preset");

    click_row(&mut app, LOW);

    assert_eq!(settings(&app).preset(), Some(QualityPreset::Low));
    assert!(menu_is_open(&app), "picking a preset leaves the menu up");
    assert!(
        selected(&mut app, LOW),
        "the open submenu shows the new pick"
    );
    assert!(!selected(&mut app, HIGH), "and only that one");
    assert!(
        selected(&mut app, "op:viewport.quality.set?render_scale=50"),
        "the choices the preset made show too"
    );
}

#[test]
fn changing_one_quality_choice_shows_the_preset_as_custom() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_panel(dir.path());
    open_settings(&mut app);
    expand(&mut app, "Quality");

    click_row(&mut app, "op:viewport.quality.set?anti_aliasing=taa");

    assert_eq!(settings(&app).preset(), None);
    let custom = app
        .world_mut()
        .query::<(&MenuBarDropdownItem, &MenuRadioRow)>()
        .iter(app.world())
        .any(|(row, mark)| row.action.is_empty() && mark.selected);
    assert!(custom, "a Custom choice stands in for the preset");
    assert!(!selected(&mut app, HIGH));
}

#[test]
fn a_show_row_flips_its_flag_and_stays_open() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_panel(dir.path());
    open_settings(&mut app);
    expand(&mut app, "Show");

    click_row(&mut app, "op:viewport.show.toggle?flag=fog&on=false");

    assert!(!settings(&app).quality.fog, "the fog flag is off");
    assert!(menu_is_open(&app));
    assert!(
        !checked(&mut app, "op:viewport.show.toggle?flag=fog&on=true"),
        "the row now offers to turn it back on, unticked"
    );

    click_row(&mut app, "op:view.toggle_grid");
    let overlays = app
        .world()
        .resource::<jackdaw::viewport_overlays::OverlaySettings>();
    assert!(!overlays.show_grid, "an editor overlay flips the same way");
}

#[test]
fn the_view_mode_rows_pick_one_mode_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_panel(dir.path());
    open_settings(&mut app);
    expand(&mut app, "View Mode");

    click_row(&mut app, "op:view.mode?mode=wireframe");
    assert_eq!(
        app.world().resource::<ViewModeSettings>().shading,
        jackdaw::view_modes::Shading::Wireframe
    );
    assert!(selected(&mut app, "op:view.mode?mode=wireframe"));

    click_row(&mut app, "op:view.mode?mode=lod_colors");
    assert_eq!(
        app.world().resource::<ViewModeSettings>().shading,
        jackdaw::view_modes::Shading::Lit
    );
    assert!(app.world().resource::<LodColorView>().0);
    assert!(selected(&mut app, "op:view.mode?mode=lod_colors"));
    assert!(!selected(&mut app, "op:view.mode?mode=wireframe"));

    click_row(&mut app, "op:view.mode?mode=lit");
    assert!(!app.world().resource::<LodColorView>().0);
    assert!(menu_is_open(&app));
}

#[test]
fn realtime_and_stats_flip_from_the_menus_own_rows() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_panel(dir.path());
    open_settings(&mut app);

    click_row(&mut app, "op:viewport.realtime.toggle?on=false");
    click_row(&mut app, "op:viewport.stats.toggle?on=true");

    assert!(!settings(&app).realtime);
    assert!(settings(&app).stats);
    assert!(checked(&mut app, "op:viewport.stats.toggle?on=false"));
}

/// A point in the viewport, well clear of the toolbar and its menus.
const OUTSIDE: Vec2 = Vec2::new(300.0, 600.0);

#[test]
fn a_click_outside_closes_the_menu_with_realtime_off() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_panel(dir.path());
    run(&mut app, "viewport.realtime.toggle on=false");
    open_settings(&mut app);

    click(&mut app, OUTSIDE);

    assert!(!menu_is_open(&app), "the click outside closed the menu");
}

#[test]
fn a_click_outside_closes_the_menu_after_one_of_its_rows_flipped_a_box() {
    for realtime in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let mut app = app_with_panel(dir.path());
        run(&mut app, &format!("viewport.realtime.toggle on={realtime}"));
        open_settings(&mut app);
        click_row(&mut app, "op:viewport.stats.toggle?on=true");
        assert!(menu_is_open(&app), "the box row leaves the menu up");

        click(&mut app, OUTSIDE);

        assert!(
            !menu_is_open(&app),
            "the first click outside closes it, realtime {realtime}"
        );
    }
}

#[test]
fn the_frame_time_graph_row_appears_once_stats_are_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_panel(dir.path());
    open_settings(&mut app);
    let graph_row = |app: &mut App| {
        app.world_mut()
            .query::<&MenuBarDropdownItem>()
            .iter(app.world())
            .any(|row| row.action.starts_with("op:viewport.stats.graph.toggle"))
    };
    assert!(!graph_row(&mut app), "no graph row while the stats are off");

    click_row(&mut app, "op:viewport.stats.toggle?on=true");
    click_row(&mut app, "op:viewport.stats.graph.toggle?on=true");

    assert!(settings(&app).frame_graph);
    assert!(checked(&mut app, "op:viewport.stats.graph.toggle?on=false"));
}

#[test]
fn the_menu_opened_from_the_toolbars_right_end_stays_inside_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_panel(dir.path());
    open_settings(&mut app);
    expand(&mut app, "Quality");

    let mut dropdowns = app
        .world_mut()
        .query_filtered::<(&ComputedNode, &UiGlobalTransform), With<MenuBarDropdown>>();
    let dropdowns: Vec<_> = dropdowns
        .iter(app.world())
        .map(|(node, transform)| {
            let scale = node.inverse_scale_factor();
            let centre = transform.translation * scale;
            let half = node.size() * scale / 2.0;
            (centre - half, centre + half)
        })
        .collect();
    assert_eq!(dropdowns.len(), 2, "the menu and its Quality submenu");
    for (min, max) in dropdowns {
        assert!(
            min.x >= 0.0 && max.x <= WINDOW.0 as f32 + 0.5,
            "a dropdown runs past the window: {min} to {max}"
        );
    }
}

#[test]
fn a_narrow_viewport_cuts_the_tools_and_keeps_the_settings_and_mode_controls() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app_with_panel_in(dir.path(), (520, 600));
    let toolbar = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::layout::Toolbar>>()
        .single(app.world())
        .expect("one toolbar");
    let right_edge = |app: &App, entity: Entity| {
        let node = app.world().get::<ComputedNode>(entity).expect("laid out");
        let transform = app
            .world()
            .get::<UiGlobalTransform>(entity)
            .expect("placed");
        (transform.translation.x + node.size().x / 2.0) * node.inverse_scale_factor()
    };
    let bar_end = right_edge(&app, toolbar);
    let button = settings_button(&mut app);
    let last = *app
        .world()
        .get::<Children>(toolbar)
        .expect("the toolbar holds its controls")
        .last()
        .expect("a last control");
    assert!(right_edge(&app, button) <= bar_end + 0.5);
    assert!(
        right_edge(&app, last) <= bar_end + 0.5,
        "the 3D and 2D switch stays inside the toolbar"
    );
}
