//! A game refuses a scene carrying the retired facade UI vocabulary, for the
//! same reason the editor does: those types are not registered, so loading
//! would hand the player a scene missing its UI.
//!
//! Two gates, because a scene reaches the world two ways: the asset loader
//! reading a `.bsn` off disk, and the spawn pass, which also serves scenes
//! handed over as in-memory text through `JackdawScene::new`.

use bevy::asset::{AssetPlugin, LoadState};
use bevy::prelude::*;
use jackdaw_runtime::{
    JackdawCatalogPath, JackdawPlugin, JackdawScene, JackdawSceneRoot, SceneRefused,
};

const RETIRED: &str = r#"
bevy_ecs::hierarchy::Children [
    #Overlay
    jackdaw_ui::UiCanvas
    ,
    #World
    bevy_transform::components::transform::Transform
]
"#;

#[test]
fn the_game_loader_refuses_a_scene_holding_retired_ui_components() {
    let mut app = runtime_app();
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(RETIRED.into(), ".".into()));
    app.world_mut().spawn(JackdawSceneRoot(scene));

    app.update();
    app.update();

    let mut names = app.world_mut().query::<&Name>();
    let spawned: Vec<String> = names
        .iter(app.world())
        .map(|name| name.as_str().to_string())
        .collect();
    assert!(
        !spawned
            .iter()
            .any(|name| name == "Overlay" || name == "World"),
        "a refused scene spawns nothing at all, not the half that still parses: {spawned:?}"
    );
}

#[test]
fn a_scene_with_no_retired_components_still_spawns() {
    let mut app = runtime_app();
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(
            r#"
#World
bevy_transform::components::transform::Transform
"#
            .into(),
            ".".into(),
        ));
    app.world_mut().spawn(JackdawSceneRoot(scene));

    app.update();
    app.update();

    let mut names = app.world_mut().query::<&Name>();
    assert!(
        names.iter(app.world()).any(|name| name.as_str() == "World"),
        "the gate must not refuse an ordinary scene"
    );
}

/// A game names a scene file rather than building a `JackdawScene` by hand, so
/// the asset loader is the gate it meets. The loader fails the load rather
/// than handing back a scene, and the asset server reports the refusal by the
/// component's name.
#[test]
fn the_asset_loader_fails_a_scene_file_holding_retired_ui_components() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("retired.bsn"), RETIRED).expect("write the scene");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin {
        file_path: dir.path().to_string_lossy().into_owned(),
        ..default()
    });
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(JackdawPlugin);

    let handle: Handle<JackdawScene> = app.world().resource::<AssetServer>().load("retired.bsn");

    let mut settled = None;
    for _ in 0..200 {
        app.update();
        let state = app
            .world()
            .resource::<AssetServer>()
            .get_load_state(&handle)
            .expect("the asset server tracks the requested scene");
        if !matches!(state, LoadState::Loading | LoadState::NotLoaded) {
            settled = Some(state);
            break;
        }
    }
    let state = settled.expect("the load resolves");

    let LoadState::Failed(err) = state else {
        panic!("the loader must refuse the file, not load it: {state:?}");
    };
    let message = err.to_string();
    assert!(
        message.contains("jackdaw_ui::UiCanvas"),
        "the refusal names the component that is gone: {message}"
    );
}

/// Refusing is not the same as loading a scene that happens to hold nothing,
/// and a game that shows a screen when its scene arrives has to tell the two
/// apart. The marker is how: it says the loader would not spawn this, so the
/// game can fall back rather than wait for entities that are never coming.
#[test]
fn a_refused_scene_is_marked_so_a_game_can_tell() {
    let mut app = runtime_app();
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(RETIRED.into(), ".".into()));
    let root = app.world_mut().spawn(JackdawSceneRoot(scene)).id();

    app.update();
    app.update();

    assert!(
        app.world().get::<SceneRefused>(root).is_some(),
        "a scene the loader would not spawn says so on its root"
    );
}

/// A refusal describes one load attempt, not the root forever. When a
/// corrected document arrives through hot reload the scene spawns and the
/// marker goes; a marker left on a healthy scene would have the game showing
/// its fallback over content that is there.
#[test]
fn a_refusal_does_not_outlive_the_document_that_earned_it() {
    let mut app = runtime_app();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(RETIRED.into(), ".".into()));
    let root = app.world_mut().spawn(JackdawSceneRoot(handle.clone())).id();

    app.update();
    app.update();
    assert!(
        app.world().get::<SceneRefused>(root).is_some(),
        "the first attempt is refused, or this proves nothing"
    );

    // What hot reload does: the same asset, now holding a corrected document.
    if let Some(mut scene) = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .get_mut(&handle)
    {
        *scene = JackdawScene::new(
            "#World\nbevy_transform::components::transform::Transform\n".into(),
            ".".into(),
        );
    }
    app.update();
    app.update();

    let mut names = app.world_mut().query::<&Name>();
    assert!(
        names.iter(app.world()).any(|name| name.as_str() == "World"),
        "the fixed document spawns"
    );
    assert!(
        app.world().get::<SceneRefused>(root).is_none(),
        "and the refusal it replaced is gone with it"
    );
}

/// A scene named as a file fails in the asset loader, so no document reaches
/// the spawn pass. Without the marker the root would sit with no children and
/// nothing to say why.
#[test]
fn a_scene_file_the_asset_loader_refuses_marks_its_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("retired.bsn"), RETIRED).expect("write the scene");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin {
        file_path: dir.path().to_string_lossy().into_owned(),
        ..default()
    });
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.add_plugins(JackdawPlugin);

    let handle: Handle<JackdawScene> = app.world().resource::<AssetServer>().load("retired.bsn");
    let root = app.world_mut().spawn(JackdawSceneRoot(handle)).id();

    for _ in 0..200 {
        app.update();
        if app.world().get::<SceneRefused>(root).is_some() {
            break;
        }
    }

    assert!(
        app.world().get::<SceneRefused>(root).is_some(),
        "a scene whose file the loader refused says so on its root too"
    );
}

/// A scene the spawn pass cannot find is usually one still on its way, and
/// marking those would have every game showing its fallback for the frames
/// before its screen arrives. Only a load the asset server has given up on is
/// a refusal.
///
/// The fixture holds a handle whose asset is not in the store, the branch a
/// still-loading file takes on every pass, so a mark made on the miss rather
/// than on the failure shows up at once.
#[test]
fn a_scene_whose_asset_has_not_arrived_is_not_marked_refused() {
    let mut app = runtime_app();
    let handle = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .reserve_handle();
    let root = app.world_mut().spawn(JackdawSceneRoot(handle.clone())).id();

    app.update();
    app.update();
    assert!(
        app.world().get::<SceneRefused>(root).is_none(),
        "a scene still on its way must not read as one that was refused"
    );

    app.world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .insert(
            handle.id(),
            JackdawScene::new(
                "#World\nbevy_transform::components::transform::Transform\n".into(),
                ".".into(),
            ),
        )
        .expect("the reserved handle takes its asset");
    app.update();
    app.update();

    let mut names = app.world_mut().query::<&Name>();
    assert!(
        names.iter(app.world()).any(|name| name.as_str() == "World"),
        "and the scene it was waiting for spawned"
    );
    assert!(
        app.world().get::<SceneRefused>(root).is_none(),
        "with no refusal left on it"
    );
}

/// A scene that carried nothing to spawn was still loaded, and must not read
/// as refused.
#[test]
fn a_scene_that_spawns_nothing_is_not_marked_refused() {
    let mut app = runtime_app();
    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(
            "// jackdaw 0.19.0 | bevy 0.19\n".into(),
            ".".into(),
        ));
    let root = app.world_mut().spawn(JackdawSceneRoot(scene)).id();

    app.update();
    app.update();

    assert!(
        app.world().get::<SceneRefused>(root).is_none(),
        "an empty scene is a scene that loaded, not one that was refused"
    );
}

/// A prefab base carrying the retired vocabulary hands it to an instance whose
/// own document never names it, so a gate reading the document as authored
/// sees a clean one and the component arrives on the way to the spawn.
#[test]
fn a_scene_inheriting_retired_ui_components_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("panel.bsn"),
        "#Panel\njackdaw_ui::UiCanvas\nbevy_transform::components::transform::Transform\n",
    )
    .expect("write the prefab");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(AssetPlugin::default());
    app.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
    app.insert_resource(JackdawCatalogPath(dir.path().join("catalog.bsn")));
    app.add_plugins(JackdawPlugin);

    let scene = app
        .world_mut()
        .resource_mut::<Assets<JackdawScene>>()
        .add(JackdawScene::new(
            "jackdaw::prefab::components::IsA { source: \"panel.bsn\", deleted: [] }\n\
             jackdaw::prefab::components::PrefabEntityId(0)\n"
                .into(),
            std::path::PathBuf::new(),
        ));
    let root = app.world_mut().spawn(JackdawSceneRoot(scene)).id();

    app.update();
    app.update();

    let mut names = app.world_mut().query::<&Name>();
    let spawned: Vec<String> = names
        .iter(app.world())
        .map(|name| name.as_str().to_string())
        .collect();
    assert!(
        !spawned.iter().any(|name| name == "Panel"),
        "an inherited facade component is refused, not applied: {spawned:?}"
    );
    assert!(
        app.world().get::<SceneRefused>(root).is_some(),
        "and the refusal is marked, like any other"
    );
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
