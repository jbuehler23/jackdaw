//! A game runtime evaluates the bindings its scenes were authored with.
//!
//! The editor registers the binding pieces itself and only evaluates in
//! preview; a game gets the whole plugin from `JackdawPlugin`, so a scene
//! loaded through the runtime loader arrives already wired.

use bevy::prelude::*;
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};

#[derive(Component, Reflect, Default)]
#[reflect(Component)]
struct Bar {
    fill: f32,
}

#[derive(Resource, Reflect, Default)]
#[reflect(Resource)]
struct Score {
    points: f32,
}

/// A widget whose only authored connection to the game is a binding. The
/// source is a resource so the document needs no cross-entity reference,
/// which the runtime spawn does not remap.
const SCENE: &str = r#"
bevy_ecs::hierarchy::Children [
    #Bar
    runtime_bindings::Bar { fill: 0.0 }
    jackdaw_bind::types::Bindings([
        jackdaw_bind::types::Binding::Field { read: [jackdaw_bind::types::BindPath { raw: "Res(runtime_bindings::Score).points" }], via: core::option::Option::None, write: jackdaw_bind::types::BindPath { raw: "runtime_bindings::Bar.fill" }, as_percent: false },
    ])
]
"#;

#[test]
fn a_scene_loaded_by_the_game_runtime_evaluates_its_bindings() {
    let mut app = runtime_app();
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(SCENE.into(), ".".into()));
    app.world_mut().spawn(JackdawSceneRoot(scene));

    app.update();
    app.update();

    let bar = named_entity(app.world_mut(), "Bar");
    assert!(
        app.world().get::<Bar>(bar).is_some(),
        "the authored widget component loaded"
    );

    app.world_mut().resource_mut::<Score>().points = 7.5;
    app.update();

    assert_eq!(
        app.world().get::<Bar>(bar).map(|b| b.fill),
        Some(7.5),
        "the game runtime evaluates the binding: the target follows its source"
    );
}

fn runtime_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(JackdawPlugin);
    app.register_type::<Bar>();
    app.register_type::<Score>();
    app.init_resource::<Score>();
    app
}

fn named_entity(world: &mut World, target: &str) -> Entity {
    let mut names = world.query::<(Entity, &Name)>();
    names
        .iter(world)
        .find_map(|(entity, name)| (name.as_str() == target).then_some(entity))
        .unwrap_or_else(|| panic!("expected entity named {target}"))
}
