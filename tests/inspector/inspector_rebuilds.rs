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

/// Run frames until one passes with no UI node coming or going, then zero the
/// tally, so work that lands late, such as assets finishing loading on a slow
/// machine, is not counted against what the test does next.
fn reset(app: &mut App) {
    for _ in 0..120 {
        *app.world_mut().resource_mut::<Tally>() = Tally::default();
        app.update();
        let tally = app.world().resource::<Tally>();
        if tally.nodes_added == 0 && tally.nodes_removed == 0 {
            break;
        }
    }
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

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Stamina(f32);

#[test]
fn a_new_selection_whose_cardless_components_arrive_a_frame_later_is_built_once() {
    let (mut app, _first, _second) = app_with_two_entities();
    reset(&mut app);

    let entity = app
        .world_mut()
        .spawn((Name::new("third"), Transform::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    jackdaw::selection::select_only(app.world_mut(), entity);
    app.update();
    app.world_mut()
        .entity_mut(entity)
        .insert((ViewVisibility::default(), InheritedVisibility::default()));
    settle(&mut app);

    let showing = cards_showing(&mut app);
    assert!(showing > 0, "the new entity's cards are up");
    assert_eq!(
        app.world().resource::<Tally>().cards_added,
        showing,
        "components with no card of their own do not put the cards up again",
    );
}

#[test]
fn a_component_that_gets_a_card_arriving_later_puts_the_cards_up_again() {
    let (mut app, first, _second) = app_with_two_entities();
    app.register_type::<Stamina>();
    select(&mut app, first);
    let before = cards_showing(&mut app);
    reset(&mut app);

    app.world_mut().entity_mut(first).insert(Stamina(1.0));
    settle(&mut app);

    let showing = cards_showing(&mut app);
    assert!(showing > before, "the new component has a card");
    assert_eq!(
        app.world().resource::<Tally>().cards_added,
        showing,
        "and the cards were put up once",
    );
}
