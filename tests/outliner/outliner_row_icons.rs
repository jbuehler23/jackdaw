//! An outliner row's glyph says what kind of thing its entity is.
//!
//! The resolver is an ordered list of rules over an entity's components
//! and, for a container, their values, and the same icons feed the Add
//! menu, so a kind looks the same wherever it is named.

use crate::util;

use bevy::prelude::*;
use jackdaw::hierarchy::HierarchyTreeContainer;
use jackdaw::ui_palette::instantiate_widget_under;
use jackdaw_api_internal::entity_icons::registered_icon;
use jackdaw_api_internal::widgets::WidgetRegistry;
use jackdaw_feathers::icons::Icon;
use jackdaw_scene_types::UiSceneRoot;

fn palette_app() -> App {
    let mut app = util::editor_test_app();
    jackdaw_api_internal::lifecycle::enable_extension(app.world_mut(), "jackdaw.ui_palette");
    app.update();
    app
}

/// An open UI scene with an outliner panel showing it.
fn outliner_app() -> (App, Entity) {
    let mut app = palette_app();
    let world = app.world_mut();
    let root = world
        .spawn((Name::new("UiRoot"), UiSceneRoot::default(), Node::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, root);
    world.spawn((
        HierarchyTreeContainer,
        Node::default(),
        Visibility::Inherited,
    ));
    app.update();
    app.update();
    (app, root)
}

fn add_widget(app: &mut App, parent: Entity, definition: &str) -> Entity {
    let entity = instantiate_widget_under(app.world_mut(), definition, Some(parent))
        .unwrap_or_else(|e| panic!("{definition} must instantiate: {e:?}"));
    app.update();
    app.update();
    entity
}

fn icon_of(app: &App, entity: Entity) -> Option<char> {
    registered_icon(app.world(), entity).map(Icon::unicode)
}

/// Each kind of thing the Add menu can create is a different glyph in
/// the outliner, and it is the glyph the Add menu draws for it.
#[test]
fn every_added_widget_shows_its_own_glyph() {
    let (mut app, root) = outliner_app();

    let expected = [
        ("ui.panel", Icon::PanelTop),
        ("ui.row", Icon::Columns3),
        ("ui.column", Icon::Rows3),
        ("ui.grid", Icon::Grid3x3),
        ("ui.label", Icon::Type),
        ("ui.image", Icon::Image),
        ("ui.button", Icon::MousePointerClick),
        ("ui.checkbox", Icon::SquareCheck),
        ("ui.radio", Icon::CircleDot),
        ("ui.slider", Icon::SlidersHorizontal),
        ("ui.text_input", Icon::TextCursorInput),
        ("ui.scroll_area", Icon::ScrollText),
    ];

    for (definition, icon) in expected {
        let entity = add_widget(&mut app, root, definition);
        assert_eq!(
            icon_of(&app, entity),
            Some(icon.unicode()),
            "{definition} must show its own glyph in the outliner"
        );
    }

    // Distinct kinds, distinct glyphs: a list of aliases would pass the
    // per-kind assertions above and still tell the reader nothing.
    let mut glyphs: Vec<char> = expected.iter().map(|(_, icon)| icon.unicode()).collect();
    glyphs.sort_unstable();
    let before = glyphs.len();
    glyphs.dedup();
    assert_eq!(before, glyphs.len(), "each kind needs its own glyph");
}

/// The glyph the outliner draws for a kind is the one its widget
/// definition carries, so the Add menu and the outliner cannot drift.
#[test]
fn the_outliner_glyph_is_the_add_menus_glyph() {
    let (mut app, root) = outliner_app();

    for definition_id in [
        "ui.panel",
        "ui.row",
        "ui.column",
        "ui.grid",
        "ui.label",
        "ui.image",
        "ui.button",
        "ui.checkbox",
        "ui.radio",
        "ui.slider",
        "ui.text_input",
        "ui.scroll_area",
    ] {
        let menu_icon = app
            .world()
            .resource::<WidgetRegistry>()
            .iter()
            .find(|definition| definition.id == definition_id)
            .and_then(|definition| definition.icon)
            .unwrap_or_else(|| panic!("{definition_id} must carry an icon"));
        let entity = add_widget(&mut app, root, definition_id);
        assert_eq!(
            icon_of(&app, entity),
            Some(menu_icon.unicode()),
            "{definition_id} must look the same in the outliner as in the Add menu"
        );
    }
}

/// A container's kind is a value, not a component, so changing the value
/// has to change the glyph the row is already drawing.
#[test]
fn flipping_flex_direction_flips_the_glyph() {
    let (mut app, root) = outliner_app();
    let row = add_widget(&mut app, root, "ui.row");
    assert_eq!(icon_of(&app, row), Some(Icon::Columns3.unicode()));

    app.world_mut()
        .get_mut::<Node>(row)
        .expect("a container has a Node")
        .flex_direction = FlexDirection::Column;
    app.update();
    app.update();

    assert_eq!(
        icon_of(&app, row),
        Some(Icon::Rows3.unicode()),
        "a row turned into a column must read as a column"
    );
}

/// The row itself, not only the resolver: the glyph the outliner is
/// drawing has to change with the value.
#[test]
fn the_drawn_row_glyph_follows_the_value() {
    let (mut app, root) = outliner_app();
    let row = add_widget(&mut app, root, "ui.row");

    // row -> TreeRowContent -> TreeRowDot -> the glyph text, the path
    // the outliner writes the icon down.
    let drawn = |app: &App| -> String {
        use jackdaw_widgets::tree_view::{TreeNode, TreeRowContent, TreeRowDot};
        let world = app.world();
        let row_entity = world
            .iter_entities()
            .find(|entity| entity.get::<TreeNode>().is_some_and(|node| node.0 == row))
            .expect("the widget has an outliner row")
            .id();
        let child_with = |parent: Entity, has: &dyn Fn(Entity) -> bool| -> Option<Entity> {
            world
                .get::<Children>(parent)?
                .iter()
                .find(|&child| has(child))
        };
        let content = child_with(row_entity, &|e| world.get::<TreeRowContent>(e).is_some())
            .expect("a row has content");
        let dot = child_with(content, &|e| world.get::<TreeRowDot>(e).is_some())
            .expect("a row draws a glyph");
        let glyph = world
            .get::<Children>(dot)
            .and_then(|children| children.iter().next())
            .expect("the glyph slot holds a text");
        world
            .get::<Text>(glyph)
            .expect("the glyph slot holds a text")
            .0
            .clone()
    };

    assert_eq!(drawn(&app), Icon::Columns3.unicode().to_string());

    app.world_mut()
        .get_mut::<Node>(row)
        .expect("a container has a Node")
        .flex_direction = FlexDirection::Column;
    app.update();
    app.update();

    assert_eq!(
        drawn(&app),
        Icon::Rows3.unicode().to_string(),
        "the drawn glyph must follow the value, not only the resolver"
    );
}

/// A brush carries `Mesh3d` like every other visible thing, and is still
/// a brush: the world kinds keep their place ahead of the general ones.
#[test]
fn a_brush_still_shows_the_cuboid() {
    let mut app = palette_app();
    let brush = app
        .world_mut()
        .spawn((
            Name::new("Brush"),
            jackdaw_scene_types::Brush::default(),
            Mesh3d::default(),
            Transform::default(),
        ))
        .id();
    app.update();
    assert_eq!(
        icon_of(&app, brush),
        Some(Icon::Cuboid.unicode()),
        "a brush must not be reduced to a mesh"
    );
}

/// The 3D kinds an author places from the Add menu each read as
/// themselves rather than as the mesh or node underneath.
#[test]
fn the_world_kinds_each_read_as_themselves() {
    let mut app = palette_app();
    let world = app.world_mut();
    let camera = world
        .spawn((Camera3d::default(), Transform::default()))
        .id();
    let sun = world
        .spawn((DirectionalLight::default(), Transform::default()))
        .id();
    let bulb = world
        .spawn((PointLight::default(), Transform::default()))
        .id();
    let spot = world
        .spawn((SpotLight::default(), Transform::default()))
        .id();
    let mesh = world.spawn((Mesh3d::default(), Transform::default())).id();
    app.update();

    for (entity, icon, what) in [
        (camera, Icon::Video, "a camera"),
        (sun, Icon::Sun, "a directional light"),
        (bulb, Icon::Lightbulb, "a point light"),
        (spot, Icon::Flashlight, "a spot light"),
        (mesh, Icon::Box, "a mesh"),
    ] {
        assert_eq!(
            icon_of(&app, entity),
            Some(icon.unicode()),
            "{what} must read as itself"
        );
    }
}

/// A scene root is what it is before it is a container, and a prefab
/// instance before it is whatever it inherits.
#[test]
fn a_scene_root_and_a_prefab_instance_win_over_what_they_are_made_of() {
    let mut app = palette_app();
    let world = app.world_mut();
    let ui_root = world.spawn((UiSceneRoot::default(), Node::default())).id();
    let scene_2d = world
        .spawn((jackdaw_scene_types::Scene2dRoot, Transform::default()))
        .id();
    let instance = world
        .spawn((
            jackdaw_prefab::components::IsA::default(),
            Mesh3d::default(),
            Transform::default(),
        ))
        .id();
    app.update();

    assert_eq!(icon_of(&app, ui_root), Some(Icon::LayoutTemplate.unicode()));
    assert_eq!(icon_of(&app, scene_2d), Some(Icon::Frame.unicode()));
    assert_eq!(
        icon_of(&app, instance),
        Some(Icon::Component.unicode()),
        "a prefab instance is a prefab instance before it is a mesh"
    );
}
