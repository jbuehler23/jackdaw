//! When the inspector rebuilds its cards.
//!
//! The panel is put up again only when what it shows has changed, and then
//! once: a reselect of the shown entity leaves it standing, and an added
//! component brings one new set of cards rather than two.

use bevy::prelude::*;
use jackdaw::inspector::ComponentDisplay;
use jackdaw::inspector::component_picker::InspectorAddComponentButton;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;

use crate::util;

#[derive(Resource, Default)]
struct Tally {
    nodes_added: usize,
    nodes_removed: usize,
    cards_added: usize,
}

fn settle(app: &mut App) {
    for _ in 0..6 {
        app.update();
    }
}

fn select(app: &mut App, entity: Entity) {
    let world = app.world_mut();
    world.resource_scope(|world, mut selection: Mut<Selection>| {
        let mut commands = world.commands();
        selection.select_single(&mut commands, entity);
    });
    world.flush();
    settle(app);
}

fn reset(app: &mut App) {
    *app.world_mut().resource_mut::<Tally>() = Tally::default();
}

/// An editor with an inspector panel and two document entities, neither
/// selected yet, counting the UI nodes and cards that come and go.
fn app_with_two_entities() -> (App, Entity, Entity) {
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    let mut spawn_one = |name: &str, x: f32| {
        let entity = app
            .world_mut()
            .spawn((
                Name::new(name.to_string()),
                Transform::from_xyz(x, 0.0, 0.0),
            ))
            .id();
        jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
        entity
    };
    let first = spawn_one("first", 1.0);
    let second = spawn_one("second", 2.0);
    settle(&mut app);
    app.init_resource::<Tally>()
        .add_observer(|_: On<Add<Node>>, mut tally: ResMut<Tally>| {
            tally.nodes_added += 1;
        })
        .add_observer(|_: On<Remove<Node>>, mut tally: ResMut<Tally>| {
            tally.nodes_removed += 1;
        })
        .add_observer(|_: On<Add<ComponentDisplay>>, mut tally: ResMut<Tally>| {
            tally.cards_added += 1;
        });
    (app, first, second)
}

fn add_component_button(app: &mut App) -> Option<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<InspectorAddComponentButton>>()
        .iter(app.world())
        .next()
}

fn cards_showing(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<(), With<ComponentDisplay>>()
        .iter(app.world())
        .count()
}

#[test]
fn reselecting_the_shown_entity_leaves_the_inspector_standing() {
    let (mut app, first, _second) = app_with_two_entities();
    select(&mut app, first);
    assert!(cards_showing(&mut app) > 0, "the entity's cards are up");
    reset(&mut app);

    select(&mut app, first);

    let tally = app.world().resource::<Tally>();
    assert_eq!(tally.nodes_added, 0, "no UI node is put up again");
    assert_eq!(tally.nodes_removed, 0, "and none is taken down");
}

#[test]
fn adding_a_component_rebuilds_the_inspector_once() {
    let (mut app, first, _second) = app_with_two_entities();
    select(&mut app, first);
    reset(&mut app);

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", first)
        .param(
            "type_path",
            <Visibility as bevy::reflect::TypePath>::type_path().to_string(),
        )
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished);
    settle(&mut app);

    let showing = cards_showing(&mut app);
    assert_eq!(
        app.world().resource::<Tally>().cards_added,
        showing,
        "every card showing was put up once, and none twice",
    );
}

#[test]
fn the_add_component_button_survives_a_change_of_selection() {
    let (mut app, first, second) = app_with_two_entities();
    select(&mut app, first);
    let button = add_component_button(&mut app).expect("the header shows Add Component");

    select(&mut app, second);

    assert_eq!(
        add_component_button(&mut app),
        Some(button),
        "the same button stays rather than a new one taking its place",
    );
}

#[test]
fn the_add_component_button_survives_a_component_being_added() {
    let (mut app, first, _second) = app_with_two_entities();
    select(&mut app, first);
    let button = add_component_button(&mut app).expect("the header shows Add Component");

    app.world_mut()
        .entity_mut(first)
        .insert(PointLight::default());
    settle(&mut app);

    assert_eq!(add_component_button(&mut app), Some(button));
}
