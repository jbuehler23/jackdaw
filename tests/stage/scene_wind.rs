//! The wind a whole scene blows by, and the per-layer wind it was folded from.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::{DetailLayer, PropertyValue, SceneWind, Terrain, Wind};

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
fn call(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    for (name, value) in params {
        call = call.param(*name, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "{id} ran");
    settle(app);
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

    let scene = tmp.path().join("assets/meadow.bsn");
    std::fs::write(
        &scene,
        "#ground\nbevy_transform::components::transform::Transform\n",
    )
    .expect("the scene is written");
    jackdaw::scenes::operators::scene_open_system(app.world_mut(), &scene);
    settle(&mut app);
    (app, tmp, scene)
}

/// The one entity the scene named, if it spawned one.
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

/// A layer as scenes written before the scene carried its own wind hold one.
fn a_layer_with_its_own_wind(name: &str, strength: f32) -> DetailLayer {
    DetailLayer {
        name: name.to_string(),
        wind_strength: strength,
        wind_speed: 0.4,
        wind_tile_size: 7.0,
        wind_direction: [0.0, 1.0],
        ..DetailLayer::default()
    }
}

fn a_terrain_with_old_wind(app: &mut App, layers: Vec<DetailLayer>) -> Entity {
    let ground = named(app, "ground").expect("the scene spawned the ground");
    app.world_mut().entity_mut(ground).insert(Terrain {
        detail: layers,
        ..Terrain::default()
    });
    settle(app);
    ground
}

#[test]
fn a_scene_with_per_layer_wind_loads_with_a_wind_that_reproduces_it() {
    let (mut app, _tmp, _scene) = editor_on_a_scene();
    let ground = a_terrain_with_old_wind(&mut app, vec![a_layer_with_its_own_wind("grass", 0.24)]);

    let blowing = app
        .world()
        .get::<Wind>(ground)
        .copied()
        .expect("the scene gained the wind its layer was blowing");
    assert_eq!(blowing.direction, 90.0);
    assert_eq!(blowing.gust_speed, 0.4);
    assert_eq!(blowing.turbulence_scale, 7.0);

    let layer = app
        .world()
        .get::<Terrain>(ground)
        .expect("the terrain")
        .detail[0]
        .clone();
    assert_eq!(
        layer.wind_response, 2.0,
        "and the strength it leaned with is now how far it goes with that wind",
    );
    assert_eq!(
        layer.wind_strength,
        DetailLayer::default().wind_strength,
        "with the legacy field back at the default BSN elides",
    );
}

#[test]
fn two_layers_that_blew_apart_come_back_under_one_wind() {
    let (mut app, _tmp, _scene) = editor_on_a_scene();
    let ground = a_terrain_with_old_wind(
        &mut app,
        vec![
            a_layer_with_its_own_wind("grass", 0.12),
            a_layer_with_its_own_wind("reeds", 0.36),
        ],
    );

    let winds: Vec<Wind> = app
        .world_mut()
        .query::<&Wind>()
        .iter(app.world())
        .copied()
        .collect();
    assert_eq!(winds.len(), 1, "one scene, one wind");

    let responses: Vec<f32> = app
        .world()
        .get::<Terrain>(ground)
        .expect("the terrain")
        .detail
        .iter()
        .map(|layer| layer.wind_response)
        .collect();
    assert!(
        responses
            .iter()
            .zip([1.0, 3.0])
            .all(|(response, expected)| (response - expected).abs() < 1e-5),
        "each layer keeps the strength it leaned with as its response, got {responses:?}",
    );
}

#[test]
fn the_scene_wind_the_render_side_reads_is_still_until_a_scene_holds_one() {
    let (mut app, _tmp, _scene) = editor_on_a_scene();
    assert_eq!(
        app.world().resource::<SceneWind>().0,
        Wind::STILL,
        "a scene with nothing in it blows nothing about",
    );

    let ground = named(&mut app, "ground").expect("the scene spawned the ground");
    app.world_mut().entity_mut(ground).insert(Wind {
        strength: 2.0,
        ..Wind::default()
    });
    settle(&mut app);

    assert_eq!(app.world().resource::<SceneWind>().0.strength, 2.0);
}

#[test]
fn the_wind_operator_sets_the_scenes_wind_and_undo_puts_it_back() {
    let (mut app, _tmp, _scene) = editor_on_a_scene();
    let ground = named(&mut app, "ground").expect("the scene spawned the ground");
    app.world_mut().entity_mut(ground).insert(Wind::default());
    settle(&mut app);
    let before = *app.world().get::<Wind>(ground).expect("the scene's wind");

    call(
        &mut app,
        "environment.wind",
        &[
            ("direction", 45.0.into()),
            ("strength", 2.5.into()),
            ("gust", 0.5.into()),
        ],
    );

    let after = *app.world().get::<Wind>(ground).expect("the scene's wind");
    assert_eq!(
        (after.direction, after.strength, after.gust),
        (45.0, 2.5, 0.5)
    );
    assert_eq!(
        after.turbulence_scale, before.turbulence_scale,
        "and the fields the call left out are untouched",
    );

    call(&mut app, "history.undo", &[]);

    assert_eq!(
        *app.world().get::<Wind>(ground).expect("the scene's wind"),
        before,
    );
}
