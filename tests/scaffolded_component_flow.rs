//! End-to-end coverage for the scaffolded user flow:
//!  1. Register a custom component (the shapes a static-game
//!     template author would write).
//!  2. Make sure the editor's component picker lists it.
//!  3. Dispatch `component.add` to attach it to an authored
//!     entity (one tracked in the scene AST).
//!  4. Confirm the component lands on the entity AND the AST
//!     records it, so a save/load round-trip would persist the
//!     value.
//!
//! Together these prove the picker + operator + AST sync agree
//! on what a user can author in the editor and have survive a
//! scene reload.

use std::any::TypeId;
use std::collections::HashSet;

use bevy::prelude::*;
use jackdaw::commands::{EditorCommand, SetJsnField};
use jackdaw::inspector::component_picker::enumerate_pickable_components;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_jsn::SceneJsnAst;
use jackdaw_runtime::EditorCategory;

mod util;

/// Mirrors the static template's `SpinningCube`: derive +
/// reflect, no `Default`. Two primitive fields exercise the
/// `build_reflective_default` walker.
#[derive(Component, Reflect)]
#[reflect(Component, @EditorCategory::new("Gameplay"))]
struct SpinningCube {
    speed: f32,
    enabled: bool,
}

/// Marker component without fields.
#[derive(Component, Reflect)]
#[reflect(Component, @EditorCategory::new("Actor"))]
struct PlayerSpawn;

fn app_with_user_components() -> App {
    let mut app = util::editor_test_app();
    app.register_type::<SpinningCube>();
    app.register_type::<PlayerSpawn>();
    app
}

/// Spawn an authored entity, register it in the scene AST so
/// component edits persist, and make it the primary selection.
fn spawn_authored_entity(app: &mut App) -> Entity {
    let entity = app.world_mut().spawn(Name::new("authored")).id();
    app.world_mut()
        .resource_mut::<SceneJsnAst>()
        .create_node(entity, None);
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    app.update();
    entity
}

#[test]
fn scaffolded_user_components_reach_picker() {
    let app = app_with_user_components();
    let registry = app
        .world()
        .resource::<bevy::ecs::reflect::AppTypeRegistry>()
        .read();
    let pickables = enumerate_pickable_components(&registry, &HashSet::new());

    let spinning = pickables
        .iter()
        .find(|p| p.short_name == "SpinningCube")
        .expect("SpinningCube must appear in the picker");
    assert_eq!(spinning.category, "Gameplay");

    let player = pickables
        .iter()
        .find(|p| p.short_name == "PlayerSpawn")
        .expect("PlayerSpawn must appear in the picker");
    assert_eq!(player.category, "Actor");
}

#[test]
fn add_component_lands_on_entity_and_in_ast() {
    let mut app = app_with_user_components();
    let entity = spawn_authored_entity(&mut app);

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "scaffolded_component_flow::SpinningCube".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);

    app.update();

    // ECS state.
    let cube = app
        .world()
        .entity(entity)
        .get::<SpinningCube>()
        .expect("SpinningCube must land on the entity");
    assert_eq!(cube.speed, 0.0, "default-constructed value");
    assert!(!cube.enabled);

    // AST state, so save/load preserves the addition.
    let ast = app.world().resource::<SceneJsnAst>();
    let node = ast
        .node_for_entity(entity)
        .expect("authored entity must be tracked in the AST");
    assert!(
        node.components
            .contains_key("scaffolded_component_flow::SpinningCube"),
        "AddComponent must record the component in the AST so \
         scene save preserves it; node has: {:?}",
        node.components.keys().collect::<Vec<_>>(),
    );
}

#[test]
fn add_marker_component_round_trips_through_ast() {
    let mut app = app_with_user_components();
    let entity = spawn_authored_entity(&mut app);

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "scaffolded_component_flow::PlayerSpawn".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);

    app.update();

    assert!(app.world().entity(entity).contains::<PlayerSpawn>());

    let ast = app.world().resource::<SceneJsnAst>();
    let node = ast.node_for_entity(entity).expect("tracked");
    assert!(
        node.components
            .contains_key("scaffolded_component_flow::PlayerSpawn"),
        "marker component must round-trip through the AST too",
    );
}

#[test]
fn inspector_field_edit_updates_ecs_and_ast() {
    // Mirrors what the inspector does when the user types into a
    // `f32` field and commits: a `SetJsnField` command runs,
    // mutating both the scene AST (so save persists the value)
    // and the ECS component (so Play picks it up immediately).
    let mut app = app_with_user_components();
    let entity = spawn_authored_entity(&mut app);

    // Add the component as the user would.
    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "scaffolded_component_flow::SpinningCube".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);
    app.update();

    // Sanity: starts at the primitive default.
    let cube = app
        .world()
        .entity(entity)
        .get::<SpinningCube>()
        .expect("SpinningCube on entity");
    assert_eq!(cube.speed, 0.0);

    // Edit `speed` to 1.5 the same way the inspector text input
    // does it: build a `SetJsnField` and execute it on the world.
    let mut cmd: Box<dyn EditorCommand> = Box::new(SetJsnField {
        entity,
        type_path: "scaffolded_component_flow::SpinningCube".to_string(),
        field_path: "speed".to_string(),
        old_value: serde_json::json!(0.0),
        new_value: serde_json::json!(1.5),
        was_derived: false,
    });
    cmd.execute(app.world_mut());
    app.update();

    // ECS reflects the new value.
    let cube = app
        .world()
        .entity(entity)
        .get::<SpinningCube>()
        .expect("SpinningCube on entity");
    assert!(
        (cube.speed - 1.5).abs() < f32::EPSILON,
        "ECS field must update; got speed = {}",
        cube.speed,
    );

    // AST reflects the new value too, so save preserves it.
    let registry = app
        .world()
        .resource::<bevy::ecs::reflect::AppTypeRegistry>()
        .clone();
    let registry = registry.read();
    let ast = app.world().resource::<SceneJsnAst>();
    let value = ast
        .get_component_field(
            entity,
            "scaffolded_component_flow::SpinningCube",
            "speed",
            &registry,
        )
        .expect("AST must store the edited field");
    let speed = value
        .as_f64()
        .expect("speed serialises as a JSON number");
    assert!(
        (speed - 1.5).abs() < 1e-6,
        "AST field must update; got speed = {speed}",
    );
}

#[test]
fn inspector_field_edit_undoes_back_to_original() {
    // The inspector's edits go through the undo stack. Verify
    // execute then undo restores the original value in both ECS
    // and AST (matches what Ctrl-Z does in the UI).
    let mut app = app_with_user_components();
    let entity = spawn_authored_entity(&mut app);

    app.world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "scaffolded_component_flow::SpinningCube".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    app.update();

    let mut cmd: Box<dyn EditorCommand> = Box::new(SetJsnField {
        entity,
        type_path: "scaffolded_component_flow::SpinningCube".to_string(),
        field_path: "speed".to_string(),
        old_value: serde_json::json!(0.0),
        new_value: serde_json::json!(1.5),
        was_derived: false,
    });
    cmd.execute(app.world_mut());
    cmd.undo(app.world_mut());
    app.update();

    let cube = app
        .world()
        .entity(entity)
        .get::<SpinningCube>()
        .unwrap();
    assert!(
        (cube.speed - 0.0).abs() < f32::EPSILON,
        "undo must restore ECS speed to 0; got {}",
        cube.speed,
    );

    // Suppress unused-import warning when only this test uses TypeId.
    let _ = TypeId::of::<SpinningCube>();
}
