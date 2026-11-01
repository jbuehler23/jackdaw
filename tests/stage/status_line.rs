//! The footer stays one line inside the window, and names the open scene
//! against the project that holds it.

use crate::util;

use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use jackdaw::status_bar::{StatusBarCenterSlot, StatusBarRightBox};
use jackdaw_feathers::status_bar::StatusBarCenter;

const WINDOW_WIDTH: f32 = 1280.0;

fn one<T: Component>(app: &mut App) -> Entity {
    app.world_mut()
        .query_filtered::<Entity, With<T>>()
        .single(app.world())
        .expect("one")
}

fn right_edge(app: &App, entity: Entity) -> f32 {
    let computed = app.world().get::<ComputedNode>(entity).expect("laid out");
    let transform = app
        .world()
        .get::<UiGlobalTransform>(entity)
        .expect("placed");
    transform.translation.x + computed.size().x / 2.0
}

#[test]
fn a_long_scene_path_leaves_the_right_side_on_screen() {
    let mut app = util::editor_test_app();
    let root = app
        .world_mut()
        .spawn(Node {
            position_type: PositionType::Absolute,
            width: Val::Px(WINDOW_WIDTH),
            flex_direction: FlexDirection::Column,
            ..default()
        })
        .id();
    let bar = app
        .world_mut()
        .spawn(jackdaw::layout::editor_status_bar())
        .id();
    app.world_mut().entity_mut(bar).insert(ChildOf(root));
    app.update();
    let slot = one::<StatusBarCenterSlot>(&mut app);
    app.world_mut().spawn((
        Node {
            width: Val::Px(2000.0),
            height: Val::Px(4.0),
            flex_shrink: 0.0,
            ..default()
        },
        ChildOf(slot),
    ));
    app.update();
    app.update();
    let right = one::<StatusBarRightBox>(&mut app);
    let edge = right_edge(&app, right);
    assert!(
        edge <= WINDOW_WIDTH + 0.5,
        "the right box ends at {edge} px in a {WINDOW_WIDTH} px window"
    );
}

#[test]
fn the_scene_label_follows_the_project_that_opens() {
    let mut app = util::editor_test_app();
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    for _ in 0..4 {
        app.update();
    }
    let shown = app
        .world_mut()
        .query_filtered::<Entity, With<StatusBarCenter>>()
        .iter(app.world())
        .next();
    let text = shown.unwrap_or_else(|| {
        app.world_mut()
            .spawn((StatusBarCenter, Text::default()))
            .id()
    });
    app.world_mut()
        .resource_mut::<jackdaw::scene_io::SceneFilePath>()
        .path = Some("/tmp/jd-status/game/assets/levels/one.bsn".to_string());
    app.update();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot::new(
            "/tmp/jd-status/game",
            default(),
        ));
    app.update();
    let shown = app.world().get::<Text>(text).expect("text").0.clone();
    assert!(shown.ends_with("levels/one.bsn"), "{shown}");
}
