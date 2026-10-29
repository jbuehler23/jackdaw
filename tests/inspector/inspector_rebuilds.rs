//! When the inspector rebuilds its cards.
//!
//! The panel changes only when what it shows has changed, and then once: a
//! reselect of the shown entity leaves it standing, and a component added to
//! or taken from it brings or takes only that component's card.

use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;
use jackdaw::inspector::component_picker::InspectorAddComponentButton;
use jackdaw::inspector::{ComponentDisplay, Inspector};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_feathers::panel_card::DisclosureSection;
use jackdaw_widgets::collapsible::CollapsibleSection;

use crate::util;

#[derive(Resource, Default)]
struct Tally {
    nodes_added: usize,
    nodes_removed: usize,
    cards_added: usize,
    cards_removed: usize,
    added: Vec<Entity>,
    removed: Vec<String>,
}

fn diag_label(world: &World, e: Entity) -> String {
    let Ok(entity) = world.get_entity(e) else {
        return format!("<{e}>");
    };
    if let Some(name) = entity.get::<Name>() {
        return format!("'{}'", name.as_str());
    }
    let skip = [
        "Node", "ChildOf", "Children", "ComputedNode", "ComputedUiTargetCamera",
        "ComputedUiRenderTargetInfo", "UiTransform", "UiGlobalTransform", "Visibility",
        "InheritedVisibility", "ViewVisibility", "Transform", "GlobalTransform", "ZIndex",
        "FocusPolicy", "ScrollPosition", "BackgroundColor", "BorderColor", "BorderRadius",
        "Outline", "TransformTreeChanged", "Pickable", "RelativeCursorPosition", "Interaction",
        "TextLayoutInfo", "TextNodeFlags", "ContentSize", "TextLayout", "TextFont", "TextColor",
        "LineHeight", "LineBreak", "ComputedTextBlock", "TextBackgroundColor", "TextShadow",
        "Hovered", "Hovering", "BoxShadow", "SyncToRenderWorld", "Cursor", "EmSize",
        "UiTreeDirty", "ThemeContext", "CalculatedClip", "FontHinting", "InheritableFont",
    ];
    entity
        .archetype()
        .components()
        .iter()
        .filter_map(|id| world.components().get_info(*id))
        .map(|info| info.name().shortname().to_string())
        .filter(|name| {
            !skip.contains(&name.as_str())
                && !["Inherited<", "Propagate<", "Computed", "Taffy", "Accessibility", "Focusable"]
                    .iter()
                    .any(|prefix| name.starts_with(prefix))
        })
        .take(4)
        .collect::<Vec<_>>()
        .join("+")
}

fn diag_chain(world: &World, mut e: Entity) -> String {
    let mut out = vec![diag_label(world, e)];
    while let Some(parent) = world.get::<ChildOf>(e).map(ChildOf::parent) {
        e = parent;
        out.push(diag_label(world, e));
    }
    out.reverse();
    out.into_iter().take(10).collect::<Vec<_>>().join(" > ")
}

fn diag_report(app: &mut App) {
    let added = app.world().resource::<Tally>().added.clone();
    let removed = app.world().resource::<Tally>().removed.clone();
    let world = app.world();
    let mut groups: std::collections::BTreeMap<String, usize> = default();
    for e in added {
        *groups.entry(format!("ADD {}", diag_chain(world, e))).or_default() += 1;
    }
    for chain in removed {
        *groups.entry(format!("REM {chain}")).or_default() += 1;
    }
    for (chain, count) in groups {
        eprintln!("DIAG {count:>4} {chain}");
    }
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
        .add_observer(|add: On<Add<Node>>, mut tally: ResMut<Tally>| {
            tally.nodes_added += 1;
            tally.added.push(add.event_target());
        })
        .add_observer(
            |remove: On<Remove<Node>>, mut world: bevy::ecs::world::DeferredWorld| {
                let chain = diag_chain(&world, remove.event_target());
                let mut tally = world.resource_mut::<Tally>();
                tally.nodes_removed += 1;
                tally.removed.push(chain);
            },
        )
        .add_observer(|_: On<Add<ComponentDisplay>>, mut tally: ResMut<Tally>| {
            tally.cards_added += 1;
        })
        .add_observer(
            |_: On<Remove<ComponentDisplay>>, mut tally: ResMut<Tally>| {
                tally.cards_removed += 1;
            },
        );
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

fn inspectors(app: &mut App) -> usize {
    app.world_mut()
        .query_filtered::<(), With<Inspector>>()
        .iter(app.world())
        .count()
}

/// The component cards of each panel, top to bottom, with their type paths.
fn cards_in_order(app: &mut App) -> Vec<(Entity, String)> {
    let inspectors: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<Inspector>>()
        .iter(app.world())
        .collect();
    let world = app.world();
    inspectors
        .into_iter()
        .filter_map(|inspector| world.get::<Children>(inspector))
        .flat_map(Children::iter)
        .filter_map(|card| {
            jackdaw::inspector::component_card_type_path(world, card)
                .map(|type_path| (card, type_path.to_string()))
        })
        .collect()
}

fn card_for(app: &mut App, type_path: &str) -> Entity {
    cards_in_order(app)
        .into_iter()
        .find(|(_, shown)| shown == type_path)
        .map(|(card, _)| card)
        .unwrap_or_else(|| panic!("a card for {type_path}"))
}

fn add_component(app: &mut App, entity: Entity, type_path: &str) {
    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param("type_path", type_path.to_string())
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished);
    settle(app);
}

fn remove_component(app: &mut App, entity: Entity, type_path: &str) {
    let result = app
        .world_mut()
        .operator("component.remove")
        .param("entity", entity)
        .param("type_path", type_path.to_string())
        .call()
        .expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished);
    settle(app);
}

fn visibility_path() -> &'static str {
    <Visibility as bevy::reflect::TypePath>::type_path()
}

fn transform_path() -> &'static str {
    <Transform as bevy::reflect::TypePath>::type_path()
}

#[test]
fn reselecting_the_shown_entity_leaves_the_inspector_standing() {
    let (mut app, first, _second) = app_with_two_entities();
    select(&mut app, first);
    assert!(cards_showing(&mut app) > 0, "the entity's cards are up");
    reset(&mut app);

    select(&mut app, first);

    diag_report(&mut app);
    let tally = app.world().resource::<Tally>();
    assert_eq!(tally.nodes_added, 0, "no UI node is put up again");
    assert_eq!(tally.nodes_removed, 0, "and none is taken down");
}

#[test]
fn adding_a_component_puts_up_only_its_card() {
    let (mut app, first, _second) = app_with_two_entities();
    select(&mut app, first);
    let before: Vec<Entity> = cards_in_order(&mut app)
        .into_iter()
        .map(|(card, _)| card)
        .collect();
    assert!(!before.is_empty(), "the entity's cards are up");
    let panels = inspectors(&mut app);
    reset(&mut app);

    add_component(&mut app, first, visibility_path());

    diag_report(&mut app);
    let tally = app.world().resource::<Tally>();
    assert_eq!(tally.cards_added, panels, "one card per panel is put up");
    assert_eq!(tally.cards_removed, 0, "and none is taken down");
    assert_eq!(tally.nodes_removed, 0, "no UI node is taken down");
    let after = cards_in_order(&mut app);
    for card in &before {
        assert!(
            after.iter().any(|(shown, _)| shown == card),
            "the card {card} still stands",
        );
    }
    assert!(
        after
            .iter()
            .any(|(_, type_path)| type_path == visibility_path()),
        "the new component has its card",
    );
}

#[test]
fn removing_a_component_takes_down_only_its_card() {
    let (mut app, first, _second) = app_with_two_entities();
    select(&mut app, first);
    add_component(&mut app, first, visibility_path());
    let staying: Vec<Entity> = cards_in_order(&mut app)
        .into_iter()
        .filter(|(_, type_path)| type_path != visibility_path())
        .map(|(card, _)| card)
        .collect();
    let panels = inspectors(&mut app);
    reset(&mut app);

    remove_component(&mut app, first, visibility_path());

    diag_report(&mut app);
    let tally = app.world().resource::<Tally>();
    assert_eq!(
        tally.cards_removed, panels,
        "one card per panel is taken down"
    );
    assert_eq!(tally.cards_added, 0, "and none is put up");
    assert_eq!(tally.nodes_added, 0, "no UI node is put up");
    let after: Vec<Entity> = cards_in_order(&mut app)
        .into_iter()
        .map(|(card, _)| card)
        .collect();
    assert_eq!(after, staying, "the other cards stand, in their order");
}

#[test]
fn a_card_put_up_for_an_added_component_takes_the_place_a_fresh_build_gives_it() {
    let (mut app, first, second) = app_with_two_entities();
    select(&mut app, first);

    add_component(&mut app, first, visibility_path());
    let added: Vec<String> = cards_in_order(&mut app)
        .into_iter()
        .map(|(_, type_path)| type_path)
        .collect();

    select(&mut app, second);
    select(&mut app, first);
    let fresh: Vec<String> = cards_in_order(&mut app)
        .into_iter()
        .map(|(_, type_path)| type_path)
        .collect();

    assert_eq!(added, fresh);
}

#[test]
fn an_expanded_card_stays_expanded_when_another_component_is_added() {
    let (mut app, first, _second) = app_with_two_entities();
    select(&mut app, first);
    let transform = card_for(&mut app, transform_path());
    let disclosure = app
        .world_mut()
        .query::<(Entity, &DisclosureSection)>()
        .iter(app.world())
        .find(|(_, link)| link.0 == transform)
        .map(|(toggle, _)| toggle)
        .expect("the card has a disclosure");
    app.world_mut().trigger(ValueChange {
        source: disclosure,
        value: true,
        is_final: true,
    });
    settle(&mut app);
    assert!(
        app.world()
            .get::<CollapsibleSection>(transform)
            .is_some_and(|section| !section.collapsed),
        "the card is expanded",
    );

    add_component(&mut app, first, visibility_path());

    assert_eq!(
        card_for(&mut app, transform_path()),
        transform,
        "the card is the one that was expanded",
    );
    assert!(
        app.world()
            .get::<CollapsibleSection>(transform)
            .is_some_and(|section| !section.collapsed),
        "and it is still expanded",
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
fn a_component_that_gets_a_card_arriving_later_puts_up_its_card() {
    let (mut app, first, _second) = app_with_two_entities();
    app.register_type::<Stamina>();
    select(&mut app, first);
    let before = cards_showing(&mut app);
    let panels = inspectors(&mut app);
    reset(&mut app);

    app.world_mut().entity_mut(first).insert(Stamina(1.0));
    settle(&mut app);

    assert_eq!(
        cards_showing(&mut app),
        before + panels,
        "the new component has a card in each panel",
    );
    diag_report(&mut app);
    let tally = app.world().resource::<Tally>();
    assert_eq!(tally.cards_added, panels, "and only it was put up");
    assert_eq!(tally.cards_removed, 0);
}
