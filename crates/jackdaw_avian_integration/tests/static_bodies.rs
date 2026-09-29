//! A collider with no body stands as a static body, and the body it was given
//! goes with the collider unless someone has made it their own.

use avian3d::prelude::*;
use bevy::prelude::*;
use jackdaw_avian_integration::{AvianCollider, ImpliedStaticBody, StaticCollidersPlugin};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StaticCollidersPlugin));
    app
}

fn settle(app: &mut App) {
    for _ in 0..3 {
        app.update();
    }
}

fn collider(app: &mut App) -> Entity {
    let entity = app
        .world_mut()
        .spawn((Transform::default(), AvianCollider::default()))
        .id();
    settle(app);
    entity
}

#[test]
fn a_collider_without_a_body_stands_as_a_static_body() {
    let mut app = app();
    let entity = collider(&mut app);

    assert_eq!(
        app.world().get::<RigidBody>(entity),
        Some(&RigidBody::Static)
    );
}

#[test]
fn removing_the_collider_takes_its_static_body_with_it() {
    let mut app = app();
    let entity = collider(&mut app);

    app.world_mut().entity_mut(entity).remove::<AvianCollider>();
    settle(&mut app);

    assert!(app.world().get::<RigidBody>(entity).is_none());
    assert!(app.world().get::<ImpliedStaticBody>(entity).is_none());
}

#[test]
fn removing_the_collider_leaves_an_authored_body() {
    let mut app = app();
    let entity = app
        .world_mut()
        .spawn((
            Transform::default(),
            AvianCollider::default(),
            RigidBody::Static,
        ))
        .id();
    settle(&mut app);
    assert!(app.world().get::<ImpliedStaticBody>(entity).is_none());

    app.world_mut().entity_mut(entity).remove::<AvianCollider>();
    settle(&mut app);

    assert_eq!(
        app.world().get::<RigidBody>(entity),
        Some(&RigidBody::Static)
    );
}

#[test]
fn a_static_body_switched_to_dynamic_stays_when_the_collider_goes() {
    let mut app = app();
    let entity = collider(&mut app);

    app.world_mut()
        .entity_mut(entity)
        .insert(RigidBody::Dynamic);
    settle(&mut app);
    app.world_mut().entity_mut(entity).remove::<AvianCollider>();
    settle(&mut app);

    assert_eq!(
        app.world().get::<RigidBody>(entity),
        Some(&RigidBody::Dynamic)
    );
}

#[test]
fn a_collider_under_a_body_belongs_to_that_body() {
    let mut app = app();
    let body = app
        .world_mut()
        .spawn((Transform::default(), RigidBody::Dynamic))
        .id();
    let part = app
        .world_mut()
        .spawn((
            Transform::default(),
            AvianCollider::default(),
            ChildOf(body),
        ))
        .id();
    let nested = app
        .world_mut()
        .spawn((Transform::default(), AvianCollider::default()))
        .id();
    let nested_part = app
        .world_mut()
        .spawn((
            Transform::default(),
            AvianCollider::default(),
            ChildOf(nested),
        ))
        .id();
    settle(&mut app);

    assert!(app.world().get::<RigidBody>(part).is_none());
    assert_eq!(
        app.world().get::<RigidBody>(nested),
        Some(&RigidBody::Static)
    );
    assert!(app.world().get::<RigidBody>(nested_part).is_none());
}
