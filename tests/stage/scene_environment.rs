//! The sky, fog, ambient light and grading a scene carries.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::{Environment, FogMode, PropertyValue, Tonemapper};

use crate::util;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/definition_project")
}

fn settle(app: &mut App) {
    for _ in 0..6 {
        app.update();
    }
}

#[track_caller]
fn call(
    app: &mut App,
    id: &'static str,
    params: &[(&'static str, PropertyValue)],
) -> OperatorResult {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    for (name, value) in params {
        call = call.param(*name, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    settle(app);
    result
}

/// An editor with a project of its own and one scene open.
fn editor_on_a_scene() -> (App, tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    std::fs::copy(
        fixture_dir().join("jackdaw.toml"),
        tmp.path().join("jackdaw.toml"),
    )
    .expect("the manifest copies");
    std::fs::create_dir_all(tmp.path().join("assets")).expect("an assets folder");

    let mut app = util::editor_test_app();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    settle(&mut app);

    let scene = tmp.path().join("assets/valley.bsn");
    std::fs::write(
        &scene,
        "#valley\nbevy_transform::components::transform::Transform\n",
    )
    .expect("the scene is written");
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    settle(&mut app);
    (app, tmp, scene)
}

fn named(app: &mut App, name: &str) -> Option<Entity> {
    let found: Vec<Entity> = app
        .world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .filter(|(_, spawned)| spawned.as_str() == name)
        .map(|(entity, _)| entity)
        .collect();
    found.into_iter().next_back()
}

/// Give the scene's root an unedited environment, in the document as well as the world.
fn an_environment(app: &mut App) -> Entity {
    let valley = named(app, "valley").expect("the scene spawned its root");
    let environment = Environment::default();
    app.world_mut()
        .entity_mut(valley)
        .insert(environment.clone());
    jackdaw::commands::sync_component_to_ast(
        app.world_mut(),
        valley,
        "jackdaw_scene_types::environment::Environment",
        &environment,
    );
    settle(app);
    valley
}

fn environment(app: &App, entity: Entity) -> Environment {
    app.world()
        .get::<Environment>(entity)
        .cloned()
        .expect("the scene's environment")
}

#[test]
fn the_environment_operator_sets_fields_by_path_and_undo_puts_them_back() {
    let (mut app, _tmp, _scene) = editor_on_a_scene();
    let valley = an_environment(&mut app);
    let before = environment(&app, valley);

    let result = call(
        &mut app,
        "environment.set",
        &[
            ("fog.mode", "ExponentialSquared".into()),
            ("fog.density", 0.003.into()),
            ("fog.color", "0.16,0.46,0.59".into()),
            ("sky.enabled", true.into()),
            ("post.tonemapper", "AcesFitted".into()),
        ],
    );
    assert_eq!(result, OperatorResult::Finished);

    let after = environment(&app, valley);
    assert_eq!(after.fog.mode, FogMode::ExponentialSquared);
    assert_eq!(after.fog.density, 0.003);
    assert_eq!(after.fog.color, Color::srgb(0.16, 0.46, 0.59));
    assert!(after.sky.enabled);
    assert_eq!(after.post.tonemapper, Tonemapper::AcesFitted);
    assert_eq!(
        after.ambient, before.ambient,
        "and the groups the call left out are untouched",
    );

    assert_eq!(
        call(&mut app, "history.undo", &[]),
        OperatorResult::Finished
    );
    assert_eq!(
        environment(&app, valley),
        before,
        "one undo takes every field back"
    );
}

#[test]
fn the_environment_operator_refuses_a_field_it_does_not_have() {
    let (mut app, _tmp, _scene) = editor_on_a_scene();
    let valley = an_environment(&mut app);
    let before = environment(&app, valley);

    let result = call(
        &mut app,
        "environment.set",
        &[("fog.density", 0.2.into()), ("fog.thickness", 2.0.into())],
    );

    assert_eq!(result, OperatorResult::Cancelled);
    assert_eq!(
        environment(&app, valley),
        before,
        "and sets none of the call"
    );
}

#[test]
fn a_scene_saved_with_an_environment_reloads_with_it() {
    let (mut app, _tmp, scene) = editor_on_a_scene();
    an_environment(&mut app);
    let result = call(
        &mut app,
        "environment.set",
        &[
            ("fog.mode", "Linear".into()),
            ("fog.end", 800.0.into()),
            ("ambient.mode", "Trilight".into()),
            ("ambient.sky", "0.62,0.64,0.66".into()),
            ("post.enabled", true.into()),
            ("post.bloom_intensity", 0.2.into()),
        ],
    );
    assert_eq!(result, OperatorResult::Finished);
    let authored = {
        let valley = named(&mut app, "valley").expect("the scene's root");
        environment(&app, valley)
    };

    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the scene saves"
    );
    let text = std::fs::read_to_string(&scene).expect("the scene is on disk");
    assert!(
        text.contains("Environment"),
        "the document carries the environment, got\n{text}",
    );

    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    settle(&mut app);
    let reopened = named(&mut app, "valley").expect("the scene spawned its root again");
    assert_eq!(environment(&app, reopened), authored);
}
