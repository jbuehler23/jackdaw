//! What undo and redo do with a remote batch, next to the single calls and
//! menu dispatches it must leave alone. A batch is one entry that puts the
//! scene back exactly, the document and what lives beside it such as terrain
//! heights, and every other path keeps the entries it always pushed.

use std::time::Duration;

use bevy::prelude::*;
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::remote::server::{batch_handler, call_operator_handler, scene_bsn_handler};
use jackdaw::selection::Selection;
use jackdaw::terrain::TerrainDataStore;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_commands::CommandHistory;
use serde_json::{Value, json};

use crate::util;
use crate::util::OperatorResultExt as _;

#[track_caller]
pub(super) fn call<M>(
    app: &mut App,
    handler: impl bevy::ecs::system::IntoSystem<In<Option<Value>>, bevy::remote::BrpResult, M> + 'static,
    params: Value,
) -> Value {
    let answer = app
        .world_mut()
        .run_system_cached_with(handler, Some(params))
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("the handler refused: {}", err.message));
    app.update();
    answer
}

/// An open scene in a project whose assets hold `prefabs/rock.bsn`.
pub(super) fn editor() -> (App, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("jackdaw-batch-undo-")
        .tempdir()
        .expect("temp dir");
    let prefab = dir.path().join("assets/prefabs/rock.bsn");
    std::fs::create_dir_all(prefab.parent().expect("a parent")).expect("a prefabs dir");
    std::fs::write(
        &prefab,
        "#Rock\nbevy_transform::components::transform::Transform\n",
    )
    .expect("the prefab");
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    (app, dir)
}

pub(super) fn depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

pub(super) fn scene_text(app: &mut App) -> String {
    let answer = call(app, scene_bsn_handler, json!({}));
    answer["bsn"].as_str().expect("scene text").to_string()
}

pub(super) fn history(app: &mut App, id: &'static str) {
    app.world_mut()
        .operator(id)
        .call()
        .unwrap_or_else(|err| panic!("{id}: {err}"))
        .assert_finished();
    app.update();
}

fn spawn_rock(x: f64) -> Value {
    json!({
        "id": "prefab.spawn_instance",
        "params": { "path": "prefabs/rock.bsn", "pos_x": x, "pos_y": 0.0, "pos_z": 0.0 },
    })
}

pub(super) fn batch(app: &mut App, calls: Vec<Value>) -> Value {
    call(app, batch_handler, json!({ "calls": calls }))
}

/// Dispatch `id` the way a menu item or keybind does.
pub(super) fn menu(app: &mut App, id: &'static str, history: bool) {
    app.world_mut()
        .operator(id)
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: history,
        })
        .call()
        .unwrap_or_else(|err| panic!("{id}: {err}"))
        .assert_finished();
    app.update();
}

#[test]
fn a_single_remote_call_is_one_entry_that_undoes_and_redoes() {
    let (mut app, _dir) = editor();
    let empty = scene_text(&mut app);
    let before = depth(&app);

    call(
        &mut app,
        call_operator_handler,
        json!({ "id": "entity.add.cube" }),
    );
    let placed = scene_text(&mut app);
    assert_eq!(depth(&app), before + 1);

    history(&mut app, "history.undo");
    assert_eq!(scene_text(&mut app), empty);
    history(&mut app, "history.redo");
    assert_eq!(scene_text(&mut app), placed);
}

#[test]
fn a_menu_dispatch_is_one_entry_that_undoes_and_redoes() {
    let (mut app, _dir) = editor();
    let empty = scene_text(&mut app);
    let before = depth(&app);

    app.world_mut()
        .operator("prefab.spawn_instance")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("path", "prefabs/rock.bsn")
        .param("pos_x", 1.0)
        .param("pos_y", 0.0)
        .param("pos_z", 0.0)
        .call()
        .expect("dispatch")
        .assert_finished();
    app.update();
    let placed = scene_text(&mut app);
    assert_eq!(depth(&app), before + 1);

    history(&mut app, "history.undo");
    assert_eq!(scene_text(&mut app), empty);
    history(&mut app, "history.redo");
    assert_eq!(scene_text(&mut app), placed);
}

#[test]
fn each_path_outside_a_batch_pushes_what_it_always_has() {
    let (mut app, _dir) = editor();

    let before = depth(&app);
    call(&mut app, call_operator_handler, spawn_rock(1.0));
    assert_eq!(depth(&app), before + 1, "a remote call");

    let before = depth(&app);
    app.world_mut()
        .operator("prefab.spawn_instance")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("path", "prefabs/rock.bsn")
        .param("pos_x", 2.0)
        .param("pos_y", 0.0)
        .param("pos_z", 0.0)
        .call()
        .expect("dispatch")
        .assert_finished();
    app.update();
    assert_eq!(depth(&app), before + 1, "a dispatch with history");

    let before = depth(&app);
    app.world_mut()
        .operator("prefab.spawn_instance")
        .param("path", "prefabs/rock.bsn")
        .param("pos_x", 3.0)
        .param("pos_y", 0.0)
        .param("pos_z", 0.0)
        .call()
        .expect("dispatch")
        .assert_finished();
    app.update();
    assert_eq!(depth(&app), before, "a dispatch without history");

    let before = depth(&app);
    menu(&mut app, "entity.add.cube", true);
    assert_eq!(
        depth(&app),
        before + 2,
        "a menu dispatch of an operator that pushes its own command, beside its snapshot"
    );

    let before = depth(&app);
    menu(&mut app, "entity.add.cube", false);
    assert_eq!(
        depth(&app),
        before + 1,
        "an operator that pushes its own command still does without history"
    );
}

#[test]
fn a_batch_undoes_as_one_and_redoes_as_one() {
    let (mut app, _dir) = editor();
    let empty = scene_text(&mut app);
    let before = depth(&app);

    batch(
        &mut app,
        vec![
            spawn_rock(1.0),
            json!({ "id": "entity.add.group", "params": { "name": "Props" } }),
            json!({ "id": "entity.add.cube" }),
            spawn_rock(2.0),
        ],
    );
    let placed = scene_text(&mut app);
    assert_eq!(depth(&app), before + 1);

    history(&mut app, "history.undo");
    assert_eq!(scene_text(&mut app), empty);
    assert_eq!(depth(&app), before);
    history(&mut app, "history.redo");
    assert_eq!(scene_text(&mut app), placed);
    assert_eq!(depth(&app), before + 1);
}

#[test]
fn a_batch_that_stops_partway_keeps_the_calls_before_it_as_one_entry() {
    let (mut app, _dir) = editor();
    let empty = scene_text(&mut app);
    let before = depth(&app);

    let outcome = batch(
        &mut app,
        vec![
            spawn_rock(1.0),
            json!({ "id": "entity.add.cube" }),
            json!({ "id": "prefab.spawn_instance" }),
            spawn_rock(2.0),
        ],
    );
    let calls = outcome["calls"].as_array().expect("an array");
    assert_eq!(calls.len(), 3, "{outcome}");
    assert_eq!(calls[2]["result"], json!("cancelled"));
    assert_eq!(depth(&app), before + 1);
    let stopped = scene_text(&mut app);
    assert_ne!(stopped, empty);

    history(&mut app, "history.undo");
    assert_eq!(scene_text(&mut app), empty);
    history(&mut app, "history.redo");
    assert_eq!(scene_text(&mut app), stopped);
}

#[test]
fn an_undo_after_a_batch_and_a_single_call_takes_the_single_call_first() {
    let (mut app, _dir) = editor();
    let empty = scene_text(&mut app);

    batch(&mut app, vec![spawn_rock(1.0), spawn_rock(2.0)]);
    let batched = scene_text(&mut app);
    call(&mut app, call_operator_handler, spawn_rock(3.0));

    history(&mut app, "history.undo");
    assert_eq!(scene_text(&mut app), batched);
    history(&mut app, "history.undo");
    assert_eq!(scene_text(&mut app), empty);
}

/// A generated terrain, selected, and the data path its heights live under.
fn terrain(app: &mut App) -> (Entity, String) {
    history(app, "entity.add.terrain");
    app.world_mut()
        .run_system_cached(jackdaw::terrain::ensure_terrain_dirty_chunks)
        .expect("dirty-chunk tracking is installed");
    app.world_mut()
        .run_system_cached(jackdaw::terrain::ensure_terrain_data_path)
        .expect("sidecar paths are minted");
    app.update();
    let mut query = app
        .world_mut()
        .query::<(Entity, &jackdaw_scene_types::Terrain)>();
    let (entity, data_path) = query
        .iter(app.world())
        .map(|(entity, terrain)| (entity, terrain.data_path.clone()))
        .next()
        .expect("a terrain");
    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    history(app, "terrain.generate");
    app.world_mut()
        .resource_mut::<Time>()
        .advance_by(Duration::from_millis(16));
    (entity, data_path)
}

fn heights(app: &App, data_path: &str) -> Vec<f32> {
    app.world()
        .resource::<TerrainDataStore>()
        .heights(data_path)
        .into_owned()
}

#[test]
fn a_batch_puts_terrain_heights_and_the_scene_back_together() {
    let (mut app, _dir) = editor();
    let (entity, data_path) = terrain(&mut app);
    let shape = {
        let terrain = app
            .world()
            .get::<jackdaw_scene_types::Terrain>(entity)
            .expect("the terrain")
            .clone();
        app.world()
            .resource::<TerrainDataStore>()
            .grid_shape(&terrain)
    };
    let centre = shape.origin + shape.size * 0.5;
    let ground = heights(&app, &data_path);
    let scene = scene_text(&mut app);

    app.world_mut().resource_mut::<Selection>().entities = vec![entity];
    batch(
        &mut app,
        vec![
            json!({ "id": "entity.add.cube" }),
            json!({
                "id": "terrain.sculpt.stamp",
                "params": { "x": centre.x, "z": centre.y, "radius": 40.0, "strength": 5.0 },
            }),
        ],
    );
    let worked = heights(&app, &data_path);
    let placed = scene_text(&mut app);
    assert_ne!(worked, ground, "the batch changed the ground");

    history(&mut app, "history.undo");
    assert_eq!(heights(&app, &data_path), ground);
    assert_eq!(scene_text(&mut app), scene);

    history(&mut app, "history.redo");
    assert_eq!(heights(&app, &data_path), worked);
    assert_eq!(scene_text(&mut app), placed);
}
