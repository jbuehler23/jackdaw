//! The 2D canvas's snapping preferences: the defaults a new project
//! opens on, the operators that change them, where they are kept, and
//! what they must stay out of.

use bevy::prelude::*;
use jackdaw::canvas_snap::{CanvasSnap, CanvasSnapKind};
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::project_settings::{Section, load_section, settings_path};
use jackdaw_api::prelude::*;

mod util;

use util::OperatorResultExt as _;

fn snap(app: &App) -> CanvasSnap {
    *app.world().resource::<CanvasSnap>()
}

#[test]
fn the_snap_kinds_ship_with_the_defaults_the_canvas_expects() {
    let app = util::editor_test_app();
    let snap = snap(&app);

    let on = CanvasSnapKind::ALL
        .into_iter()
        .filter(|kind| snap.offers(*kind))
        .map(CanvasSnapKind::id)
        .collect::<Vec<_>>();
    assert_eq!(
        on,
        vec![
            "enabled",
            "pixel",
            "parent",
            "percent_lines",
            "sibling_sides",
            "sibling_centers",
            "guides",
        ],
        "the canvas snaps out of the box, and every kind but the cross-tree one is on",
    );
    assert!(
        snap.show_rulers && snap.show_guides,
        "the rulers and the guides are drawn out of the box",
    );
    assert_eq!(
        CanvasSnapKind::ALL
            .into_iter()
            .filter_map(|kind| CanvasSnapKind::parse(kind.id()))
            .count(),
        CanvasSnapKind::ALL.len(),
        "every kind is reachable by the id a caller writes",
    );
}

#[test]
fn the_snap_operator_sets_flips_and_refuses_an_unknown_kind() {
    let mut app = util::editor_test_app();

    app.world_mut()
        .operator("canvas.snap")
        .param("kind", "other_nodes")
        .param("on", true)
        .call()
        .expect("canvas.snap dispatches")
        .assert_finished();
    assert!(
        snap(&app).other_nodes,
        "a call naming a state puts the kind in that state",
    );

    app.world_mut()
        .operator("canvas.snap")
        .param("kind", "other_nodes")
        .call()
        .expect("canvas.snap dispatches")
        .assert_finished();
    assert!(
        !snap(&app).other_nodes,
        "a call naming no state flips the kind",
    );

    let before = snap(&app);
    app.world_mut()
        .operator("canvas.snap")
        .param("kind", "nonesuch")
        .param("on", true)
        .call()
        .expect("canvas.snap dispatches")
        .assert_cancelled();
    assert_eq!(
        snap(&app),
        before,
        "a call naming no kind of the canvas's changes nothing",
    );

    app.world_mut()
        .operator("canvas.rulers")
        .param("on", false)
        .call()
        .expect("canvas.rulers dispatches")
        .assert_finished();
    app.world_mut()
        .operator("canvas.guides")
        .call()
        .expect("canvas.guides dispatches")
        .assert_finished();
    assert!(
        !snap(&app).show_rulers && !snap(&app).show_guides,
        "the view toggles set and flip the same way",
    );
}

/// A project directory of this test's own, holding `settings`.
fn project_with_settings(name: &str, settings: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!("jackdaw-canvas-snap-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".jackdaw")).expect("the project directory");
    std::fs::write(settings_path(&root), settings).expect("the settings the project opens with");
    root
}

fn open_project(app: &mut App, root: &std::path::Path) {
    app.world_mut().insert_resource(ProjectRoot {
        root: root.to_path_buf(),
        config: ProjectConfig::default(),
    });
    app.update();
}

#[test]
fn the_snap_kinds_survive_a_project_reopen_beside_the_build_settings() {
    let root = project_with_settings("reopen", "{\n  \"auto_build\": true\n}");
    let mut app = util::editor_test_app();
    open_project(&mut app, &root);

    app.world_mut()
        .operator("canvas.snap")
        .param("kind", "other_nodes")
        .param("on", true)
        .call()
        .expect("canvas.snap dispatches")
        .assert_finished();

    let written = std::fs::read_to_string(settings_path(&root)).expect("the settings were written");
    let document: serde_json::Value =
        serde_json::from_str(&written).expect("the settings are still JSON");
    assert_eq!(
        document.get("auto_build"),
        Some(&serde_json::Value::Bool(true)),
        "the settings beside the canvas's are still there: {written}",
    );

    let reopened: CanvasSnap = load_section(&root, Section::Key("canvas"));
    assert_eq!(
        reopened,
        snap(&app),
        "a project reopened reads back what the editor last wrote",
    );
    assert!(
        reopened.other_nodes,
        "including the kind the user turned on",
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn the_snap_kinds_stay_out_of_undo_snapshots() {
    let mut app = util::editor_test_app();
    let before = util::snapshot(&mut app);

    app.world_mut()
        .operator("canvas.snap")
        .param("kind", "parent")
        .param("on", false)
        .call()
        .expect("canvas.snap dispatches")
        .assert_finished();

    let after = util::snapshot(&mut app);
    assert!(
        before.equals(&*after),
        "a snap preference is not part of the scene an undo restores",
    );

    before.apply(app.world_mut());
    assert!(
        !snap(&app).parent,
        "undoing back past the change leaves the preference where the user put it",
    );
}

#[test]
fn the_header_snap_menu_lists_every_kind_with_its_state() {
    let mut app = snap_menu_app();

    assert_eq!(
        snap_menu_rows(&mut app)
            .iter()
            .map(|(action, label)| (action.as_str(), label.as_str()))
            .collect::<Vec<_>>(),
        vec![
            ("[x]op:canvas.snap?kind=enabled", "Use Snap"),
            ("[x]op:canvas.snap?kind=pixel", "Use Pixel Snap"),
            ("---", ""),
            ("##Smart Snapping", ""),
            ("[x]op:canvas.snap?kind=parent", "Parent"),
            ("[x]op:canvas.snap?kind=percent_lines", "Percent Lines"),
            ("[x]op:canvas.snap?kind=sibling_sides", "Sibling Sides"),
            ("[x]op:canvas.snap?kind=sibling_centers", "Sibling Centers"),
            ("[ ]op:canvas.snap?kind=other_nodes", "Other Nodes"),
            ("[x]op:canvas.snap?kind=guides", "Guides"),
            ("---", ""),
            ("[x]op:canvas.rulers?on=false", "Show Rulers"),
            ("[x]op:canvas.guides?on=false", "Show Guides"),
            ("##Grid: 8 px", ""),
            ("op:viewport2d.grid?size=4", "Finer"),
            ("op:viewport2d.grid?size=16", "Coarser"),
        ],
        "the menu reads the canvas's settings, and every row calls what it shows",
    );

    set_kind(&mut app, "other_nodes", true);
    app.update();
    assert!(
        snap_menu_rows(&mut app).contains(&(
            "[x]op:canvas.snap?kind=other_nodes".to_string(),
            "Other Nodes".to_string()
        )),
        "a kind turned on shows as on the next time the menu is asked",
    );
}

#[test]
fn clicking_a_snap_row_flips_the_kind_and_leaves_the_menu_open() {
    use jackdaw_widgets::menu_bar::MenuBarState;

    let mut app = snap_menu_app();
    open_snap_menu(&mut app);

    click_row(&mut app, "op:canvas.snap?kind=pixel");
    assert!(!snap(&app).pixel, "the row flips the kind its action names");
    assert!(
        app.world().resource::<MenuBarState>().open_menu.is_some(),
        "and a row that only flips a box leaves the menu up to be read",
    );
    assert!(
        dropdown_rows(&mut app).contains(&"op:canvas.snap?kind=pixel".to_string()),
        "with its rows redrawn in place",
    );
    assert!(
        checked_rows(&mut app).contains(&(String::from("op:canvas.snap?kind=pixel"), false)),
        "so the box the click emptied shows empty: {:?}",
        checked_rows(&mut app),
    );

    // A row that is a command rather than a box is done with the menu.
    click_row(&mut app, "op:viewport2d.grid?size=4");
    assert!(
        app.world().resource::<MenuBarState>().open_menu.is_none(),
        "a plain row closes the menu behind it",
    );
}

/// Open the header's Snap menu the way the operator a scripted run uses
/// opens it.
fn open_snap_menu(app: &mut App) {
    use jackdaw_widgets::menu_bar::MenuBarState;

    app.world_mut()
        .operator("menu.open")
        .param("name", "Snap")
        .call()
        .expect("menu.open dispatches")
        .assert_finished();
    app.update();
    assert!(
        app.world().resource::<MenuBarState>().open_menu.is_some(),
        "the header's Snap menu opens like any other menu",
    );
}

/// Click the open dropdown's row whose action is `action`: the button
/// click, and the press that made it, on the one frame the editor sees
/// them on.
fn click_row(app: &mut App, action: &str) {
    use bevy::input::{ButtonState, mouse::MouseButtonInput};
    use bevy::window::PrimaryWindow;
    use jackdaw_feathers::button::ButtonClickEvent;
    use jackdaw_widgets::menu_bar::MenuBarDropdownItem;

    let row = app
        .world_mut()
        .query::<(Entity, &MenuBarDropdownItem)>()
        .iter(app.world())
        .find(|(_, item)| item.action == action)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("the open menu offers a {action} row"));
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<PrimaryWindow>>()
        .single(app.world())
        .expect("headless apps still have a primary window");
    app.world_mut().trigger(ButtonClickEvent { entity: row });
    app.world_mut().write_message(MouseButtonInput {
        button: MouseButton::Left,
        state: ButtonState::Pressed,
        window,
    });
    app.update();
    // Let the button go, so the next click is a press the editor sees
    // rather than one already held down.
    app.world_mut().write_message(MouseButtonInput {
        button: MouseButton::Left,
        state: ButtonState::Released,
        window,
    });
    for _ in 0..4 {
        app.update();
    }
}

/// The actions the open dropdown's rows carry.
fn dropdown_rows(app: &mut App) -> Vec<String> {
    app.world_mut()
        .query::<&jackdaw_widgets::menu_bar::MenuBarDropdownItem>()
        .iter(app.world())
        .map(|item| item.action.clone())
        .collect()
}

/// The open dropdown's rows that show a box, and the state each shows.
fn checked_rows(app: &mut App) -> Vec<(String, bool)> {
    app.world_mut()
        .query::<(
            &jackdaw_widgets::menu_bar::MenuBarDropdownItem,
            &jackdaw_feathers::menu_bar::MenuCheckedRow,
        )>()
        .iter(app.world())
        .map(|(item, row)| (item.action.clone(), row.checked))
        .collect()
}

/// An editor with one 2D viewport panel laid out, so its header's Snap
/// menu has built its rows.
fn snap_menu_app() -> App {
    let mut app = util::editor_test_app();
    let parent = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node {
                width: px(1200),
                height: px(600),
                ..default()
            },
        ))
        .id();
    jackdaw::viewport_2d::build_viewport_2d_panel(app.world_mut(), parent);
    for _ in 0..4 {
        app.update();
    }
    app
}

/// The rows the header's Snap menu would open with.
fn snap_menu_rows(app: &mut App) -> Vec<(String, String)> {
    app.world_mut()
        .query::<&jackdaw_widgets::menu_bar::MenuBarItem>()
        .iter(app.world())
        .find(|item| item.label == "Snap")
        .expect("the 2D viewport header carries a Snap menu")
        .actions
        .clone()
}

fn set_kind(app: &mut App, kind: &str, on: bool) {
    app.world_mut()
        .operator("canvas.snap")
        .param("kind", kind.to_string())
        .param("on", on)
        .call()
        .expect("canvas.snap dispatches")
        .assert_finished();
}
