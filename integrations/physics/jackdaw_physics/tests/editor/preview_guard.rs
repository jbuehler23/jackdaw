//! What `physics.disable` refuses while a preview session is running: a
//! preview evaluator owns the components it writes, so taking them off
//! would leave the session driving a component that is gone.

use crate::util;

use bevy::prelude::*;
use jackdaw::preview_context;
use jackdaw::selection::Selection;
use jackdaw_api::prelude::*;
use jackdaw_avian_integration::AvianCollider;
use jackdaw_bind::{BindPath, Binding, Bindings};
use jackdaw_scene_types::UiSceneRoot;

/// The subject a bar reads. A real registered Rust type, which is the only
/// case the editor can preview.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Vitals {
    ratio: f32,
}

/// A widget in the same scene whose binding drives its collider, carrying
/// the physics pair `physics.disable` would take off. Returns that entity.
fn scene_with_a_bound_collider() -> (App, Entity) {
    let mut app = util::editor_app();
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

/// A previewed collider is the preview's until the session ends.
#[test]
fn previewed_physics_cannot_be_disabled_until_the_session_ends() {
    let (mut app, solid) = scene_with_a_bound_collider();

    preview_context::set_preview(app.world_mut(), true);
    app.update();
    disable_physics(&mut app, solid);
    assert!(
        app.world().get::<AvianCollider>(solid).is_some(),
        "the preview owns the collider it writes, so its removal is refused",
    );

    preview_context::set_preview(app.world_mut(), false);
    app.update();
    disable_physics(&mut app, solid);
    assert!(
        app.world().get::<AvianCollider>(solid).is_none(),
        "with the session over the same call is an ordinary edit",
    );
}
