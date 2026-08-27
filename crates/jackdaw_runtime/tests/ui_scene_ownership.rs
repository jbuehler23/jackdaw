use bevy::prelude::*;
use jackdaw_runtime::{JackdawPlugin, JackdawScene, JackdawSceneRoot};
use jackdaw_scene_types::UiSceneRoot;

#[test]
fn ui_root_is_an_ecs_root_but_is_destroyed_with_its_scene() {
    let mut app = runtime_app();
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(
            r#"
bevy_ecs::hierarchy::Children [
    #World
    bevy_transform::components::transform::Transform
    ,
    #Overlay
    jackdaw_scene_types::UiSceneRoot
]
"#
            .into(),
            ".".into(),
        ));
    let scene_root = app.world_mut().spawn(JackdawSceneRoot(scene)).id();

    app.update();
    app.update();

    let world_root = named_entity(app.world_mut(), "World");
    let ui_root = named_entity(app.world_mut(), "Overlay");
    assert_eq!(
        app.world().get::<ChildOf>(world_root).map(ChildOf::parent),
        Some(scene_root),
        "ordinary roots retain the runtime hierarchy"
    );
    assert!(
        app.world().get::<UiSceneRoot>(ui_root).is_some(),
        "the authored UI root component loaded"
    );
    assert!(
        app.world().get::<ChildOf>(ui_root).is_none(),
        "a UI scene root must be a real ECS root for Bevy layout"
    );

    app.world_mut().entity_mut(scene_root).despawn();
    app.update();
    app.update();

    assert!(app.world().get_entity(world_root).is_err());
    assert!(
        app.world().get_entity(ui_root).is_err(),
        "an unparented UI root is still owned and cleaned up by the scene"
    );
}

#[test]
fn destroying_one_scene_does_not_destroy_another_scenes_ui_root() {
    let mut app = runtime_app();
    let first_scene = scene_with_ui_root(&mut app, "FirstUiRoot");
    let second_scene = scene_with_ui_root(&mut app, "SecondUiRoot");
    let first_root = app.world_mut().spawn(JackdawSceneRoot(first_scene)).id();
    app.world_mut().spawn(JackdawSceneRoot(second_scene));

    app.update();
    app.update();

    let first_ui_root = named_entity(app.world_mut(), "FirstUiRoot");
    let second_ui_root = named_entity(app.world_mut(), "SecondUiRoot");
    app.world_mut().entity_mut(first_root).despawn();
    app.update();
    app.update();

    assert!(app.world().get_entity(first_ui_root).is_err());
    assert!(
        app.world().get_entity(second_ui_root).is_ok(),
        "scene membership must be isolated per JackdawSceneRoot"
    );
}

fn scene_with_ui_root(app: &mut App, name: &str) -> Handle<JackdawScene> {
    app.world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(
            format!(
                r#"
bevy_ecs::hierarchy::Children [
    #{name}
    jackdaw_scene_types::UiSceneRoot
]
"#
            ),
            ".".into(),
        ))
}

fn runtime_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(JackdawPlugin);
    app
}

fn named_entity(world: &mut World, target: &str) -> Entity {
    let mut names = world.query::<(Entity, &Name)>();
    names
        .iter(world)
        .find_map(|(entity, name)| (name.as_str() == target).then_some(entity))
        .unwrap_or_else(|| panic!("expected entity named {target}"))
}
