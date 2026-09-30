//! Children spawned into reused entity indices take their parents'
//! transforms even when a frame despawns thousands of entities, with
//! static transform optimizations on.

use bevy::prelude::*;
use jackdaw_runtime::JackdawPlugin;

const PARENTS: usize = 2048;

#[derive(Component)]
struct Holder;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(JackdawPlugin);
    app
}

fn stale_children(world: &mut World) -> usize {
    let mut parents = world.query_filtered::<(&GlobalTransform, &Children), With<Holder>>();
    let pairs: Vec<(Vec3, Entity)> = parents
        .iter(world)
        .map(|(global, children)| (global.translation(), children[0]))
        .collect();
    pairs
        .into_iter()
        .filter(|(parent, child)| {
            world.get::<GlobalTransform>(*child).unwrap().translation() != *parent
        })
        .count()
}

#[test]
fn children_spawned_into_reused_indices_follow_their_parents() {
    let mut app = app();
    assert!(
        app.world()
            .resource::<bevy::transform::StaticTransformOptimizations>()
            .is_enabled()
    );
    for i in 0..PARENTS {
        app.world_mut().spawn((
            Holder,
            Transform::from_xyz(i as f32 + 1.0, 5.0, 0.0),
            children![Transform::IDENTITY],
        ));
    }
    app.update();
    app.update();
    assert_eq!(stale_children(app.world_mut()), 0);

    let world = app.world_mut();
    let mut parents = world.query_filtered::<(Entity, &Children), With<Holder>>();
    let pairs: Vec<(Entity, Entity)> = parents
        .iter(world)
        .map(|(parent, children)| (parent, children[0]))
        .collect();
    for (parent, child) in pairs {
        world.despawn(child);
        world.spawn((Transform::IDENTITY, ChildOf(parent)));
    }
    for _ in 0..3 {
        app.update();
    }

    assert_eq!(stale_children(app.world_mut()), 0);
}
