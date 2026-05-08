//! End-to-end coverage for the scaffolded user flow: a custom
//! component reaches the picker, `component.add` attaches it to
//! an authored entity, and the AST records the addition so a
//! save/load round-trip would persist it.

use std::collections::HashSet;

use bevy::prelude::*;
use jackdaw::commands::{EditorCommand, SetJsnField};
use jackdaw::inspector::component_picker::{PickerDenylist, enumerate_pickable_components};
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_jsn::SceneJsnAst;
use jackdaw_runtime::{EditorCategory, EditorHidden};

mod util;

#[derive(Component, Reflect)]
#[reflect(Component, @EditorCategory::new("Gameplay"))]
struct SpinningCube {
    speed: f32,
    enabled: bool,
}

#[derive(Component, Reflect)]
#[reflect(Component, @EditorCategory::new("Actor"))]
struct PlayerSpawn;

/// Mirrors a plugin author marking a helper Component as
/// editor-hidden. Used by the integration test to verify the
/// dogfood-able public API works end-to-end through the real
/// editor `App` lifecycle.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default, @EditorHidden)]
struct PluginInternalSupport;

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
    let pickables =
        enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default());

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
fn editor_hidden_marker_hides_component_in_real_app() {
    // Mirrors a plugin author tagging a helper Component with
    // `@EditorHidden`. Registered through the same path a user's
    // crate would use; we then walk the full editor type registry
    // and assert the picker enumeration filters it out.
    let mut app = util::editor_test_app();
    app.register_type::<PluginInternalSupport>();
    app.register_type::<SpinningCube>();
    app.update();

    let registry = app
        .world()
        .resource::<bevy::ecs::reflect::AppTypeRegistry>()
        .read();
    let pickables =
        enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default());
    let names: Vec<&str> = pickables.iter().map(|p| p.short_name.as_str()).collect();

    assert!(
        names.contains(&"SpinningCube"),
        "control: unmarked component must still appear in the picker; got {names:?}",
    );
    assert!(
        !names.contains(&"PluginInternalSupport"),
        "@EditorHidden must hide a Component from the picker when \
         registered through the same App lifecycle a user/extension would use; got {names:?}",
    );
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

    let cube = app
        .world()
        .entity(entity)
        .get::<SpinningCube>()
        .expect("SpinningCube must land on the entity");
    assert_eq!(cube.speed, 0.0, "default-constructed value");
    assert!(!cube.enabled);

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
    // Inspector field commits dispatch `SetJsnField`, which must
    // mutate both the AST (so save persists) and the ECS
    // component (so play picks it up immediately).
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

    let cube = app
        .world()
        .entity(entity)
        .get::<SpinningCube>()
        .expect("SpinningCube on entity");
    assert_eq!(cube.speed, 0.0);

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
    let speed = value.as_f64().expect("speed serialises as a JSON number");
    assert!(
        (speed - 1.5).abs() < 1e-6,
        "AST field must update; got speed = {speed}",
    );
}

#[test]
fn inspector_field_edit_undoes_back_to_original() {
    // Inspector edits go through the undo stack; execute + undo
    // must restore the original value (Ctrl-Z in the UI).
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

    let cube = app.world().entity(entity).get::<SpinningCube>().unwrap();
    assert!(
        (cube.speed - 0.0).abs() < f32::EPSILON,
        "undo must restore ECS speed to 0; got {}",
        cube.speed,
    );
}
