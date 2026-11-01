//! A colour field in the generic card reads as a swatch row: its picker opens
//! on a click of the swatch, stays open across a rebuild of the inspector, and
//! ignores a right click.

use bevy::{
    feathers::controls::{ColorPlaneValue, ColorSwatchValue},
    picking::pointer::PointerButton,
    prelude::*,
};

use crate::util;

#[derive(Component, Reflect, Clone, PartialEq, Debug, Default)]
#[reflect(Component, Default)]
struct Tint {
    color: Color,
}

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn one<T: Component>(app: &mut App) -> Entity {
    let found: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<T>>()
        .iter(app.world())
        .collect();
    assert_eq!(found.len(), 1, "one {}", std::any::type_name::<T>());
    found[0]
}

fn inspected() -> (App, Entity) {
    let mut app = util::editor_test_app();
    app.register_type::<Tint>();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let entity = app
        .world_mut()
        .spawn((Name::new("tinted"), Transform::default(), Tint::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    jackdaw::selection::select_only(app.world_mut(), entity);
    settle(&mut app);
    (app, entity)
}

/// Whether some ancestor of the picker's plane, below its colour row, is
/// hidden.
fn picker_hidden(app: &mut App) -> bool {
    let swatch = one::<ColorSwatchValue>(app);
    let header = app
        .world()
        .get::<ChildOf>(swatch)
        .map(ChildOf::parent)
        .expect("the swatch sits in the row header");
    let row = app
        .world()
        .get::<ChildOf>(header)
        .map(ChildOf::parent)
        .expect("the header sits in the row");
    let mut at = one::<ColorPlaneValue>(app);
    while at != row {
        if app
            .world()
            .get::<Node>(at)
            .is_some_and(|node| node.display == Display::None)
        {
            return true;
        }
        match app.world().get::<ChildOf>(at) {
            Some(parent) => at = parent.parent(),
            None => return false,
        }
    }
    false
}

fn click_with(app: &mut App, entity: Entity, button: PointerButton) {
    let window = app
        .world_mut()
        .query_filtered::<Entity, With<bevy::window::PrimaryWindow>>()
        .single(app.world())
        .expect("a primary window");
    let target = bevy::camera::RenderTarget::Window(bevy::window::WindowRef::Primary)
        .normalize(Some(window))
        .expect("the primary window normalizes");
    app.world_mut()
        .trigger(bevy::picking::events::PointerClick {
            entity,
            pointer: bevy::picking::events::Pointer::new(
                bevy::picking::pointer::PointerId::Mouse,
                bevy::picking::pointer::Location {
                    target,
                    position: Vec2::ZERO,
                },
            ),
            button,
            hit: bevy::picking::backend::HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
            duration: core::time::Duration::ZERO,
            count: 1,
        });
    settle(app);
}

#[test]
fn a_colour_row_is_a_swatch_whose_picker_stays_open_across_a_rebuild() {
    let (mut app, entity) = inspected();
    assert!(picker_hidden(&mut app), "the picker starts closed");
    let swatch = one::<ColorSwatchValue>(&mut app);
    click_with(&mut app, swatch, PointerButton::Secondary);
    assert!(picker_hidden(&mut app), "a right click leaves it closed");
    click_with(&mut app, swatch, PointerButton::Primary);
    assert!(!picker_hidden(&mut app), "a click on the swatch opens it");

    jackdaw::selection::clear_selection_in_world(app.world_mut());
    settle(&mut app);
    jackdaw::selection::select_only(app.world_mut(), entity);
    settle(&mut app);
    assert!(!picker_hidden(&mut app), "the rebuilt row is still open");
    let swatch = one::<ColorSwatchValue>(&mut app);
    click_with(&mut app, swatch, PointerButton::Primary);
    assert!(picker_hidden(&mut app), "a second click closes it");
}

/// The label text of the generic colour row and its node.
fn label_named(app: &mut App, text: &str) -> Option<Entity> {
    app.world_mut()
        .query::<(Entity, &Text)>()
        .iter(app.world())
        .find(|(_, label)| label.0 == text)
        .map(|(entity, _)| entity)
}

#[test]
fn a_generic_colour_row_labels_itself_like_the_other_generic_rows() {
    let (mut app, _entity) = inspected();
    let label = label_named(&mut app, "color:").expect("a `color:` label");
    let node = app.world().get::<Node>(label).expect("a node");
    assert_eq!(node.width, Val::Auto, "the label is sized by its text");
}

/// A user component whose colour field has a long name.
#[derive(Component, Reflect, Clone, PartialEq, Debug, Default)]
#[reflect(Component, Default)]
struct Swatches {
    a_rather_long_user_colour_field_name: Color,
}

/// The custom properties card is hidden from the inspector, so a user
/// component's long colour field stands in for a long property name: both take
/// the same inline label.
#[test]
fn a_long_colour_field_name_is_not_clipped() {
    const LONG: &str = "a_rather_long_user_colour_field_name";
    let mut app = util::editor_test_app();
    app.register_type::<Swatches>();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    let entity = app
        .world_mut()
        .spawn((Name::new("swatches"), Swatches::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    jackdaw::selection::select_only(app.world_mut(), entity);
    settle(&mut app);
    let label = app
        .world_mut()
        .query::<(Entity, &Text)>()
        .iter(app.world())
        .find(|(_, text)| text.0.starts_with(LONG))
        .map(|(entity, _)| entity)
        .expect("the field's label");
    assert_eq!(
        app.world().get::<Text>(label).map(|t| t.0.as_str()),
        Some(format!("{LONG}:").as_str())
    );
    let node = app.world().get::<Node>(label).expect("a node");
    assert_eq!(node.width, Val::Auto, "the label is as wide as its name");
    assert_ne!(node.overflow, Overflow::clip(), "nothing cuts the name off");
}
