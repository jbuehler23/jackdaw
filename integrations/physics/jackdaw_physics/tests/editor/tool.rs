//! The physics tool, driven the way a person drives it: Shift+P, a drag on a
//! selected body, then Space to keep where it settled or Esc to put it back.

use crate::util;

use avian3d::prelude::RigidBody;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use jackdaw::commands::CommandHistory;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_api::tools::ToolStatus;
use jackdaw_api::viewport::{MainViewportCamera, SceneViewport, ViewportRemap};
use jackdaw_scene_types::Brush;

/// A selected dynamic cube brush standing in front of the viewport camera.
fn dynamic_cube(app: &mut App) -> Entity {
    app.insert_resource(TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / 60.0),
    ));
    let result = app
        .world_mut()
        .operator("entity.add.cube")
        .call()
        .expect("dispatch");
    assert_eq!(result, OperatorResult::Finished);
    util::settle(app);
    let cube = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .single(app.world())
        .expect("one brush");
    let set = app
        .world_mut()
        .operator("component.set")
        .param("entity", cube)
        .param(
            "type_path",
            "avian3d::dynamics::rigid_body::RigidBody".to_string(),
        )
        .param("field", String::new())
        .param("value", "\"Dynamic\"".to_string())
        .call()
        .expect("dispatch");
    assert_eq!(set, OperatorResult::Finished);
    app.world_mut().resource_mut::<Selection>().entities = vec![cube];
    util::settle(app);
    assert_eq!(
        app.world().get::<RigidBody>(cube),
        Some(&RigidBody::Dynamic),
        "the cube simulates once the tool lets it"
    );
    cube
}

/// Where `point` is drawn, in the window pixels `input.pointer` takes.
fn on_screen(app: &mut App, point: Vec3) -> Vec2 {
    let (camera, camera_transform) = app
        .world_mut()
        .query_filtered::<(&Camera, &GlobalTransform), With<MainViewportCamera>>()
        .iter(app.world())
        .next()
        .map(|(camera, transform)| (camera.clone(), *transform))
        .expect("a viewport camera");
    let (computed, node_transform) = app
        .world_mut()
        .query_filtered::<(&ComputedNode, &bevy::ui::UiGlobalTransform), With<SceneViewport>>()
        .iter(app.world())
        .next()
        .map(|(computed, transform)| (*computed, *transform))
        .expect("a viewport node");
    let local = camera
        .world_to_viewport(&camera_transform, point)
        .expect("the point is in view");
    let map = ViewportRemap::new(&camera, &computed, &node_transform);
    map.top_left + local / map.remap
}

fn translation(app: &App, entity: Entity) -> Vec3 {
    app.world()
        .get::<Transform>(entity)
        .expect("a transform")
        .translation
}

/// Start the tool and throw the cube by dragging it up and to the side.
fn throw(app: &mut App, cube: Entity) {
    util::run(app, "input.key key=KeyP mods=shift");
    assert!(
        app.world().resource::<ToolStatus>().0.is_some(),
        "Shift+P starts the physics tool"
    );
    let from = on_screen(app, translation(app, cube));
    let to = from + Vec2::new(40.0, -60.0);
    util::run(
        app,
        &format!("input.pointer x={} y={} action=press", from.x, from.y),
    );
    util::run(
        app,
        &format!("input.pointer x={} y={} action=move", to.x, to.y),
    );
    util::run(
        app,
        &format!("input.pointer x={} y={} action=release", to.x, to.y),
    );
    for _ in 0..30 {
        app.update();
    }
}

#[test]
fn space_keeps_where_a_thrown_body_settled_as_one_undo_step() {
    let mut app = util::editor_with_viewport((1600, 1000));
    let cube = dynamic_cube(&mut app);
    let start = translation(&app, cube);
    let undo_depth = app.world().resource::<CommandHistory>().undo_stack.len();

    throw(&mut app, cube);
    let thrown = translation(&app, cube);
    assert!(
        thrown.distance(start) > 0.1,
        "the drag and the simulation moved the cube: {start} -> {thrown}"
    );

    util::run(&mut app, "input.key key=Space");
    assert_eq!(
        app.world().resource::<ToolStatus>().0,
        None,
        "Space ends the tool"
    );
    let kept = translation(&app, cube);
    util::settle(&mut app);
    assert!(
        translation(&app, cube).distance(kept) < 0.001,
        "the simulation stops with the tool"
    );
    assert!(
        kept.distance(start) > 0.1,
        "and the cube stays where it settled: {start} -> {kept}"
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        undo_depth + 1,
        "the move is one undo step"
    );

    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    util::settle(&mut app);
    let undone = translation(&app, cube);
    assert!(
        undone.distance(start) < 0.01,
        "undo puts the cube back where the tool found it: {start} -> {kept} -> {undone}"
    );
}

#[test]
fn escape_puts_a_thrown_body_back() {
    let mut app = util::editor_with_viewport((1600, 1000));
    let cube = dynamic_cube(&mut app);
    let start = translation(&app, cube);
    let undo_depth = app.world().resource::<CommandHistory>().undo_stack.len();

    throw(&mut app, cube);
    assert!(translation(&app, cube).distance(start) > 0.1);

    util::run(&mut app, "input.key key=Escape");
    assert_eq!(app.world().resource::<ToolStatus>().0, None);
    assert!(
        translation(&app, cube).distance(start) < 0.01,
        "Esc puts the cube back"
    );
    let history: Vec<String> = app
        .world()
        .resource::<CommandHistory>()
        .undo_stack
        .iter()
        .map(|entry| entry.description().to_string())
        .collect();
    assert_eq!(
        history.len(),
        undo_depth,
        "and leaves nothing to undo: {history:?}"
    );
}
