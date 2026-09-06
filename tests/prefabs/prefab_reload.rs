//! Reloading a prefab that changed on disk.
//!
//! The instances of the changed file pick up its new values; everything
//! else in the scene keeps the entity it already had, so an id a panel,
//! a selection or a caller is holding still means what it did.

use bevy::prelude::*;

const PREFAB_V1: &str = r#"#Rock
bevy_transform::components::transform::Transform
Children [
    #RockBody
    bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 0.0, y: 1.0, z: 0.0 }, rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }, scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 } }
]
"#;

const PREFAB_V2: &str = r#"#Rock
bevy_transform::components::transform::Transform
Children [
    #RockBody
    bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 0.0, y: 4.0, z: 0.0 }, rotation: glam::Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }, scale: glam::Vec3 { x: 1.0, y: 1.0, z: 1.0 } }
]
"#;

fn make_app() -> App {
    use bevy::render::RenderPlugin;
    use bevy::render::settings::{RenderCreation, WgpuSettings};
    use bevy::winit::WinitPlugin;

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(RenderPlugin {
                render_creation: RenderCreation::Automatic(Box::new(WgpuSettings {
                    backends: None,
                    ..default()
                })),
                ..default()
            })
            .disable::<WinitPlugin>(),
    );
    app.add_plugins(jackdaw_scene_types::SceneTypesPlugin::default());
    app.add_plugins(jackdaw_bsn::JackdawBsnPlugin);
    app.add_plugins(jackdaw::prefab::PrefabPlugin);
    app.init_resource::<jackdaw::commands::CommandHistory>();
    app.init_resource::<jackdaw::scene_io::SceneFilePath>();
    app.init_resource::<jackdaw::scene_io::SceneDirtyState>();
    app.init_resource::<jackdaw::selection::Selection>();
    app.init_resource::<jackdaw::scenes::Scenes>();
    app
}

fn entity_named(app: &mut App, name: &str) -> Option<Entity> {
    let mut query = app.world_mut().query::<(Entity, &Name)>();
    query
        .iter(app.world())
        .find(|(_, found)| found.as_str() == name)
        .map(|(entity, _)| entity)
}

/// Take the file's new contents into the cache the way the watcher does,
/// then reload the instances that read it.
fn reload(
    app: &mut App,
    path: &std::path::Path,
    text: &str,
) -> jackdaw::prefab::watcher::PrefabReload {
    // The sparse capture compares against the baseline the scene was last
    // resolved with, so it has to happen before the cache moves on.
    let sparse = jackdaw::prefab::watcher::capture_sparse_scene_text(app.world_mut())
        .expect("the scene has a live document");
    std::fs::write(path, text).expect("write the new prefab");
    let ast = jackdaw::prefab::save_load::read_prefab_ast(path).expect("the new prefab parses");
    app.world_mut()
        .resource_mut::<jackdaw::prefab::PrefabAstCache>()
        .insert(path, ast);
    // No frame in between: the reload applies its patches itself, and a tick
    // would let the watcher's own debounce fire for the write above.
    jackdaw::prefab::watcher::reload_instances_of(app.world_mut(), &sparse, path)
}

#[test]
fn a_prefab_reload_leaves_the_rest_of_the_scene_alone() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("rock.bsn");
    std::fs::write(&path, PREFAB_V1).expect("write the prefab");

    let mut app = make_app();
    let unrelated = app.world_mut().spawn(Name::new("Beacon")).id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), unrelated);
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &path, Vec3::ZERO);
    app.update();

    // Spawning the instance respawns the scene, so the id to hold is the one
    // the document settled on.
    let beacon = entity_named(&mut app, "Beacon").expect("the unrelated entity is in the scene");
    let body = entity_named(&mut app, "RockBody").expect("the inherited child spawned");
    assert_eq!(
        app.world()
            .get::<Transform>(body)
            .map(|at| at.translation.y),
        Some(1.0),
        "the instance starts at the file's first value"
    );

    assert!(
        matches!(
            reload(&mut app, &path, PREFAB_V2),
            jackdaw::prefab::watcher::PrefabReload::Instances
        ),
        "an instance at the top of the document reloads on its own"
    );

    assert!(
        app.world().get_entity(beacon).is_ok(),
        "an entity that has nothing to do with the prefab keeps its id"
    );
    assert_eq!(
        app.world().get::<Name>(beacon).map(Name::as_str),
        Some("Beacon"),
        "and keeps what it was"
    );

    let body = entity_named(&mut app, "RockBody").expect("the inherited child respawned");
    assert_eq!(
        app.world()
            .get::<Transform>(body)
            .map(|at| at.translation.y),
        Some(4.0),
        "the instance picked up the file's new value"
    );
}

const OUTER: &str = r#"#Outer
bevy_transform::components::transform::Transform
Children [
    #InnerHolder
    jackdaw::prefab::components::IsA { source: "inner.bsn", deleted: [] }
]
"#;

/// A prefab another prefab references has no `IsA` node of its own in the
/// scene, so nothing here matches the file that changed. The reload has to
/// say so rather than report the file handled, or editing the inner prefab
/// changes nothing anyone can see.
#[test]
fn a_prefab_only_another_prefab_reads_falls_back_to_the_whole_scene() {
    let tmp = tempfile::tempdir().unwrap();
    let inner = tmp.path().join("inner.bsn");
    let outer = tmp.path().join("outer.bsn");
    std::fs::write(&inner, PREFAB_V1).expect("write the inner prefab");
    std::fs::write(&outer, OUTER).expect("write the outer prefab");

    let mut app = make_app();
    jackdaw::prefab::operators::spawn_instance(app.world_mut(), &outer, Vec3::ZERO);
    app.update();

    assert!(
        matches!(
            reload(&mut app, &inner, PREFAB_V2),
            jackdaw::prefab::watcher::PrefabReload::Unhandled
        ),
        "a file no instance names directly has to fall back to the whole scene"
    );
}
