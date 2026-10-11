//! The inspector offers Add Component from the view it opens on, and a
//! component picked there is shown once it is added.

use crate::util;

use crate::util::OperatorResultExt as _;
use bevy::{
    prelude::*,
    ui::{ComputedNode, UiGlobalTransform},
    window::{PrimaryWindow, WindowResolution},
};
use jackdaw::inspector::component_picker::InspectorAddComponentButton;
use jackdaw::selection::Selection;

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct StageLantern;

fn settle(app: &mut App) {
    for _ in 0..8 {
        app.update();
    }
}

/// Run one synthetic input clause and play it out.
fn input(app: &mut App, clause: &str) {
    jackdaw::boot_ops::run_op_clause(app.world_mut(), clause)
        .expect("the clause dispatches")
        .assert_finished();
    for _ in 0..200 {
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

/// Where `entity` is drawn, in window logical pixels.
fn centre_of(app: &App, entity: Entity) -> Vec2 {
    let transform = app
        .world()
        .get::<UiGlobalTransform>(entity)
        .expect("the button is placed");
    let computed = app
        .world()
        .get::<ComputedNode>(entity)
        .expect("the button is laid out");
    transform.translation * computed.inverse_scale_factor() * app.world().resource::<UiScale>().0
}

/// The Add Component button inside the inspector mounted at `panel`.
fn add_component_button(app: &mut App, panel: Entity) -> Option<Entity> {
    let buttons: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<InspectorAddComponentButton>>()
        .iter(app.world())
        .collect();
    buttons.into_iter().find(|&button| {
        let mut cursor = Some(button);
        while let Some(entity) = cursor {
            if entity == panel {
                return true;
            }
            cursor = app.world().get::<ChildOf>(entity).map(ChildOf::parent);
        }
        false
    })
}

#[test]
fn add_component_is_offered_on_the_view_the_inspector_opens_on() {
    let mut app = util::editor_test_app();
    app.register_type::<StageLantern>();
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    settle(&mut app);
    {
        let mut windows = app
            .world_mut()
            .query_filtered::<&mut Window, With<PrimaryWindow>>();
        let mut window = windows
            .single_mut(app.world_mut())
            .expect("headless apps still have a primary window");
        window.resolution = WindowResolution::new(1600, 1000);
    }
    let leaf = app
        .world_mut()
        .spawn(Node {
            position_type: PositionType::Absolute,
            width: Val::Px(320.0),
            height: Val::Px(900.0),
            flex_direction: FlexDirection::Column,
            ..default()
        })
        .id();
    let content = app
        .world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()))
        .id();
    app.world_mut().entity_mut(content).insert(ChildOf(leaf));

    let entity = app
        .world_mut()
        .spawn((Name::new("Crate"), Transform::default()))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    let world = app.world_mut();
    world.resource_scope(|world, mut selection: Mut<Selection>| {
        let mut commands = world.commands();
        selection.select_single(&mut commands, entity);
    });
    world.flush();
    settle(&mut app);

    let button = add_component_button(&mut app, content)
        .expect("the Object view, where the inspector opens, offers Add Component");
    let at = centre_of(&app, button);
    input(
        &mut app,
        &format!("input.pointer x={} y={} action=click", at.x, at.y),
    );
    input(&mut app, "input.text text=StageLantern");
    input(&mut app, "input.key key=Enter");

    assert!(
        app.world().entity(entity).contains::<StageLantern>(),
        "the picked component is on the selected entity"
    );
    assert!(
        shows_text(&mut app, "Lantern"),
        "the inspector shows the added component's card"
    );
}

/// Whether a text containing `needle` is on screen: none of its ancestors is
/// hidden.
fn shows_text(app: &mut App, needle: &str) -> bool {
    let texts: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &Text)>()
        .iter(app.world())
        .filter(|(_, text)| text.0.contains(needle))
        .map(|(entity, _)| entity)
        .collect();
    texts.into_iter().any(|text| {
        let mut cursor = Some(text);
        while let Some(entity) = cursor {
            if app
                .world()
                .get::<Node>(entity)
                .is_some_and(|node| node.display == Display::None)
            {
                return false;
            }
            cursor = app.world().get::<ChildOf>(entity).map(ChildOf::parent);
        }
        true
    })
}
