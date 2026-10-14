//! Operators that act on a target `Entity` read it from
//! `OperatorParameters::as_entity("entity")`, which only matches
//! `PropertyValue::Entity`. An `entity.to_bits() as i64` falls through to `None`,
//! so the operator early-returns `Cancelled` and the action is a silent no-op.
//! Both directions are pinned here, so extending `as_entity` to coerce
//! `PropertyValue::Int` has to be a deliberate change.

use crate::util;

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::PropertyValue;

/// Test-only reflected component the picker can resolve by `type_path`.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
struct OperatorParamTestMarker {
    value: i32,
}

/// Deliberately no `Default`. The bool/String/f32 fields exercise three primitive
/// `ReflectDefault` paths through `build_reflective_default`.
#[derive(Component, Reflect, Debug, PartialEq)]
#[reflect(Component)]
struct OperatorParamNoDefaultMarker {
    a: bool,
    b: String,
    c: f32,
}

/// Build the editor test app and register `OperatorParamTestMarker` so
/// `component_id_for_path` can find it. No entity has the component, so this also
/// exercises the `register_type`-only path `component.add` must handle.
fn app_with_test_marker() -> App {
    let mut app = util::editor_test_app();
    app.register_type::<OperatorParamTestMarker>();
    app.register_type::<OperatorParamNoDefaultMarker>();
    app
}

/// Spawn a target entity and make it the primary selection: inspector operators
/// gate on `has_primary_selection` before they ever inspect their params.
fn spawn_selected_target(app: &mut App) -> Entity {
    let entity = app.world_mut().spawn(Name::new("op-param-target")).id();
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    app.update();
    entity
}

fn spawn_authored_selected_target(app: &mut App) -> Entity {
    let entity = spawn_selected_target(app);
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    entity
}

#[test]
fn component_add_with_entity_param_inserts_component() {
    let mut app = app_with_test_marker();
    let entity = spawn_authored_selected_target(&mut app);

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "operators::operator_entity_params::OperatorParamTestMarker".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(
        result,
        OperatorResult::Finished,
        "component.add should report Finished with valid params"
    );

    // The dispatcher queues the insert via `commands.queue`, so a frame has to
    // tick before the ECS reflects the change.
    app.update();

    assert!(
        app.world()
            .entity(entity)
            .contains::<OperatorParamTestMarker>(),
        "component.add did not insert OperatorParamTestMarker; the entity-param plumbing regressed"
    );
}

#[test]
fn component_add_inserts_component_without_default_derive() {
    // Components without a `Default` derive must still insert via
    // `build_reflective_default`, which walks field defaults.
    let mut app = app_with_test_marker();
    let entity = spawn_authored_selected_target(&mut app);

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "operators::operator_entity_params::OperatorParamNoDefaultMarker".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);

    app.update();
    let inserted = app
        .world()
        .entity(entity)
        .get::<OperatorParamNoDefaultMarker>()
        .expect("no-default component should land on the entity");
    assert!(!inserted.a);
    assert_eq!(inserted.b, "");
    assert_eq!(inserted.c, 0.0);
}

#[test]
fn component_add_with_int_entity_param_cancels() {
    // Passing the entity as `i64` must not mutate the world: `as_entity` rejects
    // `PropertyValue::Int` and the operator returns `Cancelled` cleanly.
    let mut app = app_with_test_marker();
    let entity = spawn_selected_target(&mut app);
    let entity_as_int: i64 = entity.to_bits() as i64;

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity_as_int)
        .param(
            "type_path",
            "operators::operator_entity_params::OperatorParamTestMarker".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(
        result,
        OperatorResult::Cancelled,
        "component.add must reject PropertyValue::Int for `entity`; \
         coercing it would silently revive the regression that broke \
         every inspector/hierarchy operator at once"
    );

    app.update();
    assert!(
        !app.world()
            .entity(entity)
            .contains::<OperatorParamTestMarker>(),
        "component.add inserted the component despite Cancelled result"
    );
}

#[test]
fn component_add_refuses_an_untracked_entity() {
    let mut app = app_with_test_marker();
    let entity = spawn_selected_target(&mut app);
    let undo_before = app.world().resource::<CommandHistory>().undo_stack.len();

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "operators::operator_entity_params::OperatorParamTestMarker".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);

    app.update();
    assert!(
        !app.world()
            .entity(entity)
            .contains::<OperatorParamTestMarker>(),
        "component.add must not insert onto an entity that is not in the document"
    );
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        undo_before,
        "a refused add must not land on the undo stack"
    );
}

#[test]
fn component_remove_with_entity_param_removes_component() {
    let mut app = app_with_test_marker();
    let entity = spawn_selected_target(&mut app);

    // Seed the component so there's something to remove.
    app.world_mut()
        .entity_mut(entity)
        .insert(OperatorParamTestMarker { value: 42 });
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);
    assert!(
        app.world()
            .entity(entity)
            .contains::<OperatorParamTestMarker>()
    );

    let result = app
        .world_mut()
        .operator("component.remove")
        .param("entity", entity)
        .param(
            "type_path",
            "operators::operator_entity_params::OperatorParamTestMarker".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);

    app.update();
    assert!(
        !app.world()
            .entity(entity)
            .contains::<OperatorParamTestMarker>(),
        "component.remove did not remove the component"
    );
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.ast_for(entity).expect("the target is tracked");
    assert!(
        !ast.component_type_paths(node)
            .iter()
            .any(|path| { path == "operators::operator_entity_params::OperatorParamTestMarker" }),
        "component.remove left the component in the document; document holds {:?}",
        ast.component_type_paths(node)
    );
}

/// Every operator reading `entity` through `as_entity("entity")` must reject a
/// `PropertyValue::Int`. `type_path` / `field_path` are filled with valid
/// placeholders so the rejected entity param is the only thing that can drive
/// the call to `Cancelled`.

#[test]
fn entity_param_rejects_int_across_inspector_and_hierarchy_ops() {
    let mut app = app_with_test_marker();
    let entity = spawn_selected_target(&mut app);
    let entity_as_int: i64 = entity.to_bits() as i64;

    let type_path_factory: fn() -> PropertyValue = || {
        PropertyValue::String(
            "operators::operator_entity_params::OperatorParamTestMarker"
                .to_string()
                .into(),
        )
    };
    let field_factory: fn() -> PropertyValue = || PropertyValue::String("value".to_string().into());
    let kind_factory: fn() -> PropertyValue = || PropertyValue::String("field".to_string().into());

    let cases: &[(&'static str, &[(&'static str, fn() -> PropertyValue)])] = &[
        ("component.add", &[("type_path", type_path_factory)]),
        ("component.remove", &[("type_path", type_path_factory)]),
        (
            "component.revert_baseline",
            &[("type_path", type_path_factory)],
        ),
        (
            "animation.toggle_keyframe",
            &[
                ("component_type_path", type_path_factory),
                ("field_path", field_factory),
            ],
        ),
        ("hierarchy.rename_begin", &[]),
        (
            "field.set",
            &[
                ("type_path", type_path_factory),
                ("field", field_factory),
                ("value", field_factory),
            ],
        ),
        ("binding.add", &[("kind", kind_factory)]),
        ("binding.set", &[]),
    ];
    for (id, extras) in cases {
        let mut builder = app.world_mut().operator(*id).param("entity", entity_as_int);
        for (key, factory) in *extras {
            builder = builder.param(*key, factory());
        }
        let result = builder
            .call()
            .unwrap_or_else(|err| panic!("{id}: dispatch errored: {err}"));
        assert_eq!(
            result,
            OperatorResult::Cancelled,
            "{id} accepted PropertyValue::Int for `entity`; that revives \
             the silent-fail regression. If `as_entity` was intentionally \
             extended to coerce ints, update this guard with that rationale."
        );
    }
}
