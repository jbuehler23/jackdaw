//! A prefab instance whose file the project does not hold.
//!
//! Nothing is inherited, so the instance has no name of its own and nothing
//! below it. It still has to be seen and read as broken, or the only sign a
//! reference is dangling is a scene that quietly lost a building.

use crate::util;

use bevy::prelude::*;
use jackdaw_feathers::tree_view::category_color;
use jackdaw_widgets::tree_view::{
    EntityCategory, TreeIndex, TreeRowContent, TreeRowDot, TreeRowLabel,
};

/// An outliner panel over a scene holding one instance of `source`, with an
/// empty prefab cache, which is what a missing file leaves behind.
fn panel_over_an_instance(source: &str) -> (App, Entity, Entity) {
    let mut app = util::editor_test_app();
    let panel = app
        .world_mut()
        .spawn((
            jackdaw::hierarchy::HierarchyTreeContainer,
            Node::default(),
            Visibility::Inherited,
        ))
        .id();
    app.update();

    let world = app.world_mut();
    let instance = world
        .spawn((
            Transform::default(),
            Visibility::default(),
            jackdaw_prefab::components::IsA {
                source: source.into(),
                deleted: Vec::new(),
            },
            jackdaw_prefab::components::PrefabEntityId(0),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, instance);
    for _ in 0..6 {
        app.update();
    }
    (app, panel, instance)
}

fn row_for(app: &App, source: Entity, panel: Entity) -> Option<Entity> {
    app.world().resource::<TreeIndex>().get(panel, source)
}

/// The colour the row's glyph is drawn in.
fn glyph_color(app: &App, row: Entity) -> Color {
    let world = app.world();
    let child_with = |parent: Entity, has: &dyn Fn(Entity) -> bool| -> Option<Entity> {
        world
            .get::<Children>(parent)?
            .iter()
            .find(|&child| has(child))
    };
    let content =
        child_with(row, &|e| world.get::<TreeRowContent>(e).is_some()).expect("a row has content");
    let dot = child_with(content, &|e| world.get::<TreeRowDot>(e).is_some())
        .expect("a row draws a glyph");
    let glyph = world
        .get::<Children>(dot)
        .and_then(|children| children.iter().next())
        .expect("the glyph is a text child");
    world
        .get::<TextColor>(glyph)
        .expect("the glyph is coloured")
        .0
}

#[test]
fn an_instance_with_no_prefab_keeps_a_row_marked_as_broken() {
    let (app, panel, instance) = panel_over_an_instance("prefabs/no_such_lamp.bsn");

    let row = row_for(&app, instance, panel)
        .expect("an instance that inherited no name still has a row to see");

    assert_eq!(
        glyph_color(&app, row),
        category_color(EntityCategory::MissingPrefab, false),
        "the row is drawn as broken rather than as an ordinary instance",
    );
}

/// What the row's label reads.
fn shown(app: &App, row: Entity) -> String {
    let world = app.world();
    let child_with = |parent: Entity, has: &dyn Fn(Entity) -> bool| -> Option<Entity> {
        world
            .get::<Children>(parent)?
            .iter()
            .find(|&child| has(child))
    };
    let content =
        child_with(row, &|e| world.get::<TreeRowContent>(e).is_some()).expect("a row has content");
    let label = child_with(content, &|e| world.get::<TreeRowLabel>(e).is_some())
        .expect("a row carries a label");
    world
        .get::<Text>(label)
        .expect("the label carries text")
        .0
        .clone()
}

#[test]
fn a_row_with_no_inherited_name_reads_as_the_file_it_points_at() {
    let (app, panel, instance) = panel_over_an_instance("prefabs/no_such_lamp.bsn");
    let row = row_for(&app, instance, panel).expect("a row");

    assert_eq!(
        shown(&app, row),
        "no_such_lamp",
        "the row names the file rather than the entity number",
    );
}

/// An instance of a missing prefab under a named group, which is where a scene
/// usually puts one.
fn panel_over_a_group_holding_an_instance(source: &str) -> (App, Entity, Entity, Entity) {
    let mut app = util::editor_test_app();
    let panel = app
        .world_mut()
        .spawn((
            jackdaw::hierarchy::HierarchyTreeContainer,
            Node::default(),
            Visibility::Inherited,
        ))
        .id();
    app.update();

    let world = app.world_mut();
    let group = world
        .spawn((
            Name::new("Group"),
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, group);
    let instance = world
        .spawn((
            Transform::default(),
            Visibility::default(),
            jackdaw_prefab::components::IsA {
                source: source.into(),
                deleted: Vec::new(),
            },
            jackdaw_prefab::components::PrefabEntityId(0),
            ChildOf(group),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, instance);
    for _ in 0..6 {
        app.update();
    }
    (app, panel, group, instance)
}

#[test]
fn an_instance_under_a_group_keeps_its_row_too() {
    let (mut app, panel, group, instance) =
        panel_over_a_group_holding_an_instance("prefabs/no_such_lamp.bsn");

    let group_row = row_for(&app, group, panel).expect("the group has a row");
    app.world_mut()
        .entity_mut(group_row)
        .insert(jackdaw_widgets::tree_view::TreeNodeExpanded(true));
    for _ in 0..4 {
        app.update();
    }

    let row = row_for(&app, instance, panel)
        .expect("opening the group shows the instance that inherited no name");
    assert_eq!(shown(&app, row), "no_such_lamp");
}
