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

/// The glyph the outliner is actually drawing for `source`.
///
/// row -> `TreeRowContent` -> `TreeRowDot` -> the glyph text, the path the
/// outliner writes the icon down. Asserting here rather than on the
/// resolver is what says the row caught up, which is a separate thing from
/// the rule being right.
fn drawn(app: &App, source: Entity) -> String {
    use jackdaw_widgets::tree_view::{TreeNode, TreeRowContent, TreeRowDot};
    let world = app.world();
    let row_entity = world
        .iter_entities()
        .find(|entity| {
            entity
                .get::<TreeNode>()
                .is_some_and(|node| node.0 == source)
        })
        .expect("the entity has an outliner row")
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
        ("ui.spacer", Icon::Space),
        ("ui.separator", Icon::SeparatorHorizontal),
        ("ui.progress", Icon::Gauge),
        ("ui.label", Icon::Type),
        ("ui.image", Icon::Image),
        ("ui.button", Icon::MousePointerClick),
        ("ui.checkbox", Icon::SquareCheck),
        ("ui.toggle", Icon::ToggleLeft),
        ("ui.radio", Icon::CircleDot),
        ("ui.slider", Icon::SlidersHorizontal),
        ("ui.text_input", Icon::TextCursorInput),
        ("ui.scroll_area", Icon::ScrollText),
        ("ui.dropdown", Icon::SquareChevronDown),
        ("ui.radio_group", Icon::ListChecks),
        ("ui.tabs", Icon::PanelsTopLeft),
        ("ui.nine_patch", Icon::Grid2x2),
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
        "ui.spacer",
        "ui.separator",
        "ui.progress",
        "ui.label",
        "ui.image",
        "ui.button",
        "ui.checkbox",
        "ui.toggle",
        "ui.radio",
        "ui.slider",
        "ui.text_input",
        "ui.scroll_area",
        "ui.dropdown",
        "ui.radio_group",
        "ui.tabs",
        "ui.nine_patch",
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

    assert_eq!(drawn(&app, row), Icon::Columns3.unicode().to_string());

    app.world_mut()
        .get_mut::<Node>(row)
        .expect("a container has a Node")
        .flex_direction = FlexDirection::Column;
    app.update();
    app.update();

    assert_eq!(
        drawn(&app, row),
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

/// A model instance and a scene root each name themselves. Without a rule
/// of their own both fell back to the plain-entity dot, which is what a
/// scene of hundreds of models looked like: a column of identical dots.
#[test]
fn a_model_instance_and_a_scene_root_read_as_themselves() {
    let mut app = palette_app();
    let world = app.world_mut();
    let instance = world
        .spawn((
            jackdaw_scene_types::GltfSource {
                path: "models/dungeon.glb".into(),
                scene_index: 0,
            },
            Transform::default(),
        ))
        .id();
    let scene_root = world
        .spawn((jackdaw_scene_types::SceneRootTag, Transform::default()))
        .id();
    app.update();

    assert_eq!(
        icon_of(&app, instance),
        Some(Icon::Boxes.unicode()),
        "a model instance is several parts held under one instance, and not \
         the single box an authored mesh gets"
    );
    assert_eq!(
        icon_of(&app, scene_root),
        Some(Icon::Clapperboard.unicode())
    );
}

/// A row is built when `Name` lands, which for a loaded scene is before the
/// component saying what the entity is. The glyph catches up, and so does
/// the colour: a model whose row was drawn as a plain entity kept the grey
/// dot long after its glyph had become a model's.
#[test]
fn a_model_row_catches_up_with_its_kind_in_glyph_and_colour() {
    use bevy::prelude::TextColor;
    use jackdaw_feathers::tree_view::category_color;
    use jackdaw_widgets::tree_view::{EntityCategory, TreeNode, TreeRowContent, TreeRowDot};

    let (mut app, _root) = outliner_app();
    let source = app
        .world_mut()
        .spawn((Name::new("CommonTree_1"), Transform::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), source);
    app.update();
    app.update();

    app.world_mut()
        .entity_mut(source)
        .insert(jackdaw_scene_types::GltfSource {
            path: "models/dungeon.glb".into(),
            scene_index: 0,
        });
    app.update();
    app.update();

    assert_eq!(drawn(&app, source), Icon::Boxes.unicode().to_string());

    let world = app.world();
    let row = world
        .iter_entities()
        .find(|entity| {
            entity
                .get::<TreeNode>()
                .is_some_and(|node| node.0 == source)
        })
        .expect("the entity has an outliner row")
        .id();
    let child_with = |parent: Entity, has: &dyn Fn(Entity) -> bool| -> Option<Entity> {
        world
            .get::<Children>(parent)?
            .iter()
            .find(|&child| has(child))
    };
    let content = child_with(row, &|e| world.get::<TreeRowContent>(e).is_some())
        .expect("the row has content");
    let dot =
        child_with(content, &|e| world.get::<TreeRowDot>(e).is_some()).expect("the row has a dot");
    let glyph = world
        .get::<Children>(dot)
        .and_then(|children| children.iter().next())
        .expect("the dot holds a text");
    assert_eq!(
        world
            .get::<TextColor>(glyph)
            .expect("the glyph is coloured")
            .0,
        category_color(EntityCategory::Scene, false),
        "the colour caught up with the kind as well as the glyph"
    );
}

/// A widget's glyph reaches the row it is added to, not only the resolver:
/// the row is spawned by one observer and the glyph written by another, and
/// the two only agree if the second one runs.
#[test]
fn an_added_widget_draws_its_glyph_in_the_row() {
    let (mut app, root) = outliner_app();
    let button = add_widget(&mut app, root, "ui.button");
    let label = add_widget(&mut app, root, "ui.label");

    let menu_icon = |app: &App, id: &str| {
        app.world()
            .resource::<WidgetRegistry>()
            .get(id)
            .and_then(|definition| definition.icon)
            .unwrap_or_else(|| panic!("{id} has a glyph"))
            .unicode()
            .to_string()
    };
    assert_eq!(drawn(&app, button), menu_icon(&app, "ui.button"));
    assert_eq!(drawn(&app, label), menu_icon(&app, "ui.label"));
}

/// A row is spawned when `Transform` lands, and the document applies a
/// patch one component at a time, so a streamed or pasted terrain's kind
/// arrives after its row. Without an observer for it the row keeps the
/// fallback dot for the rest of the session.
#[test]
fn a_terrain_gets_its_glyph_when_the_kind_lands_after_the_row() {
    let (mut app, _root) = outliner_app();
    let source = app
        .world_mut()
        .spawn((Name::new("Terrain"), Transform::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), source);
    app.update();
    app.update();
    assert_ne!(
        drawn(&app, source),
        Icon::Mountain.unicode().to_string(),
        "nothing has said it is a terrain yet",
    );

    app.world_mut()
        .entity_mut(source)
        .insert(jackdaw_scene_types::Terrain::default());
    app.update();
    app.update();

    assert_eq!(
        drawn(&app, source),
        Icon::Mountain.unicode().to_string(),
        "the row caught up with the kind that landed after it",
    );
}

/// Every `Node` is a container of some kind, so the rule saying so has to
/// stand behind the rules that name kinds, including the ones an extension
/// loaded after the outliner registers. It used to answer first, which made
/// every such rule unreachable.
#[test]
fn an_extension_rule_on_a_node_is_reachable_past_the_container_fallback() {
    let mut app = palette_app();
    let spawn_point = app
        .world_mut()
        .spawn((
            Name::new("Spawn"),
            jackdaw_multiplayer::SpawnPoint::default(),
            Node::default(),
        ))
        .id();
    app.update();

    assert_eq!(
        icon_of(&app, spawn_point),
        Some(Icon::MapPin.unicode()),
        "the extension's rule must be reachable on an entity the fallback also matches",
    );
}
