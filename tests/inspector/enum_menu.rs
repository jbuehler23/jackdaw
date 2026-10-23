//! Changing an enum field rebuilds its menu inside the card.

use bevy::feathers::controls::{FeathersMenu, FeathersMenuItem};
use bevy::prelude::*;
use bevy::ui_widgets::Activate;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;

use crate::util;

#[derive(Component, Reflect, Clone, PartialEq, Debug)]
#[reflect(Component, Default)]
enum Shape {
    Ball { radius: f32 },
    Hull,
}

impl Default for Shape {
    fn default() -> Self {
        Self::Ball { radius: 1.0 }
    }
}

/// Stands in for the collider a physics engine builds from the shape.
#[derive(Component)]
struct Collider;

/// Rebuilds the collider whenever the shape is written, the way a physics
/// engine's hook does, so the entity changes archetype in the same frame as
/// the variant.
fn rebuild_collider(
    insert: On<Insert<Shape>>,
    colliders: Query<(), With<Collider>>,
    mut commands: Commands,
) {
    let entity = insert.event_target();
    if colliders.contains(entity) {
        commands.entity(entity).remove::<Collider>();
    } else {
        commands.entity(entity).insert(Collider);
    }
}

fn settle(app: &mut App) {
    for _ in 0..6 {
        app.update();
    }
}

fn orphaned_menus(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<(), (With<FeathersMenu>, Without<ChildOf>)>()
        .iter(app.world())
        .count()
}

/// An inspector showing a selected entity whose shape the card offers as a
/// variant menu.
fn inspected_shape() -> (App, Entity) {
    let mut app = util::editor_test_app();
    app.register_type::<Shape>().add_observer(rebuild_collider);
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    let entity = app
        .world_mut()
        .spawn((Name::new("shape"), Transform::default(), Shape::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.world_mut()
        .resource_scope(|world, mut selection: Mut<Selection>| {
            let mut commands = world.commands();
            selection.select_single(&mut commands, entity);
        });
    app.world_mut().flush();
    settle(&mut app);
    assert!(
        app.world_mut()
            .query_filtered::<(), With<FeathersMenu>>()
            .iter(app.world())
            .next()
            .is_some(),
        "the card shows the variant menu",
    );
    (app, entity)
}

/// The menu item captioned `caption`.
fn menu_item(app: &mut App, caption: &str) -> Entity {
    let items: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<FeathersMenuItem>>()
        .iter(app.world())
        .collect();
    let mut texts = app.world_mut().query::<(&Text, &ChildOf)>();
    texts
        .iter(app.world())
        .find_map(|(text, child_of)| {
            (text.0 == caption && items.contains(&child_of.parent())).then_some(child_of.parent())
        })
        .unwrap_or_else(|| panic!("the menu offers {caption}"))
}

#[test]
fn setting_a_variant_that_rebuilds_the_card_leaves_no_menu_at_the_window_root() {
    let (mut app, entity) = inspected_shape();

    let result = app
        .world_mut()
        .operator("component.set")
        .param("entity", entity)
        .param("type_path", "inspector::enum_menu::Shape".to_string())
        .param("field", String::new())
        .param("value", "\"Hull\"".to_string())
        .call()
        .expect("component.set dispatches");
    assert_eq!(result, OperatorResult::Finished);
    settle(&mut app);

    assert_eq!(app.world().get::<Shape>(entity), Some(&Shape::Hull));
    assert_eq!(
        orphaned_menus(&mut app),
        0,
        "a menu was left at the window root"
    );
}

#[test]
fn picking_a_variant_that_rebuilds_the_card_leaves_no_menu_at_the_window_root() {
    let (mut app, entity) = inspected_shape();

    let item = menu_item(&mut app, "Hull");
    app.world_mut().trigger(Activate { entity: item });
    settle(&mut app);

    assert_eq!(app.world().get::<Shape>(entity), Some(&Shape::Hull));
    assert_eq!(
        orphaned_menus(&mut app),
        0,
        "a menu was left at the window root"
    );
}
