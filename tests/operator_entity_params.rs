//! Inspector / hierarchy operators that act on a target `Entity` all
//! read it from `OperatorParameters::as_entity("entity")`. That helper
//! ONLY matches `PropertyValue::Entity` (see
//! `crates/jackdaw_api_internal/src/operator.rs:180`); a `PropertyValue::Int`
//! produced by `entity.to_bits() as i64` falls through to `None`, so
//! the operator early-returns `Cancelled` and the user-visible action
//! becomes a silent no-op.
//!
//! That regression hit every inspector and hierarchy call site at once
//! when the codebase had `.param("entity", entity.to_bits() as i64)`
//! sprinkled around. The fix was to pass `Entity` directly so the
//! `From<Entity> for PropertyValue` conversion produces
//! `PropertyValue::Entity`.
//!
//! The tests in this file pin both directions:
//!  * Happy path: dispatching with a real `Entity` actually mutates the
//!    ECS (the component lands, physics components attach/detach, etc.).
//!  * Regression guard: dispatching the same op with `entity.to_bits()
//!    as i64` returns `Cancelled` and leaves the world untouched. If a
//!    future refactor "helpfully" extends `as_entity` to coerce
//!    `PropertyValue::Int`, that's a deliberate behaviour change and
//!    these tests will demand attention.

use bevy::prelude::*;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_avian_integration::AvianCollider;
use jackdaw_jsn::PropertyValue;

mod util;

/// Test-only component used by the component.add/remove tests. Lives
/// here (instead of a fixture crate) because we just need a reflected
/// type the picker can resolve by `type_path`.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
struct OperatorParamTestMarker {
    value: i32,
}

/// Minimum-viable component shape: derive + reflect, no
/// `Default`. The bool/String/f32 fields exercise three
/// different primitive `ReflectDefault` paths through
/// `build_reflective_default`.
#[derive(Component, Reflect, Debug, PartialEq)]
#[reflect(Component)]
struct OperatorParamNoDefaultMarker {
    a: bool,
    b: String,
    c: f32,
}

/// Build the editor test app and register `OperatorParamTestMarker`
/// for reflection so `component_id_for_path` can find it. The
/// component starts unseen by the world (no entity has it), so this
/// also exercises the `register_type`-only path that `component.add`
/// must handle.
fn app_with_test_marker() -> App {
    let mut app = util::editor_test_app();
    app.register_type::<OperatorParamTestMarker>();
    app.register_type::<OperatorParamNoDefaultMarker>();
    app
}

/// Spawn a target entity and make it the primary selection. Inspector
/// operators gate on `has_primary_selection`, so without this they
/// short-circuit before ever inspecting their params.
fn spawn_selected_target(app: &mut App) -> Entity {
    let entity = app.world_mut().spawn(Name::new("op-param-target")).id();
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    app.update();
    entity
}

#[test]
fn component_add_with_entity_param_inserts_component() {
    let mut app = app_with_test_marker();
    let entity = spawn_selected_target(&mut app);

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "operator_entity_params::OperatorParamTestMarker".to_string(),
        )
        .call()
        .expect("dispatch resolves");
    assert_eq!(
        result,
        OperatorResult::Finished,
        "component.add should report Finished with valid params"
    );

    // The dispatcher queues the actual insert via `commands.queue`, so
    // we have to drain pending commands by ticking a frame before the
    // ECS reflects the change.
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
    // Regression guard: components without a `Default` derive
    // must still reach the picker and insert via
    // `build_reflective_default` (which walks field defaults).
    let mut app = app_with_test_marker();
    let entity = spawn_selected_target(&mut app);

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity)
        .param(
            "type_path",
            "operator_entity_params::OperatorParamNoDefaultMarker".to_string(),
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
    // Regression guard for the silent-fail bug: passing the entity as
    // `i64` (the old broken pattern from inspector/hierarchy UI code)
    // must NOT mutate the world. `as_entity` rejects `PropertyValue::Int`
    // and the operator should return `Cancelled` cleanly.
    let mut app = app_with_test_marker();
    let entity = spawn_selected_target(&mut app);
    let entity_as_int: i64 = entity.to_bits() as i64;

    let result = app
        .world_mut()
        .operator("component.add")
        .param("entity", entity_as_int)
        .param(
            "type_path",
            "operator_entity_params::OperatorParamTestMarker".to_string(),
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
fn component_remove_with_entity_param_removes_component() {
    let mut app = app_with_test_marker();
    let entity = spawn_selected_target(&mut app);

    // Seed the component so there's something to remove.
    app.world_mut()
        .entity_mut(entity)
        .insert(OperatorParamTestMarker { value: 42 });
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
            "operator_entity_params::OperatorParamTestMarker".to_string(),
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
}

#[test]
fn physics_enable_with_entity_param_attaches_components() {
    use avian3d::prelude::RigidBody;

    let mut app = util::editor_test_app();
    let entity = spawn_selected_target(&mut app);

    let result = app
        .world_mut()
        .operator("physics.enable")
        .param("entity", entity)
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);

    app.update();
    let entity_ref = app.world().entity(entity);
    assert!(
        entity_ref.contains::<RigidBody>(),
        "physics.enable should attach RigidBody"
    );
    assert!(
        entity_ref.contains::<AvianCollider>(),
        "physics.enable should attach AvianCollider"
    );
}

#[test]
fn physics_disable_with_entity_param_detaches_components() {
    use avian3d::prelude::ColliderConstructor;
    use avian3d::prelude::RigidBody;

    let mut app = util::editor_test_app();
    let entity = spawn_selected_target(&mut app);

    // Pre-attach physics so disable has something to remove.
    app.world_mut().entity_mut(entity).insert((
        RigidBody::Dynamic,
        AvianCollider(ColliderConstructor::Cuboid {
            x_length: 1.0,
            y_length: 1.0,
            z_length: 1.0,
        }),
    ));
    app.update();

    let result = app
        .world_mut()
        .operator("physics.disable")
        .param("entity", entity)
        .call()
        .expect("dispatch resolves");
    assert_eq!(result, OperatorResult::Finished);

    app.update();
    let entity_ref = app.world().entity(entity);
    assert!(
        !entity_ref.contains::<RigidBody>(),
        "physics.disable should remove RigidBody"
    );
    assert!(
        !entity_ref.contains::<AvianCollider>(),
        "physics.disable should remove AvianCollider"
    );
}

/// Sweep guard: every operator that reads `entity` from params via
/// `as_entity("entity")` must reject a `PropertyValue::Int`, because
/// the inspector/hierarchy UI used to pass `entity.to_bits() as i64`
/// and got silent no-ops across the board. Looping the contract on
/// every entity-taking op is cheaper than per-op happy-path tests and
/// would catch a future call site that adds an `as_entity` impl on
/// `PropertyValue::Int`.
///
/// `type_path` / `field_path` etc. are filled with valid placeholders
/// so the only thing that can drive the call to `Cancelled` is the
/// rejected entity param.
#[test]
fn entity_param_rejects_int_across_inspector_and_hierarchy_ops() {
    let mut app = app_with_test_marker();
    let entity = spawn_selected_target(&mut app);
    let entity_as_int: i64 = entity.to_bits() as i64;

    let type_path_factory: fn() -> PropertyValue = || {
        PropertyValue::String(
            "operator_entity_params::OperatorParamTestMarker"
                .to_string()
                .into(),
        )
    };
    let field_factory: fn() -> PropertyValue = || PropertyValue::String("value".to_string().into());

    let cases: &[(&'static str, &[(&'static str, fn() -> PropertyValue)])] = &[
        ("component.add", &[("type_path", type_path_factory)]),
        ("component.remove", &[("type_path", type_path_factory)]),
        (
            "component.revert_baseline",
            &[("type_path", type_path_factory)],
        ),
        ("physics.enable", &[]),
        ("physics.disable", &[]),
        (
            "animation.toggle_keyframe",
            &[
                ("component_type_path", type_path_factory),
                ("field_path", field_factory),
            ],
        ),
        ("hierarchy.rename_begin", &[]),
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
