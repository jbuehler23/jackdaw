//! Reflection probes: what a new one carries, and what a baked one saves.

use std::path::{Path, PathBuf};

use bevy::light::LightProbe;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::{PropertyValue, ReflectionProbe};

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

fn the_probe(app: &mut App) -> Entity {
    let mut probes = app
        .world_mut()
        .query_filtered::<Entity, With<ReflectionProbe>>();
    let found: Vec<Entity> = probes.iter(app.world()).collect();
    assert_eq!(found.len(), 1, "one probe in the scene");
    found[0]
}

#[test]
fn a_new_probe_reflects_nothing_until_it_is_baked() {
    let (mut app, _tmp, scene) = editor_on_a_scene();
    assert_eq!(
        call(&mut app, "entity.add.reflection_probe", &[]),
        OperatorResult::Finished
    );
    let probe = the_probe(&mut app);
    assert_eq!(
        app.world().get::<ReflectionProbe>(probe),
        Some(&ReflectionProbe::default())
    );
    let mut light_probes = app.world_mut().query::<&LightProbe>();
    assert_eq!(light_probes.iter(app.world()).count(), 0);

    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the scene saves"
    );
    let text = std::fs::read_to_string(&scene).expect("the scene is on disk");
    assert!(text.contains("ReflectionProbe"), "got\n{text}");
    assert!(
        !text.contains("EnvironmentMapLight") && !text.contains("LightProbe"),
        "nothing baked or generated is saved, got\n{text}"
    );
}

/// The scene names a bake by path and carries nothing generated from it; the
/// cubemap and the light probe are rebuilt from that path on load.
#[test]
fn a_baked_probe_keeps_its_bake_across_a_save_and_reopen() {
    let (mut app, _tmp, scene) = editor_on_a_scene();
    assert_eq!(
        call(&mut app, "entity.add.reflection_probe", &[]),
        OperatorResult::Finished
    );
    let probe = the_probe(&mut app);
    let set = call(
        &mut app,
        "field.set",
        &[
            ("entity", PropertyValue::Entity(probe)),
            (
                "type_path",
                "jackdaw_scene_types::types::ReflectionProbe".into(),
            ),
            ("field", "baked".into()),
            ("value", "\"valley/lake.probe.hdr\"".into()),
        ],
    );
    assert_eq!(set, OperatorResult::Finished);
    assert!(
        jackdaw::scene_io::save_scene(app.world_mut()),
        "the scene saves"
    );

    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    settle(&mut app);
    let reopened = the_probe(&mut app);
    assert_eq!(
        app.world()
            .get::<ReflectionProbe>(reopened)
            .map(|probe| probe.baked.as_str()),
        Some("valley/lake.probe.hdr")
    );
    let text = std::fs::read_to_string(&scene).expect("the scene is on disk");
    assert!(
        text.contains("valley/lake.probe.hdr")
            && !text.contains("LightProbe")
            && !text.contains("EnvironmentMapLight"),
        "the scene names the bake by path and saves nothing generated, got\n{text}"
    );
}
