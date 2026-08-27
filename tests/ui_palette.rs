//! Widget creation: the registry feeds the Add menu's UI Widgets section,
//! and one row turns a definition into an authored entity in the open UI
//! scene.
//!
//! Four contracts are pinned here:
//!
//! 1. Every registered definition reaches the Add menu, in its `UI Widgets`
//!    section, and activating one of those rows creates the widget.
//! 2. Creating a widget parents it inside the open UI scene, registers the
//!    whole subtree in the document parent-first (so a save nests it), selects
//!    the new root, and lands as exactly one undo entry.
//! 3. A document with no UI scene refuses the request instead of spawning a
//!    stray node somewhere the user cannot see.
//! 4. A widget is one outliner row, not one row per entity it is built from.
//!    The button is the one widget with a child, and that child is authored
//!    too: its caption is an editable node with its own row, not a generated
//!    part.

use bevy::input_focus::tab_navigation::TabGroup;
use bevy::prelude::*;
use jackdaw::add_entity_picker::{
    UI_SECTION_PREFIX, WIDGET_ACTION_PREFIX, add_menu_rows, collect_add_menu_items,
};
use jackdaw::commands::CommandHistory;
use jackdaw::hierarchy::HierarchyTreeContainer;
use jackdaw::selection::Selection;
use jackdaw::ui_palette::{
    PaletteError, instantiate_widget, register_authored_subtree, seed_ui_scene_root,
};
use jackdaw_feathers::menu_bar::{
    SECTION_ACTION_PREFIX, SEPARATOR_ACTION, SUBMENU_ACTION_PREFIX, SUBMENU_END_ACTION,
};
use jackdaw_scene_types::UiSceneRoot;

mod util;

/// The widget vocabulary ships as its own extension, which an on-disk
/// extension config may leave off. Force it on so the test exercises the
/// shipped vocabulary rather than whatever the local config enables.
fn palette_app() -> App {
    let mut app = util::editor_test_app();
    jackdaw_api_internal::lifecycle::enable_extension(app.world_mut(), "jackdaw.ui_palette");
    app.update();
    app
}

/// An open UI scene: one unparented `UiSceneRoot`, registered in the
/// document the way a load or an Add would leave it.
fn open_ui_scene(world: &mut World) -> Entity {
    let root = world
        .spawn((Name::new("UiRoot"), UiSceneRoot::default(), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, root);
    root
}

fn ast_holds(world: &World, entity: Entity) -> bool {
    world
        .resource::<jackdaw_bsn::SceneBsnAst>()
        .ast_for(entity)
        .is_some()
}

#[test]
fn the_add_menu_lists_every_built_in_widget() {
    let mut app = palette_app();
    let items = collect_add_menu_items(app.world_mut());

    let widget_items: Vec<(String, String, String)> = items
        .iter()
        .filter(|item| item.action.starts_with("widget:"))
        .map(|item| {
            (
                item.action.clone(),
                item.label.clone(),
                item.category.name.clone().unwrap_or_default(),
            )
        })
        .collect();

    for expected in [
        "widget:ui.panel",
        "widget:ui.row",
        "widget:ui.column",
        "widget:ui.grid",
        "widget:ui.label",
        "widget:ui.image",
        "widget:ui.button",
        "widget:ui.checkbox",
        "widget:ui.radio",
        "widget:ui.toggle",
        "widget:ui.slider",
        "widget:ui.text_input",
        "widget:ui.scroll_area",
    ] {
        assert!(
            widget_items.iter().any(|(action, ..)| action == expected),
            "{expected} missing from the Add menu; got {widget_items:?}",
        );
    }

    let button = widget_items
        .iter()
        .find(|(action, ..)| action == "widget:ui.button")
        .expect("the button definition reaches the Add menu");
    assert_eq!(button.1, "Button");
    assert_eq!(button.2, format!("{UI_SECTION_PREFIX}Controls"));
}

/// The rows one group of the Add menu expands into: everything between
/// its opener and the closer that matches it.
fn group_rows(rows: &[(String, String)], group: &str) -> Vec<(String, String)> {
    let opener = format!("{SUBMENU_ACTION_PREFIX}{group}");
    let start = rows
        .iter()
        .position(|(action, _)| *action == opener)
        .unwrap_or_else(|| panic!("the Add menu has a `{group}` group; got {rows:?}"))
        + 1;
    let mut depth = 1;
    let mut inside = Vec::new();
    for row in &rows[start..] {
        if row.0.starts_with(SUBMENU_ACTION_PREFIX) {
            depth += 1;
        } else if row.0 == SUBMENU_END_ACTION {
            depth -= 1;
            if depth == 0 {
                return inside;
            }
        }
        inside.push(row.clone());
    }
    panic!("the `{group}` group is never closed; got {rows:?}");
}

/// The rows under one section label, up to the next label or divider.
fn section_rows(rows: &[(String, String)], section: &str) -> Vec<(String, String)> {
    let label = format!("{SECTION_ACTION_PREFIX}{section}");
    let start = rows
        .iter()
        .position(|(action, _)| *action == label)
        .unwrap_or_else(|| panic!("there is a `{section}` section; got {rows:?}"))
        + 1;
    rows[start..]
        .iter()
        .take_while(|(action, _)| {
            !action.starts_with(SECTION_ACTION_PREFIX) && action != SEPARATOR_ACTION
        })
        .cloned()
        .collect()
}

/// Every widget row of the Add menu's UI group, in menu order.
fn ui_group(rows: &[(String, String)]) -> Vec<(String, String)> {
    group_rows(rows, "UI")
        .into_iter()
        .filter(|(action, _)| {
            !action.starts_with(SECTION_ACTION_PREFIX) && action != SEPARATOR_ACTION
        })
        .collect()
}

#[test]
fn every_registered_widget_sits_in_the_ui_group() {
    let mut app = palette_app();
    let registered: Vec<(String, String, String)> = app
        .world()
        .resource::<jackdaw_api_internal::WidgetRegistry>()
        .iter()
        .map(|definition| {
            (
                definition.id.to_string(),
                definition.name.to_string(),
                definition.category.to_string(),
            )
        })
        .collect();
    assert!(
        !registered.is_empty(),
        "the built-in definitions are registered",
    );

    let rows = add_menu_rows(app.world_mut());
    let union = ui_group(&rows);
    let inside = group_rows(&rows, "UI");

    for (id, name, category) in &registered {
        let action = format!("{WIDGET_ACTION_PREFIX}{id}");
        assert!(
            union.contains(&(action.clone(), name.clone())),
            "{action} missing from the Add menu's UI group; got {union:?}",
        );
        assert!(
            section_rows(&inside, category).contains(&(action.clone(), name.clone())),
            "{action} is not in the group's own `{category}` section; got {inside:?}",
        );
    }
    assert_eq!(
        union.len(),
        registered.len(),
        "the UI group holds the registry and nothing else: {union:?}",
    );
}

/// The menu opens on the entries themselves, groups everything else
/// behind a row that expands, and never leaves a divider stranded.
#[test]
fn the_add_menu_opens_on_entries_and_groups_the_rest() {
    let mut app = palette_app();
    let rows = add_menu_rows(app.world_mut());

    assert_eq!(
        rows.first().map(|(_, label)| label.clone()),
        Some(String::from("Empty")),
        "the general entries are in the menu itself: {rows:?}",
    );
    assert!(
        rows.iter()
            .any(|(action, _)| action.starts_with(SUBMENU_ACTION_PREFIX)),
        "the rest are behind groups that expand: {rows:?}",
    );

    let mut depth = 0i32;
    for (action, label) in &rows {
        if let Some(group) = action.strip_prefix(SUBMENU_ACTION_PREFIX) {
            depth += 1;
            assert!(
                !group.is_empty(),
                "a group that expands says what it holds: {rows:?}",
            );
            continue;
        }
        if action == SUBMENU_END_ACTION {
            depth -= 1;
            assert!(depth >= 0, "a group is closed once: {rows:?}");
            continue;
        }
        if action.starts_with(SECTION_ACTION_PREFIX) || action == SEPARATOR_ACTION {
            continue;
        }
        assert!(!label.is_empty(), "every action row is labelled: {rows:?}");
    }
    assert_eq!(depth, 0, "every group is closed: {rows:?}");

    for pair in rows.windows(2) {
        assert!(
            !(pair[0].0 == SEPARATOR_ACTION && pair[1].0 == SEPARATOR_ACTION),
            "two dividers never touch: {rows:?}",
        );
    }
    assert_ne!(
        rows.last().map(|(action, _)| action.clone()),
        Some(String::from(SEPARATOR_ACTION)),
        "the menu does not end on a divider: {rows:?}",
    );
}

/// A group of one entry says what the entry is; a group worth expanding
/// keeps its own name.
#[test]
fn a_group_of_one_stands_in_for_its_entry() {
    let mut app = palette_app();
    let rows = add_menu_rows(app.world_mut());
    assert!(
        rows.iter().any(|(_, label)| label == "Terrain"),
        "the one region kind is a row of its own: {rows:?}",
    );
    assert!(
        !rows
            .iter()
            .any(|(action, _)| *action == format!("{SUBMENU_ACTION_PREFIX}Regions")),
        "and it is not hidden behind a group holding only itself: {rows:?}",
    );
    assert!(
        group_rows(&rows, "Lights")
            .iter()
            .any(|(_, label)| label == "Point Light"),
        "a group with more than one entry keeps its name: {rows:?}",
    );
}

#[test]
fn activating_a_ui_group_row_creates_the_widget_undoably() {
    let mut app = palette_app();
    let root = open_ui_scene(app.world_mut());
    let rows = add_menu_rows(app.world_mut());
    let (action, _) = ui_group(&rows)
        .into_iter()
        .find(|(action, _)| action.ends_with("ui.button"))
        .expect("the button row is in the UI group");

    app.world_mut()
        .trigger(jackdaw_widgets::menu_bar::MenuAction { action });
    app.update();

    let world = app.world_mut();
    let button = world
        .query_filtered::<Entity, bevy::prelude::With<bevy::ui_widgets::Button>>()
        .iter(world)
        .find(|entity| !world.entity(*entity).contains::<jackdaw::EditorEntity>())
        .expect("the menu row spawned a button");
    assert_eq!(
        world.get::<ChildOf>(button).map(ChildOf::parent),
        Some(root),
        "the row parents the widget the way the command always has",
    );
    assert!(ast_holds(world, button), "the widget joins the document");
    assert_eq!(
        world.resource::<CommandHistory>().undo_stack.len(),
        1,
        "one menu activation is one undo entry",
    );

    let mut history = world.remove_resource::<CommandHistory>().unwrap();
    history.undo(world);
    world.insert_resource(history);
    assert!(
        world.get_entity(button).is_err(),
        "undo takes the menu-created widget back",
    );
}

#[test]
fn creating_a_widget_authors_it_under_the_open_ui_scene() {
    let mut app = palette_app();
    let world = app.world_mut();
    let root = open_ui_scene(world);

    let button = instantiate_widget(world, "ui.button").expect("the UI scene accepts a button");

    assert_eq!(
        world.get::<ChildOf>(button).map(ChildOf::parent),
        Some(root),
        "a new widget belongs to the open UI scene",
    );
    assert!(ast_holds(world, button), "the widget joins the document");
    assert_eq!(
        world.resource::<Selection>().primary(),
        Some(button),
        "the new widget is what the user is now editing",
    );
    assert_eq!(
        world.resource::<CommandHistory>().undo_stack.len(),
        1,
        "one click is one undo entry",
    );
    assert!(world.get::<Name>(button).is_some(), "every widget is named");

    // The document nests the widget under the root, so a save round-trips it
    // as part of the scene rather than as a second top-level node.
    let text =
        jackdaw::scene_io::emit_bsn_scene_with_inline_assets(world, std::path::Path::new("."));
    assert!(
        text.contains("bevy_ui_widgets::button::Button"),
        "the saved document carries the widget: {text}",
    );
    let root_at = text
        .find("UiSceneRoot")
        .expect("the saved document carries the UI scene root");
    let button_at = text
        .find("bevy_ui_widgets::button::Button")
        .expect("the saved document carries the button");
    assert!(
        root_at < button_at,
        "the widget is emitted inside the root, not before it: {text}",
    );
}

#[test]
fn undoing_a_widget_removes_it_from_the_world_and_the_document() {
    let mut app = palette_app();
    let world = app.world_mut();
    open_ui_scene(world);

    let button = instantiate_widget(world, "ui.button").expect("the UI scene accepts a button");

    let mut history = world.remove_resource::<CommandHistory>().unwrap();
    history.undo(world);
    world.insert_resource(history);

    assert!(
        world.get_entity(button).is_err(),
        "undo despawns the widget it created",
    );
    assert!(
        !ast_holds(world, button),
        "undo takes the widget out of the document too",
    );
}

#[test]
fn a_selected_node_inside_the_ui_scene_is_the_new_parent() {
    let mut app = palette_app();
    let world = app.world_mut();
    open_ui_scene(world);

    let panel = instantiate_widget(world, "ui.panel").expect("the UI scene accepts a panel");
    let button = instantiate_widget(world, "ui.button").expect("the panel accepts a button");

    assert_eq!(
        world.get::<ChildOf>(button).map(ChildOf::parent),
        Some(panel),
        "a widget lands inside whatever the user has selected",
    );
}

#[test]
fn a_selection_outside_the_ui_scene_falls_back_to_the_scene_root() {
    let mut app = palette_app();
    let world = app.world_mut();
    let root = open_ui_scene(world);

    let elsewhere = world.spawn((Name::new("Cube"), Transform::default())).id();
    jackdaw::scene_io::register_entity_in_ast(world, elsewhere);
    world.resource_mut::<Selection>().entities = vec![elsewhere];

    let button = instantiate_widget(world, "ui.button").expect("the UI scene still accepts it");
    assert_eq!(
        world.get::<ChildOf>(button).map(ChildOf::parent),
        Some(root),
        "a 3D selection cannot adopt a UI node; the scene root does",
    );
}

#[test]
fn a_document_with_no_ui_scene_refuses_the_widget() {
    let mut app = palette_app();
    let world = app.world_mut();

    let before = world.entities().count_spawned();
    let result = instantiate_widget(world, "ui.button");

    assert_eq!(result, Err(PaletteError::NoUiScene));
    assert_eq!(
        world.resource::<CommandHistory>().undo_stack.len(),
        0,
        "a refused request is not an undo entry",
    );
    assert_eq!(
        world.entities().count_spawned(),
        before,
        "a refused request spawns nothing",
    );
}

#[test]
fn an_unknown_definition_is_refused_by_name() {
    let mut app = palette_app();
    let world = app.world_mut();
    open_ui_scene(world);

    assert_eq!(
        instantiate_widget(world, "ui.nope"),
        Err(PaletteError::UnknownDefinition("ui.nope".to_string())),
    );
}

#[test]
fn a_subtree_is_registered_parent_before_children() {
    let mut app = palette_app();
    let world = app.world_mut();
    let scene_root = open_ui_scene(world);

    let root = world
        .spawn((Name::new("Card"), Node::default(), ChildOf(scene_root)))
        .id();
    let child = world
        .spawn((Name::new("Header"), Node::default(), ChildOf(root)))
        .id();
    let grandchild = world
        .spawn((Name::new("Title"), Node::default(), ChildOf(child)))
        .id();

    register_authored_subtree(world, root);

    for entity in [root, child, grandchild] {
        assert!(ast_holds(world, entity), "{entity} joined the document");
    }

    // Registering a child before its parent would emit it as a second root.
    let text =
        jackdaw::scene_io::emit_bsn_scene_with_inline_assets(world, std::path::Path::new("."));
    let card_at = text.find("Card").expect("the card is emitted");
    let header_at = text.find("Header").expect("the header is emitted");
    let title_at = text.find("Title").expect("the title is emitted");
    assert!(
        card_at < header_at && header_at < title_at,
        "the document nests the subtree: {text}",
    );
}

/// Rows for children are spawned lazily, so a test that wants to see them
/// has to mark the parent row as already expanded, as `multi_outliner`
/// does.
fn mark_expanded(world: &mut World, source: Entity) {
    let mut rows = world.query::<(
        &jackdaw_widgets::tree_view::TreeNode,
        &mut jackdaw_widgets::tree_view::TreeChildrenPopulated,
    )>();
    for (node, mut populated) in rows.iter_mut(world) {
        if node.0 == source {
            populated.0 = true;
        }
    }
}

fn rows_for(world: &mut World, source: Entity) -> usize {
    world
        .query::<&jackdaw_widgets::tree_view::TreeNode>()
        .iter(world)
        .filter(|node| node.0 == source)
        .count()
}

#[test]
fn a_widget_is_one_outliner_row_and_its_internals_are_none() {
    let mut app = palette_app();
    let world = app.world_mut();
    let root = open_ui_scene(world);
    world.spawn((
        HierarchyTreeContainer,
        Node::default(),
        Visibility::Inherited,
    ));
    app.update();

    let world = app.world_mut();
    mark_expanded(world, root);
    let button = instantiate_widget(world, "ui.button").expect("the UI scene accepts a button");
    app.update();

    let world = app.world_mut();
    assert_eq!(
        rows_for(world, button),
        1,
        "a button is one outliner row in the one open outliner",
    );

    // What a widget implementation adds under an authored node at runtime:
    // a child the document has no node for.
    mark_expanded(world, button);
    let internal = world
        .spawn((Name::new("Button Label"), Node::default(), ChildOf(button)))
        .id();
    app.update();

    let world = app.world_mut();
    assert_eq!(
        rows_for(world, internal),
        0,
        "a part the document never heard of is not an outliner row",
    );
    assert_eq!(rows_for(world, button), 1, "the button is still one row",);
}

/// Plenty of authored entities are parented first and registered a frame or
/// more later: a new animation clip registers from an `Update` system after
/// `spawn_new_clip_for_selection` parents it, and a load registers its
/// entities after spawning them. Judged at the moment the row-spawn observer
/// runs, those are indistinguishable from a generated part, so the row is
/// withheld and has to arrive once the document catches up.
#[test]
fn a_child_registered_a_frame_later_still_gets_its_row() {
    let mut app = palette_app();
    let world = app.world_mut();
    let root = open_ui_scene(world);
    world.spawn((
        HierarchyTreeContainer,
        Node::default(),
        Visibility::Inherited,
    ));
    app.update();

    let world = app.world_mut();
    mark_expanded(world, root);

    // Parented now, registered later: exactly the clip and load shape.
    let late = world
        .spawn((Name::new("Clip"), Node::default(), ChildOf(root)))
        .id();
    app.update();

    let world = app.world_mut();
    assert_eq!(
        rows_for(world, late),
        0,
        "nothing to show yet: the document has never heard of it",
    );

    jackdaw::scene_io::register_entity_in_ast(world, late);
    app.update();

    let world = app.world_mut();
    assert_eq!(
        rows_for(world, late),
        1,
        "the row arrives with the document node, not never",
    );
}

/// The other half of the same rule: a part that is derived rather than merely
/// late, such as a GLTF instance's children or a terrain's chunks, never
/// registers, so it never gets a row, no matter how many times the document
/// changes around it.
#[test]
fn a_derived_child_stays_rowless_across_later_registrations() {
    let mut app = palette_app();
    let world = app.world_mut();
    let root = open_ui_scene(world);
    world.spawn((
        HierarchyTreeContainer,
        Node::default(),
        Visibility::Inherited,
    ));
    app.update();

    let world = app.world_mut();
    mark_expanded(world, root);
    let derived = world
        .spawn((Name::new("Chunk"), Node::default(), ChildOf(root)))
        .id();
    app.update();

    // Something else registers, so the retry pass runs with this child still
    // unregistered.
    let world = app.world_mut();
    let sibling = world
        .spawn((Name::new("Authored"), Node::default(), ChildOf(root)))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, sibling);
    app.update();
    app.update();

    let world = app.world_mut();
    assert_eq!(
        rows_for(world, sibling),
        1,
        "the registered sibling does get its row",
    );
    assert_eq!(
        rows_for(world, derived),
        0,
        "a derived child is not an outliner row, however often the document moves",
    );
}

// ---------------------------------------------------------------------------
// Feathers theming and value behaviour
// ---------------------------------------------------------------------------

use bevy::feathers::{
    controls::ButtonVariant,
    cursor::EntityCursor,
    focus::FocusIndicator,
    theme::{InheritableThemeTextColor, ThemeBackgroundColor, ThemeBorderColor, UiTheme},
    tokens,
};
use bevy::ui::Checked;
use bevy::ui_widgets::{Slider, SliderValue, ToggleChecked, ValueChange};

/// The feathers styling paths the save allowlist names as string literals.
/// A rename upstream turns each literal into a silent no-op, so they are
/// pinned against the real `TypePath` here.
#[test]
fn the_allowlisted_feathers_paths_are_the_real_type_paths() {
    use bevy::reflect::TypePath;

    for (literal, real) in [
        (
            "bevy_feathers::theme::ThemeBackgroundColor",
            ThemeBackgroundColor::type_path(),
        ),
        (
            "bevy_feathers::theme::ThemeBorderColor",
            ThemeBorderColor::type_path(),
        ),
        (
            "bevy_feathers::theme::ThemeTextColor",
            bevy::feathers::theme::ThemeTextColor::type_path(),
        ),
        (
            "bevy_feathers::theme::InheritableThemeTextColor",
            InheritableThemeTextColor::type_path(),
        ),
        (
            "bevy_feathers::theme::ThemedText",
            bevy::feathers::theme::ThemedText::type_path(),
        ),
        (
            "bevy_feathers::controls::button::ButtonVariant",
            ButtonVariant::type_path(),
        ),
        (
            "bevy_feathers::focus::FocusIndicator",
            FocusIndicator::type_path(),
        ),
        (
            "bevy_feathers::cursor::EntityCursor",
            EntityCursor::type_path(),
        ),
    ] {
        assert_eq!(literal, real);
        assert!(
            !jackdaw::scene_io::should_skip_component(real),
            "{real} must survive the bevy_feathers:: skip prefix",
        );
    }
}

/// A palette button is a themed feathers button, not a flat coloured box:
/// it carries the styling set `update_button_styles` keys on, plus the
/// focus and cursor treatment the deprecated `button_bundle` spawned.
#[test]
fn a_created_button_carries_the_feathers_styling_set() {
    let mut app = palette_app();
    let world = app.world_mut();
    open_ui_scene(world);

    let button = instantiate_widget(world, "ui.button").expect("the UI scene accepts a button");

    assert_eq!(
        world.get::<ButtonVariant>(button),
        Some(&ButtonVariant::Normal)
    );
    assert_eq!(
        world
            .get::<ThemeBackgroundColor>(button)
            .map(|t| t.0.to_string()),
        Some(tokens::BUTTON_BG.to_string()),
    );
    assert_eq!(
        world
            .get::<InheritableThemeTextColor>(button)
            .map(|t| t.0.to_string()),
        Some(tokens::BUTTON_TEXT.to_string()),
    );
    assert!(world.get::<FocusIndicator>(button).is_some());
    assert!(world.get::<EntityCursor>(button).is_some());

    // The caption is a child: `InheritableThemeTextColor` propagates to
    // descendants and never colours text on its own entity.
    let caption = world
        .get::<Children>(button)
        .and_then(|children| children.iter().next())
        .expect("a button has a caption child");
    assert!(
        world.get::<Text>(caption).is_some(),
        "the caption holds the label"
    );

    // The styling has to reach the document, or a reload gets a bare box.
    let text =
        jackdaw::scene_io::emit_bsn_scene_with_inline_assets(world, std::path::Path::new("."));
    for path in [
        "bevy_feathers::theme::ThemeBackgroundColor",
        "bevy_feathers::theme::InheritableThemeTextColor",
        "bevy_feathers::controls::button::ButtonVariant",
    ] {
        assert!(
            text.contains(path),
            "the saved button carries {path}: {text}"
        );
    }
}

/// Save the open document, then load it into a fresh editor.
fn round_trip(app: &mut App) -> App {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("ui.bsn");
    let text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(app.world_mut(), dir.path());
    std::fs::write(&path, &text).expect("write the scene");

    let mut reloaded = palette_app();
    jackdaw::scene_io::load_scene_from_file(reloaded.world_mut(), &path);
    reloaded.update();
    reloaded
}

fn by_name(world: &mut World, name: &str) -> Entity {
    world
        .query::<(Entity, &Name)>()
        .iter(world)
        .find(|(_, entity_name)| entity_name.as_str() == name)
        .map(|(entity, _)| entity)
        .unwrap_or_else(|| panic!("no entity named {name} after the reload"))
}

/// Theming is not a spawn-time paint job: a document that comes back from
/// disk must be re-themed by feathers' own systems.
#[test]
fn a_reloaded_button_is_re_themed() {
    let mut app = palette_app();
    open_ui_scene(app.world_mut());
    instantiate_widget(app.world_mut(), "ui.button").expect("the UI scene accepts a button");

    let mut reloaded = round_trip(&mut app);
    let button = by_name(reloaded.world_mut(), "Button");

    let expected = reloaded
        .world()
        .resource::<UiTheme>()
        .color(&tokens::BUTTON_BG);
    assert_eq!(
        reloaded
            .world()
            .get::<BackgroundColor>(button)
            .map(|bg| bg.0),
        Some(expected),
        "feathers repaints the loaded button from its theme token",
    );
}

/// A checkbox authored in the editor, saved, and loaded back still toggles.
/// Observers are not components, so the behaviour cannot ride along in the
/// document and has to be attached by a plugin.
#[test]
fn a_reloaded_checkbox_still_toggles() {
    let mut app = palette_app();
    open_ui_scene(app.world_mut());
    instantiate_widget(app.world_mut(), "ui.checkbox").expect("the UI scene accepts a checkbox");

    let mut reloaded = round_trip(&mut app);
    let checkbox = by_name(reloaded.world_mut(), "Checkbox");
    assert!(
        reloaded.world().get::<Checked>(checkbox).is_none(),
        "a fresh checkbox loads unchecked",
    );

    reloaded
        .world_mut()
        .trigger(ToggleChecked { entity: checkbox });
    reloaded.update();
    assert!(
        reloaded.world().get::<Checked>(checkbox).is_some(),
        "clicking a reloaded checkbox checks it",
    );

    reloaded
        .world_mut()
        .trigger(ToggleChecked { entity: checkbox });
    reloaded.update();
    assert!(
        reloaded.world().get::<Checked>(checkbox).is_none(),
        "and clicking again clears it",
    );
}

/// Editor chrome runs its own checkbox state machines (the Extensions dialog
/// refuses a toggle it cannot honour, the inspector mirrors a reflected
/// field). The authored-widget observers must not reach into them, and what
/// tells the two apart is a node in the scene document.
#[test]
fn the_self_update_observers_leave_editor_chrome_alone() {
    let mut app = palette_app();
    let chrome = app
        .world_mut()
        .spawn((
            jackdaw::EditorEntity,
            Node::default(),
            bevy::ui_widgets::Checkbox,
        ))
        .id();

    app.world_mut().trigger(ToggleChecked { entity: chrome });
    app.update();

    assert!(
        app.world().get::<Checked>(chrome).is_none(),
        "the editor's own checkboxes keep managing their own state",
    );
}

/// A slider that self-updates and a two-way `Value` binding write the same
/// number: the binding's equality guards mean the pair settles instead of
/// ping-ponging.
#[test]
fn a_bound_slider_settles_when_it_also_self_updates() {
    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource)]
    struct MixerSettings {
        master: f32,
    }

    /// Frames that saw `SliderValue` written. A binding and a self-update
    /// that disagree show up here as a rising count.
    #[derive(Resource, Default)]
    struct Touches(usize);

    fn count_touches(sliders: Query<(), Changed<SliderValue>>, mut touches: ResMut<Touches>) {
        touches.0 += sliders.iter().count();
    }

    let mut app = util::headless_app();
    app.add_plugins(jackdaw_bind::JackdawBindPlugin);
    app.finish();
    app.update();
    app.register_type::<MixerSettings>();
    app.init_resource::<MixerSettings>();
    app.init_resource::<Touches>();
    app.add_systems(Last, count_touches);

    let slider = app
        .world_mut()
        .spawn((
            Name::new("Volume"),
            Node::default(),
            Slider::default(),
            SliderValue(0.0),
            jackdaw_bind::Bindings(vec![jackdaw_bind::Binding::Value {
                with: jackdaw_bind::BindPath::new("Res(MixerSettings).master"),
                two_way: true,
            }]),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), slider);
    app.update();

    app.world_mut().trigger(ValueChange {
        source: slider,
        value: 0.75f32,
        is_final: true,
    });
    app.update();

    assert_eq!(
        app.world().get::<SliderValue>(slider).map(|value| value.0),
        Some(0.75),
        "the drag moves the slider itself",
    );
    assert_eq!(
        app.world().resource::<MixerSettings>().master,
        0.75,
        "and the two-way binding carries it to the source",
    );

    // Quiescence: neither side rewrites what the other already agreed on.
    app.world_mut().resource_mut::<Touches>().0 = 0;
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(
        app.world().resource::<Touches>().0,
        0,
        "the pair settles rather than fighting every frame",
    );
}

/// Feathers swaps a button's theme tokens in place for hover, pressed, and
/// disabled: `set_button_styles` writes `ThemeBackgroundColor` and
/// `InheritableThemeTextColor` on the button itself. Any path that captures a
/// button's live components into the document, such as a paste, a late
/// registration, or a re-registration after an undo, can therefore catch it
/// mid-interaction and record `feathers.button.bg.pressed` as the colour the
/// user authored. Emission normalises that back.
#[test]
fn a_pressed_button_saves_its_resting_colour() {
    let mut app = palette_app();
    open_ui_scene(app.world_mut());
    let button =
        instantiate_widget(app.world_mut(), "ui.button").expect("the UI scene accepts a button");

    // `update_button_styles` reads `Hovered`, which the plugin hydrates in
    // `Update`; let that land before the press.
    app.update();

    // What a mouse-down does: feathers rewrites both theme components to the
    // pressed tokens, in place.
    app.world_mut().entity_mut(button).insert(bevy::ui::Pressed);
    app.update();
    assert_eq!(
        app.world()
            .get::<ThemeBackgroundColor>(button)
            .map(|token| token.0.to_string()),
        Some(tokens::BUTTON_BG_PRESSED.to_string()),
        "feathers really did swap the live token; otherwise this test proves nothing",
    );

    // Re-register the button while it is held down: this is the capture step
    // a paste or a late registration performs, and it snapshots whatever the
    // live components say.
    let world = app.world_mut();
    world
        .resource_mut::<jackdaw_bsn::SceneBsnAst>()
        .remove_entity_node(button);
    jackdaw::scene_io::register_entity_in_ast(world, button);

    let text = jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        std::path::Path::new("."),
    );
    assert!(
        text.contains(&tokens::BUTTON_BG.to_string()),
        "the document records the resting colour: {text}",
    );
    assert!(
        !text.contains(&tokens::BUTTON_BG_PRESSED.to_string()),
        "and not the pressed colour: {text}",
    );

    // The live component is untouched: emission works on a clone, and the
    // button on screen is still pressed.
    assert_eq!(
        app.world()
            .get::<ThemeBackgroundColor>(button)
            .map(|token| token.0.to_string()),
        Some(tokens::BUTTON_BG_PRESSED.to_string()),
    );
}

/// The caption is authored content, not a generated part: it survives the
/// round trip with its text and its themed-text opt-in.
#[test]
fn a_reloaded_button_keeps_its_caption() {
    let mut app = palette_app();
    open_ui_scene(app.world_mut());
    instantiate_widget(app.world_mut(), "ui.button").expect("the UI scene accepts a button");

    let mut reloaded = round_trip(&mut app);
    let caption = by_name(reloaded.world_mut(), "Caption");
    assert_eq!(
        reloaded
            .world()
            .get::<Text>(caption)
            .map(|text| text.0.clone()),
        Some("Button".to_string()),
    );
    assert!(
        reloaded
            .world()
            .get::<bevy::feathers::theme::ThemedText>(caption)
            .is_some(),
        "the caption still opts in to the button's inherited text colour",
    );
}

/// A text input's text is authored state, and `bevy_text::EditableText`
/// cannot carry it: it is not `Reflect`, so the document holds `TextValue`
/// and the widget crate puts an editor back and fills it on load.
#[test]
fn a_reloaded_text_input_keeps_its_text() {
    use jackdaw_widgets_runtime::TextValue;

    let mut app = palette_app();
    open_ui_scene(app.world_mut());
    let input = instantiate_widget(app.world_mut(), "ui.text_input")
        .expect("the UI scene accepts an input");
    app.world_mut()
        .get_mut::<TextValue>(input)
        .expect("the palette's input carries a text value")
        .0 = "Ada Lovelace".to_string();
    // The save writes the document, so the edit reaches it the way every
    // editor edit does rather than living in the world alone.
    let typed = app.world().get::<TextValue>(input).cloned().unwrap();
    jackdaw::commands::sync_component_to_ast(
        app.world_mut(),
        input,
        "jackdaw_widgets_runtime::TextValue",
        &typed,
    );
    app.update();

    let mut reloaded = round_trip(&mut app);
    let loaded = by_name(reloaded.world_mut(), "Text Input");
    assert_eq!(
        reloaded
            .world()
            .get::<TextValue>(loaded)
            .map(|value| value.0.clone()),
        Some("Ada Lovelace".to_string()),
        "the document carries the text through the save",
    );
    let shown: String = reloaded
        .world()
        .get::<bevy::text::EditableText>(loaded)
        .expect("the loaded input still has an editor")
        .value()
        .into_iter()
        .collect();
    assert_eq!(
        shown, "Ada Lovelace",
        "and the loaded document puts it back in the box",
    );
}

/// The authored cursor is written as `EntityCursor::System(Pointer)`, an
/// enum's tuple variant, which is not itself a registered type. Loading it
/// takes both halves: `bevy_feathers` registers none of its own types, and the
/// loader needs a fallback for a tuple patch naming a variant.
#[test]
fn a_reloaded_button_keeps_its_cursor() {
    use bevy::window::SystemCursorIcon;

    let mut app = palette_app();
    open_ui_scene(app.world_mut());
    instantiate_widget(app.world_mut(), "ui.button").expect("the UI scene accepts a button");

    let mut reloaded = round_trip(&mut app);
    let button = by_name(reloaded.world_mut(), "Button");
    assert_eq!(
        reloaded.world().get::<EntityCursor>(button),
        Some(&EntityCursor::System(SystemCursorIcon::Pointer)),
        "the authored cursor survives the round trip",
    );
}

/// A `Field` binding that writes a marker component is authored as a path
/// with no field, which is a spelling the document has to carry exactly.
#[test]
fn a_marker_write_binding_survives_the_round_trip() {
    use jackdaw_bind::{BindPath, Binding, Bindings};

    let mut app = palette_app();
    open_ui_scene(app.world_mut());
    let button =
        instantiate_widget(app.world_mut(), "ui.button").expect("the UI scene accepts a button");
    let authored = Bindings(vec![Binding::Field {
        read: vec![BindPath::new("my_game::Form.incomplete")],
        via: None,
        write: BindPath::new("bevy_ui::interaction_states::InteractionDisabled"),
        as_percent: false,
    }]);
    app.world_mut().entity_mut(button).insert(authored.clone());
    jackdaw::commands::sync_component_to_ast(
        app.world_mut(),
        button,
        "jackdaw_bind::types::Bindings",
        &authored,
    );
    app.update();

    let mut reloaded = round_trip(&mut app);
    let loaded = by_name(reloaded.world_mut(), "Button");
    assert_eq!(
        reloaded.world().get::<Bindings>(loaded),
        Some(&authored),
        "the write path comes back spelled exactly as it was authored",
    );
}

/// Tab navigation gathers focusables from a `TabGroup` ancestor, so a
/// screen authored without one is keyboard-unreachable however many
/// buttons it holds. The root a new UI scene starts from is where that
/// group belongs, next to the reference size the 2D stage frames against.
#[test]
fn a_seeded_ui_root_carries_the_focus_group_and_the_reference_size() {
    let mut app = palette_app();

    let root = seed_ui_scene_root(app.world_mut());

    assert_eq!(
        app.world()
            .get::<TabGroup>(root)
            .map(|group| (group.order, group.modal)),
        Some((0, false)),
        "the seeded root is a non-modal tab group, so tabbing reaches the widgets under it",
    );
    assert_eq!(
        app.world()
            .get::<UiSceneRoot>(root)
            .map(|scene_root| scene_root.reference_size),
        Some(UVec2::new(1280, 720)),
        "the 2D stage frames the scene against this reference size",
    );
}

/// The group is authored state, not a session decoration: it has to be in
/// the document, which means the type has to be reflected and unskipped.
#[test]
fn a_seeded_ui_root_saves_its_focus_group() {
    let mut app = palette_app();
    seed_ui_scene_root(app.world_mut());

    let mut reloaded = round_trip(&mut app);

    let root = by_name(reloaded.world_mut(), "UiRoot");
    assert!(
        reloaded.world().get::<TabGroup>(root).is_some(),
        "the saved root comes back a tab group",
    );
    assert_eq!(
        reloaded
            .world()
            .get::<UiSceneRoot>(root)
            .map(|scene_root| scene_root.reference_size),
        Some(UVec2::new(1280, 720)),
        "and with the reference size it was authored at",
    );
}

/// A scene whose root declares no group can be hand-authored or older than
/// the seeding rule, and the first widget added to it is the moment to put
/// the group back.
#[test]
fn the_first_widget_into_a_groupless_root_backfills_the_focus_group() {
    let mut app = palette_app();
    let root = open_ui_scene(app.world_mut());
    assert!(
        app.world().get::<TabGroup>(root).is_none(),
        "the fixture is a pre-Task-4 root, with no group to find",
    );

    instantiate_widget(app.world_mut(), "ui.button").expect("the UI scene accepts a button");

    assert!(
        app.world().get::<TabGroup>(root).is_some(),
        "adding a widget gave the scene the group its keyboard navigation needs",
    );
    let mut reloaded = round_trip(&mut app);
    let loaded = by_name(reloaded.world_mut(), "UiRoot");
    assert!(
        reloaded.world().get::<TabGroup>(loaded).is_some(),
        "the backfill reached the document, not just the live world",
    );
    let button = by_name(reloaded.world_mut(), "Button");
    assert!(
        reloaded
            .world()
            .get::<bevy::input_focus::tab_navigation::TabIndex>(button)
            .is_some(),
        "and the group has something to gather: the widget's own tab index survived too",
    );
}

/// Idempotent, and it defers: a root that already declares a group keeps
/// the one it declares, order and modality intact, however many widgets
/// arrive after it.
#[test]
fn a_root_that_already_has_a_focus_group_keeps_the_one_it_has() {
    let mut app = palette_app();
    let root = open_ui_scene(app.world_mut());
    app.world_mut().entity_mut(root).insert(TabGroup {
        order: 3,
        modal: true,
    });

    instantiate_widget(app.world_mut(), "ui.button").expect("the UI scene accepts a button");
    instantiate_widget(app.world_mut(), "ui.label").expect("the UI scene accepts a label");

    assert_eq!(
        app.world()
            .get::<TabGroup>(root)
            .map(|group| (group.order, group.modal)),
        Some((3, true)),
        "the backfill adds a missing group; it never overwrites an authored one",
    );
}
