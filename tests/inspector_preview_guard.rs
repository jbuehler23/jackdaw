//! What the inspector refuses while a preview session is running.
//!
//! A preview evaluator owns the components it writes: for as long as the
//! session is on, the number in `Node.width` belongs to the binding, not to
//! the document. The field rows refuse an authored edit to one, and so does
//! the component header's remove button: taking the whole component away is
//! the same edit, only larger, and letting it through would leave the
//! session driving a component that is not there.
//!
//! `physics.disable` is the same edit again, larger still: three removals
//! and a rewrite of the document's physics patches in one entry.

use bevy::prelude::*;
use jackdaw::preview_context;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_avian_integration::AvianCollider;
use jackdaw_bind::{BindPath, Binding, Bindings};
use jackdaw_scene_types::UiSceneRoot;

mod util;

/// The subject a bar reads. A real registered Rust type, which is the only
/// case the editor can preview.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Vitals {
    ratio: f32,
}

const NODE: &str = "bevy_ui::ui_node::Node";

/// An editor holding one UI scene whose fill bar binds `Node.width`, with
/// preview off. Returns the fill entity.
fn scene_with_a_bound_width() -> (App, Entity) {
    let mut app = util::editor_test_app();
    app.register_type::<Vitals>();

    let world = app.world_mut();
    let root = world
        .spawn((Name::new("UiRoot"), UiSceneRoot::default(), Node::default()))
        .id();
    let fill = world
        .spawn((
            Name::new("Fill"),
            Node::default(),
            ChildOf(root),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Vitals.ratio")],
                via: None,
                write: BindPath::new("Node.width"),
                as_percent: true,
            }]),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, root);
    jackdaw::scene_io::register_entity_in_ast(world, fill);
    // The operator is only available with a primary selection, so a test
    // that never selects would pass on the dispatch being refused.
    app.world_mut().resource_mut::<Selection>().entities = vec![fill];
    app.update();
    (app, fill)
}

fn remove_component(app: &mut App, entity: Entity, type_path: &str) {
    let outcome = app
        .world_mut()
        .operator("component.remove")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("entity", entity)
        .param("type_path", type_path.to_string())
        .call()
        .expect("dispatch");
    // The operator takes the call and queues the work either way, so this
    // says the dispatch was accepted. Without it, an unavailable operator
    // would look the same as the guard doing its job.
    assert!(
        matches!(outcome, OperatorResult::Finished),
        "the removal has to be dispatched before anything can refuse it",
    );
    app.update();
}

/// The guard and its other half in one test, so neither half can pass by
/// doing nothing: mid-preview the removal is refused, and with the session
/// over the same removal goes through.
#[test]
fn a_previewed_component_cannot_be_removed_until_the_session_ends() {
    let (mut app, fill) = scene_with_a_bound_width();

    preview_context::set_preview(app.world_mut(), true);
    app.update();
    remove_component(&mut app, fill, NODE);
    assert!(
        app.world().get::<Node>(fill).is_some(),
        "the preview owns the component it writes, so the header's X is refused",
    );

    preview_context::set_preview(app.world_mut(), false);
    app.update();
    remove_component(&mut app, fill, NODE);
    assert!(
        app.world().get::<Node>(fill).is_none(),
        "with the session over the same removal is an ordinary edit",
    );
}

/// A widget in the same scene whose binding drives its collider, carrying
/// the physics pair `physics.disable` would take off. Returns that entity.
fn scene_with_a_bound_collider() -> (App, Entity) {
    let mut app = util::editor_test_app();
    app.register_type::<Vitals>();

    let world = app.world_mut();
    let root = world
        .spawn((Name::new("UiRoot"), UiSceneRoot::default(), Node::default()))
        .id();
    let solid = world
        .spawn((
            Name::new("Solid"),
            Node::default(),
            avian3d::prelude::RigidBody::Static,
            AvianCollider::default(),
            ChildOf(root),
            Bindings(vec![Binding::Field {
                read: vec![BindPath::new("Vitals.ratio")],
                via: None,
                write: BindPath::new("AvianCollider.0"),
                as_percent: false,
            }]),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(world, root);
    jackdaw::scene_io::register_entity_in_ast(world, solid);
    app.world_mut().resource_mut::<Selection>().entities = vec![solid];
    app.update();
    (app, solid)
}

fn disable_physics(app: &mut App, entity: Entity) {
    let outcome = app
        .world_mut()
        .operator("physics.disable")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("entity", entity)
        .call()
        .expect("dispatch");
    assert!(
        matches!(outcome, OperatorResult::Finished),
        "the disable has to be dispatched before anything can refuse it",
    );
    app.update();
}

/// Both halves again: mid-preview the disable is refused with the
/// components left where they are, and with the session over the same call
/// takes them off.
#[test]
fn previewed_physics_cannot_be_disabled_until_the_session_ends() {
    let (mut app, solid) = scene_with_a_bound_collider();

    preview_context::set_preview(app.world_mut(), true);
    app.update();
    disable_physics(&mut app, solid);
    assert!(
        app.world().get::<AvianCollider>(solid).is_some(),
        "the preview owns the collider it writes, so the disable is refused",
    );
    assert!(
        app.world()
            .get::<avian3d::prelude::RigidBody>(solid)
            .is_some(),
        "and the rest of the bundle it would have taken with it stays too",
    );

    preview_context::set_preview(app.world_mut(), false);
    app.update();
    disable_physics(&mut app, solid);
    assert!(
        app.world().get::<AvianCollider>(solid).is_none(),
        "with the session over the same call is an ordinary edit",
    );
}
