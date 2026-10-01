//! A placed model's collider sits on its meshes, which are its descendants,
//! and a change of shape leaves one set of the new shape behind.

use avian3d::prelude::*;
use bevy::asset::AssetPlugin;
use bevy::prelude::*;
use jackdaw_avian_integration::{AvianCollider, ModelCollidersPlugin};

fn app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        TransformPlugin,
        AssetPlugin::default(),
        bevy::world_serialization::WorldSerializationPlugin,
    ));
    app.init_asset::<Mesh>();
    app.add_plugins((PhysicsPlugins::default(), ModelCollidersPlugin));
    app.finish();
    app.cleanup();
    app
}

/// A model root holding one node that holds one mesh, the way a glTF scene
/// instance nests its parts.
fn model(app: &mut App, shape: ColliderConstructor) -> (Entity, Entity) {
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Mesh::from(Cuboid::new(1.0, 2.0, 1.0)));
    let root = app
        .world_mut()
        .spawn((Transform::default(), AvianCollider(shape)))
        .id();
    let node = app
        .world_mut()
        .spawn((Name::new("Rock"), Transform::default(), ChildOf(root)))
        .id();
    let part = app
        .world_mut()
        .spawn((
            Name::new("Rock.Mesh"),
            Transform::default(),
            Mesh3d(mesh),
            ChildOf(node),
        ))
        .id();
    settle(app);
    (root, part)
}

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn set_shape(app: &mut App, root: Entity, shape: ColliderConstructor) {
    app.world_mut()
        .entity_mut(root)
        .insert(AvianCollider(shape));
    settle(app);
}

fn colliders(app: &mut App) -> Vec<(Entity, Collider)> {
    let mut query = app.world_mut().query::<(Entity, &Collider)>();
    query
        .iter(app.world())
        .map(|(entity, collider)| (entity, collider.clone()))
        .collect()
}

#[test]
fn a_mesh_shape_builds_a_collider_on_the_models_mesh() {
    let mut app = app();
    let (root, part) = model(&mut app, ColliderConstructor::TrimeshFromMesh);

    let built = colliders(&mut app);
    assert_eq!(built.len(), 1, "one collider for the one mesh");
    assert_eq!(built[0].0, part);
    assert!(built[0].1.shape().as_trimesh().is_some());
    assert!(app.world().get::<Collider>(root).is_none());
}

#[test]
fn switching_shape_leaves_one_set_of_the_new_shape() {
    let mut app = app();
    let (root, part) = model(&mut app, ColliderConstructor::TrimeshFromMesh);

    set_shape(&mut app, root, ColliderConstructor::ConvexHullFromMesh);
    let built = colliders(&mut app);
    assert_eq!(built.len(), 1);
    assert_eq!(built[0].0, part);
    assert!(built[0].1.shape().as_convex_polyhedron().is_some());

    set_shape(
        &mut app,
        root,
        ColliderConstructor::Cuboid {
            x_length: 1.0,
            y_length: 2.0,
            z_length: 1.0,
        },
    );
    let built = colliders(&mut app);
    assert_eq!(built.len(), 1, "a primitive shape sits on the model alone");
    assert_eq!(built[0].0, root);
    assert!(built[0].1.shape().as_cuboid().is_some());

    set_shape(&mut app, root, ColliderConstructor::TrimeshFromMesh);
    let built = colliders(&mut app);
    assert_eq!(built.len(), 1);
    assert_eq!(built[0].0, part);
}

#[test]
fn removing_the_collider_takes_the_models_colliders_with_it() {
    let mut app = app();
    let (root, _) = model(&mut app, ColliderConstructor::TrimeshFromMesh);

    app.world_mut().entity_mut(root).remove::<AvianCollider>();
    settle(&mut app);

    assert!(colliders(&mut app).is_empty());
}

#[test]
fn parts_that_arrive_after_the_collider_get_it_too() {
    let mut app = app();
    let root = app
        .world_mut()
        .spawn((
            Transform::default(),
            AvianCollider(ColliderConstructor::ConvexHullFromMesh),
        ))
        .id();
    settle(&mut app);
    let mesh = app
        .world_mut()
        .resource_mut::<Assets<Mesh>>()
        .add(Mesh::from(Cuboid::new(1.0, 1.0, 1.0)));
    let part = app
        .world_mut()
        .spawn((Transform::default(), Mesh3d(mesh), ChildOf(root)))
        .id();
    settle(&mut app);

    let built = colliders(&mut app);
    assert_eq!(built.len(), 1);
    assert_eq!(built[0].0, part);
}
