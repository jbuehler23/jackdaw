//! An enum field's variant dropdown inside its card.

use bevy::feathers::controls::{
    FeathersListRow, FeathersMenu, FeathersMenuButton, FeathersMenuPopup, FeathersSelect,
};
use bevy::prelude::*;
use bevy::ui_widgets::SetSelected;
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

/// Pick the row captioned `caption` in the card's variant dropdown, the way
/// the list does on a click.
fn pick(app: &mut App, caption: &str) {
    let rows: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<FeathersListRow>>()
        .iter(app.world())
        .collect();
    let row = app
        .world_mut()
        .query::<(&Text, &ChildOf)>()
        .iter(app.world())
        .find_map(|(text, child_of)| {
            (text.0 == caption && rows.contains(&child_of.parent())).then_some(child_of.parent())
        })
        .unwrap_or_else(|| panic!("the dropdown offers {caption}"));
    let select = ancestors(app, row)
        .into_iter()
        .find(|ancestor| app.world().get::<FeathersSelect>(*ancestor).is_some())
        .expect("the row is in a select");
    app.world_mut().trigger(SetSelected {
        entity: select,
        row,
    });
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

    pick(&mut app, "Hull");
    settle(&mut app);

    assert_eq!(app.world().get::<Shape>(entity), Some(&Shape::Hull));
    assert_eq!(
        orphaned_menus(&mut app),
        0,
        "a menu was left at the window root"
    );
}

#[derive(Component, Reflect, Clone, PartialEq, Debug, Default)]
#[reflect(Component, Default)]
enum Facing {
    #[default]
    North,
    South,
    East,
}

fn run(app: &mut App, clause: &str) {
    use crate::util::OperatorResultExt as _;
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    for _ in 0..300 {
        app.update();
        if app
            .world()
            .resource::<jackdaw::test_input::SyntheticInput>()
            .is_idle()
        {
            break;
        }
    }
    settle(app);
}

/// Click the middle of `entity` as drawn.
fn click(app: &mut App, entity: Entity) {
    let transform = app
        .world()
        .get::<bevy::ui::UiGlobalTransform>(entity)
        .expect("the node is placed");
    let computed = app
        .world()
        .get::<ComputedNode>(entity)
        .expect("the node is laid out");
    assert!(computed.size().x > 0.0, "{entity} is on screen");
    let at = transform.translation
        * computed.inverse_scale_factor()
        * app.world().resource::<UiScale>().0;
    run(
        app,
        &format!("input.pointer x={} y={} action=click", at.x, at.y),
    );
}

fn ancestors(app: &App, entity: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut cursor = app.world().get::<ChildOf>(entity).map(ChildOf::parent);
    while let Some(ancestor) = cursor {
        out.push(ancestor);
        cursor = app.world().get::<ChildOf>(ancestor).map(ChildOf::parent);
    }
    out
}

/// The menu button of the dropdown in the card showing `caption`. A card ships
/// collapsed in a fresh app and opens on a pointer gesture; headless, its body
/// is shown by hand so the dropdown lays out.
fn dropdown_reading(app: &mut App, caption: &str) -> Entity {
    let texts: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &Text)>()
        .iter(app.world())
        .filter(|(_, text)| text.0 == caption)
        .map(|(entity, _)| entity)
        .collect();
    let (text, button) = texts
        .into_iter()
        .find_map(|text| {
            let ancestors = ancestors(app, text);
            if ancestors
                .iter()
                .any(|a| app.world().get::<FeathersMenuPopup>(*a).is_some())
            {
                return None;
            }
            ancestors
                .into_iter()
                .find(|a| app.world().get::<FeathersMenuButton>(*a).is_some())
                .map(|button| (text, button))
        })
        .unwrap_or_else(|| panic!("a dropdown reads {caption}"));
    for ancestor in ancestors(app, text) {
        if let Some(mut node) = app.world_mut().get_mut::<Node>(ancestor)
            && node.display == Display::None
        {
            node.display = Display::Flex;
        }
    }
    settle(app);
    button
}

/// The text the menu button draws.
fn button_caption(app: &App, button: Entity) -> String {
    let mut stack = vec![button];
    while let Some(entity) = stack.pop() {
        if let Some(text) = app.world().get::<Text>(entity) {
            return text.0.clone();
        }
        if let Some(children) = app.world().get::<Children>(entity) {
            stack.extend(children.iter().rev());
        }
    }
    panic!("the menu button draws a caption");
}

/// The drawn text reading `caption`, if exactly one is drawn.
fn drawn_text(app: &mut App, caption: &str) -> Option<Entity> {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &Text, &InheritedVisibility, &ComputedNode)>()
        .iter(app.world())
        .filter(|(_, text, visible, computed)| {
            text.0 == caption && visible.get() && computed.size().x > 0.0
        })
        .map(|(entity, ..)| entity)
        .collect();
    (found.len() == 1).then(|| found[0])
}

/// A unit enum on a shown entity is offered as a dropdown in its card; picking
/// another variant with the pointer writes it and the dropdown then reads it,
/// as it does the variant an undo puts back.
#[test]
fn a_picked_variant_is_what_the_dropdown_reads() {
    let mut app = util::editor_test_app();
    app.register_type::<Facing>();
    // The editor's own layout may already fill the window, so the inspector
    // under test sits above it, and below the dropdown's popup, where the
    // pointer reaches it.
    let panel = app
        .world_mut()
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: px(320),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            GlobalZIndex(50),
        ))
        .id();
    app.world_mut().spawn((
        jackdaw::layout::inspector_components_content(default()),
        ChildOf(panel),
    ));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    let entity = app
        .world_mut()
        .spawn((Name::new("facing"), Facing::North))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    app.world_mut()
        .resource_scope(|world, mut selection: Mut<Selection>| {
            let mut commands = world.commands();
            selection.select_single(&mut commands, entity);
        });
    app.world_mut().flush();
    settle(&mut app);

    let trigger = dropdown_reading(&mut app, "North");
    click(&mut app, trigger);
    let row = drawn_text(&mut app, "South").expect("the open list offers South");
    click(&mut app, row);

    assert_eq!(app.world().get::<Facing>(entity), Some(&Facing::South));
    assert_eq!(
        button_caption(&app, trigger),
        "South",
        "the dropdown reads the picked variant",
    );

    app.world_mut().resource_scope(
        |world, mut history: Mut<jackdaw::commands::CommandHistory>| {
            history.undo(world);
        },
    );
    settle(&mut app);
    assert_eq!(app.world().get::<Facing>(entity), Some(&Facing::North));
    assert_eq!(
        button_caption(&app, trigger),
        "North",
        "and follows the undo back",
    );
}
