//! Batches that roll back, rehearse or rebuild: `atomic`, `dry_run`, `stage`
//! with `after`, and `checks`, each leaving the scene and the history as the
//! caller was promised.

use bevy::prelude::*;
use bevy::remote::BrpResult;
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::remote::server::{
    batch_handler, call_operator_handler, paced_batch_handler, scene_bsn_handler,
};
use jackdaw_api_internal::operator::OperatorWorldExt as _;
use jackdaw_commands::CommandHistory;
use jackdaw_scene_types::BuildStage;
use serde_json::{Value, json};

use crate::util;
use crate::util::OperatorResultExt as _;

fn run_batch(app: &mut App, params: Value) -> BrpResult {
    let answer = app
        .world_mut()
        .run_system_cached_with(batch_handler, Some(params))
        .expect("the handler ran");
    app.update();
    answer
}

#[track_caller]
fn batch_ok(app: &mut App, params: Value) -> Value {
    run_batch(app, params).unwrap_or_else(|err| panic!("the batch refused: {}", err.message))
}

#[track_caller]
fn batch_err(app: &mut App, params: Value) -> bevy::remote::BrpError {
    match run_batch(app, params) {
        Ok(answer) => panic!("the batch succeeded: {answer}"),
        Err(err) => err,
    }
}

fn scene_text(app: &mut App) -> String {
    app.world_mut()
        .run_system_cached_with(scene_bsn_handler, Some(json!({})))
        .expect("the handler ran")
        .expect("the scene reads")["bsn"]
        .as_str()
        .expect("text")
        .to_string()
}

fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn redo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().redo_stack.len()
}

fn undo(app: &mut App) {
    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("history.undo dispatches")
        .assert_finished();
    app.update();
}

fn staged_scene() -> App {
    let mut app = util::editor_test_app();
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    app
}

/// Add a named cube by hand, outside any batch, and hand back its id.
fn hand_made_cube(app: &mut App, name: &str, x: f32) -> u64 {
    let outcome = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({
                "id": "entity.add.cube",
                "params": { "name": name, "pos": [x, 0.0, 0.0] },
            })),
        )
        .expect("the handler ran")
        .expect("the cube is added");
    app.update();
    outcome["entities"][0].as_u64().expect("an id")
}

fn cube_call(name: &str, x: f32) -> Value {
    json!({ "id": "entity.add.cube", "params": { "name": name, "pos": [x, 0.0, 0.0] } })
}

/// Every scene entity named `name`.
fn named(app: &mut App, name: &str) -> Vec<Entity> {
    app.world_mut()
        .query::<(Entity, &Name)>()
        .iter(app.world())
        .filter(|(_, held)| held.as_str() == name)
        .map(|(entity, _)| entity)
        .collect()
}

#[track_caller]
fn the_one(app: &mut App, name: &str) -> Entity {
    let found = named(app, name);
    assert_eq!(
        found.len(),
        1,
        "expected one `{name}`, found {}",
        found.len()
    );
    found[0]
}

fn stage_groups(app: &mut App, name: &str) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<(Entity, &Name), With<BuildStage>>()
        .iter(app.world())
        .filter(|(_, held)| held.as_str() == name)
        .map(|(entity, _)| entity)
        .collect()
}

fn child_names(app: &App, parent: Entity) -> Vec<String> {
    let mut names: Vec<String> = app
        .world()
        .get::<Children>(parent)
        .map(|children| children.to_vec())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|child| app.world().get::<Name>(child))
        .map(|name| name.as_str().to_string())
        .collect();
    names.sort();
    names
}

fn parent_of(app: &App, entity: Entity) -> Option<Entity> {
    app.world().get::<ChildOf>(entity).map(ChildOf::parent)
}

/// A batch that fails at its forty-first call takes the first forty back: the
/// scene reads the same text, no undo entry is left, the redo stack is kept,
/// and the error names the call.
#[test]
fn an_atomic_batch_failing_at_call_forty_leaves_the_scene_as_it_was() {
    let mut app = staged_scene();
    hand_made_cube(&mut app, "Lamp", 0.0);
    hand_made_cube(&mut app, "Bench", 2.0);
    undo(&mut app);
    let text = scene_text(&mut app);
    let (undo_before, redo_before) = (undo_depth(&app), redo_depth(&app));
    assert_eq!(redo_before, 1);

    let mut calls: Vec<Value> = (0..40)
        .map(|i| cube_call(&format!("Part {i}"), 0.0))
        .collect();
    calls.push(json!({ "id": "entity.add.cube", "params": { "parent": "$0.entities.5" } }));
    let err = batch_err(&mut app, json!({ "atomic": true, "calls": calls }));

    assert!(err.message.contains("call 40"), "{}", err.message);
    assert!(err.message.contains("rolled back"), "{}", err.message);
    let data = err.data.expect("the error says what ran");
    assert_eq!(data["failed_at"], json!(40), "{data}");
    assert_eq!(data["rolled_back"], json!(true), "{data}");
    assert_eq!(data["calls"].as_array().map(Vec::len), Some(40), "{data}");
    assert_eq!(scene_text(&mut app), text, "the scene changed");
    assert_eq!(undo_depth(&app), undo_before, "an undo entry was left");
    assert_eq!(redo_depth(&app), redo_before, "the redo stack was lost");
    assert!(named(&mut app, "Part 0").is_empty());
}

/// A call that ends without finishing fails an atomic batch the same way.
#[test]
fn an_atomic_batch_rolls_back_a_call_that_did_not_finish() {
    let mut app = staged_scene();
    let text = scene_text(&mut app);
    let before = undo_depth(&app);
    let err = batch_err(
        &mut app,
        json!({
            "atomic": true,
            "calls": [
                cube_call("Kept", 0.0),
                {
                    "id": "component.set",
                    "params": {
                        "entity": "$0.entities.0",
                        "type_path": "no::such::Component",
                        "field": "x",
                        "value": "1",
                    },
                },
            ],
        }),
    );
    assert!(err.message.contains("call 1"), "{}", err.message);
    assert_eq!(scene_text(&mut app), text);
    assert_eq!(undo_depth(&app), before);
}

/// A dry run answers what the real run answers and leaves nothing behind.
#[test]
fn a_dry_run_reports_the_real_outcomes_and_leaves_no_history() {
    let mut app = staged_scene();
    let text = scene_text(&mut app);
    let before = undo_depth(&app);
    let calls = json!([
        { "id": "entity.add.group", "params": { "name": "Props" } },
        { "id": "entity.add.cube", "params": { "name": "Crate", "parent": "$0.entities.0" } },
        { "id": "entity.rename", "params": { "entity": "$1.entities.0", "name": "Crate" } },
    ]);

    let rehearsed = batch_ok(&mut app, json!({ "dry_run": true, "calls": calls }));
    assert_eq!(rehearsed["dry_run"], json!(true), "{rehearsed}");
    assert_eq!(scene_text(&mut app), text, "a dry run changed the scene");
    assert_eq!(undo_depth(&app), before, "a dry run left an undo entry");

    let real = batch_ok(&mut app, json!({ "calls": calls }));
    assert_eq!(undo_depth(&app), before + 1);
    let shape = |answer: &Value| -> Vec<Value> {
        answer["calls"]
            .as_array()
            .expect("calls")
            .iter()
            .map(|call| {
                let mut call = call.clone();
                call["entities"] = json!(call["entities"].as_array().map_or(0, Vec::len));
                call
            })
            .collect()
    };
    assert_eq!(shape(&rehearsed), shape(&real), "{rehearsed} vs {real}");
}

/// Running a stage twice leaves one set of its parts, and one undo takes the
/// second run back to the first run's parts.
#[test]
fn a_stage_run_twice_holds_one_set_and_undoes_to_the_last() {
    let mut app = staged_scene();
    let first = batch_ok(
        &mut app,
        json!({ "stage": "Inn", "calls": [cube_call("Wall", 0.0), cube_call("Roof A", 0.0)] }),
    );
    assert_eq!(first["stage"]["created"], json!(true), "{first}");
    let group = stage_groups(&mut app, "Inn");
    assert_eq!(group.len(), 1);
    assert_eq!(child_names(&app, group[0]), ["Roof A", "Wall"]);
    let depth = undo_depth(&app);

    let second = batch_ok(
        &mut app,
        json!({ "stage": "Inn", "calls": [cube_call("Wall", 0.0), cube_call("Roof B", 0.0)] }),
    );
    assert_eq!(second["stage"]["replaced"], json!(2), "{second}");
    assert_eq!(undo_depth(&app), depth + 1, "a rebuild is one undo entry");
    let group = stage_groups(&mut app, "Inn");
    assert_eq!(group.len(), 1, "the rebuild added a second group");
    assert_eq!(child_names(&app, group[0]), ["Roof B", "Wall"]);
    the_one(&mut app, "Wall");

    undo(&mut app);
    let group = stage_groups(&mut app, "Inn");
    assert_eq!(group.len(), 1);
    assert_eq!(child_names(&app, group[0]), ["Roof A", "Wall"]);
    assert!(named(&mut app, "Roof B").is_empty());
}

/// A rebuild keeps the group where it was put by hand, and builds its parts
/// in the group's own frame.
#[test]
fn a_stage_rebuild_keeps_the_groups_transform_and_place() {
    let mut app = staged_scene();
    hand_made_cube(&mut app, "Lamp", 0.0);
    batch_ok(
        &mut app,
        json!({ "stage": "Inn", "calls": [cube_call("Wall", 1.0)] }),
    );
    let group = stage_groups(&mut app, "Inn")[0];
    app.world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({
                "id": "entity.set_transform",
                "params": { "entity": group.to_bits(), "x": 10.0 },
            })),
        )
        .expect("the handler ran")
        .expect("the group moves");
    app.update();
    let lamp = the_one(&mut app, "Lamp");
    let folder = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": "entity.add.group", "params": { "name": "Village" } })),
        )
        .expect("the handler ran")
        .expect("a folder");
    app.update();
    let folder = Entity::try_from_bits(folder["entities"][0].as_u64().expect("an id")).expect("id");
    app.world_mut()
        .operator("entity.reparent")
        .param("child", group)
        .param("parent", folder)
        .call()
        .expect("entity.reparent dispatches")
        .assert_finished();
    app.update();

    batch_ok(
        &mut app,
        json!({ "stage": "Inn", "calls": [cube_call("Wall", 2.0)] }),
    );
    let group = stage_groups(&mut app, "Inn")[0];
    assert_eq!(
        parent_of(&app, group),
        Some(folder),
        "the group left its folder"
    );
    assert_eq!(
        app.world().get::<Transform>(group).map(|t| t.translation.x),
        Some(10.0)
    );
    let wall = the_one(&mut app, "Wall");
    assert_eq!(parent_of(&app, wall), Some(group));
    assert_eq!(
        app.world().get::<Transform>(wall).map(|t| t.translation.x),
        Some(2.0)
    );
    assert_eq!(parent_of(&app, lamp), None);
}

/// Rebuilding one stage leaves other stages and hand-made nodes as they were.
#[test]
fn a_stage_rebuild_leaves_other_stages_and_hand_made_nodes_alone() {
    let mut app = staged_scene();
    hand_made_cube(&mut app, "Lamp", 4.0);
    batch_ok(
        &mut app,
        json!({ "stage": "Walls", "calls": [cube_call("Wall", 0.0)] }),
    );
    batch_ok(
        &mut app,
        json!({ "stage": "Inn", "after": ["Walls"], "calls": [cube_call("Roof", 0.0)] }),
    );
    let walls = stage_groups(&mut app, "Walls")[0];
    let wall = the_one(&mut app, "Wall");
    let lamp = the_one(&mut app, "Lamp");

    batch_ok(
        &mut app,
        json!({ "stage": "Inn", "after": ["Walls"], "calls": [cube_call("Roof", 1.0)] }),
    );
    assert_eq!(
        the_one(&mut app, "Wall"),
        wall,
        "the other stage was rebuilt"
    );
    assert_eq!(
        the_one(&mut app, "Lamp"),
        lamp,
        "the hand-made node was touched"
    );
    assert_eq!(child_names(&app, walls), ["Wall"]);
    assert_eq!(
        app.world().get::<Transform>(lamp).map(|t| t.translation.x),
        Some(4.0)
    );
    let inn = stage_groups(&mut app, "Inn")[0];
    assert_eq!(
        app.world()
            .get::<BuildStage>(inn)
            .map(|stage| stage.after.clone()),
        Some(vec!["Walls".to_string()])
    );
}

/// A stage that comes after a stage the scene does not hold is refused
/// before anything runs.
#[test]
fn a_stage_after_a_missing_stage_is_refused() {
    let mut app = staged_scene();
    let text = scene_text(&mut app);
    let err = batch_err(
        &mut app,
        json!({ "stage": "Inn", "after": ["Walls"], "calls": [cube_call("Roof", 0.0)] }),
    );
    assert!(err.message.contains("`Walls`"), "{}", err.message);
    assert_eq!(scene_text(&mut app), text);
    assert!(stage_groups(&mut app, "Inn").is_empty());
}

/// A stage call that edits a node outside its group, or builds under one,
/// fails the batch, which rolls back.
#[test]
fn a_stage_refuses_a_call_that_reaches_outside_its_group() {
    let mut app = staged_scene();
    hand_made_cube(&mut app, "Lamp", 4.0);
    batch_ok(
        &mut app,
        json!({ "stage": "Inn", "calls": [cube_call("Roof", 0.0)] }),
    );
    let text = scene_text(&mut app);
    let before = undo_depth(&app);

    // A rollback respawns the scene, so the lamp is looked up again each time.
    for outside in [
        |lamp: u64| json!({ "id": "entity.set_transform", "params": { "entity": lamp, "x": 9.0 } }),
        |lamp: u64| json!({ "id": "entity.add.cube", "params": { "name": "Shade", "parent": lamp } }),
    ] {
        let lamp = the_one(&mut app, "Lamp").to_bits();
        let outside = outside(lamp);
        let err = batch_err(
            &mut app,
            json!({ "stage": "Inn", "calls": [cube_call("Roof", 1.0), outside] }),
        );
        assert!(err.message.contains("call 1"), "{}", err.message);
        assert!(
            err.message.contains("outside stage `Inn`"),
            "{}",
            err.message
        );
        assert_eq!(scene_text(&mut app), text, "the refused rebuild stayed");
        assert_eq!(undo_depth(&app), before);
    }
}

/// A stage saved and opened again is still found by name, so running it
/// replaces rather than adds.
#[test]
fn a_reopened_stage_is_replaced_not_duplicated() {
    let dir = tempfile::Builder::new()
        .prefix("jackdaw-stage-")
        .tempdir()
        .expect("temp dir");
    let path = dir.path().join("assets/scenes/inn.bsn");
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("a scenes dir");
    let project = |app: &mut App| {
        app.world_mut().insert_resource(ProjectRoot::new(
            dir.path().to_path_buf(),
            ProjectConfig::default(),
        ));
        app.update();
    };

    let mut app = staged_scene();
    project(&mut app);
    batch_ok(
        &mut app,
        json!({ "stage": "Inn", "calls": [cube_call("Roof", 0.0)] }),
    );
    app.world_mut()
        .operator("scene.save")
        .param("path", path.to_string_lossy().into_owned())
        .call()
        .expect("scene.save dispatches")
        .assert_finished();
    app.update();
    app.update();
    drop(app);

    let mut app = util::editor_test_app();
    project(&mut app);
    app.world_mut()
        .operator("scene.open")
        .param("path", path.to_string_lossy().into_owned())
        .call()
        .expect("scene.open dispatches")
        .assert_finished();
    app.update();
    app.update();
    assert_eq!(
        stage_groups(&mut app, "Inn").len(),
        1,
        "the stage did not load"
    );
    the_one(&mut app, "Roof");

    let answer = batch_ok(
        &mut app,
        json!({ "stage": "Inn", "calls": [cube_call("Roof", 1.0)] }),
    );
    assert_eq!(answer["stage"]["created"], json!(false), "{answer}");
    assert_eq!(stage_groups(&mut app, "Inn").len(), 1);
    the_one(&mut app, "Roof");
}

/// A failing check fails the batch: an atomic one rolls back, a plain one
/// keeps its calls as one undo entry and says so.
#[test]
fn a_failing_check_fails_the_batch() {
    let mut app = staged_scene();
    let text = scene_text(&mut app);
    let before = undo_depth(&app);
    let checks = json!([
        { "count": { "filter": { "name_prefix": "Fence" }, "eq": 2 } },
        { "bounds": { "entity": "$0.entities.0", "min": [-5.0, -5.0, -5.0], "max": [5.0, 5.0, 5.0] } },
    ]);

    let err = batch_err(
        &mut app,
        json!({ "atomic": true, "checks": checks, "calls": [cube_call("Fence 1", 0.0)] }),
    );
    assert!(
        err.message.contains("1 of 2 checks failed"),
        "{}",
        err.message
    );
    let data = err.data.expect("the error says what was checked");
    assert_eq!(data["checks"]["failed"], json!(1), "{data}");
    assert_eq!(data["rolled_back"], json!(true), "{data}");
    assert_eq!(scene_text(&mut app), text);
    assert_eq!(undo_depth(&app), before);

    let err = batch_err(
        &mut app,
        json!({ "checks": checks, "calls": [cube_call("Fence 1", 0.0)] }),
    );
    assert_eq!(err.data.expect("data")["rolled_back"], json!(false));
    assert_eq!(undo_depth(&app), before + 1);
    the_one(&mut app, "Fence 1");

    let answer = batch_ok(
        &mut app,
        json!({ "atomic": true, "checks": checks, "calls": [cube_call("Fence 2", 2.0)] }),
    );
    assert_eq!(answer["checks"]["passed"], json!(2), "{answer}");
}

/// A malformed check, or one that names a call that does not run, refuses the
/// batch before anything runs.
#[test]
fn a_malformed_check_is_refused_before_any_call_runs() {
    let mut app = staged_scene();
    let before = undo_depth(&app);
    for check in [
        json!({ "level": {} }),
        json!({ "bounds": { "entity": "$1.entities.0" } }),
    ] {
        let err = batch_err(
            &mut app,
            json!({ "checks": [check], "calls": [cube_call("Crate", 0.0)] }),
        );
        assert!(err.message.contains("check 0"), "{}", err.message);
    }
    assert_eq!(undo_depth(&app), before);
    assert!(named(&mut app, "Crate").is_empty());
}

/// A call that writes to disk cannot be rolled back, so an atomic, dry-run or
/// staged batch refuses it up front and names it.
#[test]
fn a_disk_write_is_refused_in_a_batch_that_must_roll_back() {
    let mut app = staged_scene();
    let text = scene_text(&mut app);
    for flags in [
        json!({ "atomic": true }),
        json!({ "dry_run": true }),
        json!({ "stage": "Inn" }),
    ] {
        let mut params = flags.clone();
        params["calls"] = json!([
            cube_call("Crate", 0.0),
            { "id": "scene.save", "params": { "path": "assets/scenes/out.bsn" } },
            { "id": "prefab.pack" },
        ]);
        let err = batch_err(&mut app, params);
        assert!(
            err.message.contains("call 1 (scene.save)")
                && err.message.contains("call 2 (prefab.pack)"),
            "{flags}: {}",
            err.message
        );
        let data = err.data.expect("the refusal lists the calls");
        assert_eq!(
            data["unrecoverable"][0]["id"],
            json!("scene.save"),
            "{data}"
        );
        assert_eq!(scene_text(&mut app), text, "{flags}: something ran");
    }
}

/// A batch that deletes a node undoes to one copy of it: the scene change and
/// the deletion's own command must not both put it back.
#[test]
fn undoing_a_batch_that_deleted_a_node_brings_back_one_copy() {
    let mut app = staged_scene();
    let lamp = hand_made_cube(&mut app, "Lamp", 0.0);
    batch_ok(
        &mut app,
        json!({ "calls": [{ "id": "entity.delete", "params": { "entity": lamp } }] }),
    );
    assert!(named(&mut app, "Lamp").is_empty());
    undo(&mut app);
    the_one(&mut app, "Lamp");
}

/// A stage rebuild redoes to one set of parts, and undoing both runs leaves
/// no stage behind.
#[test]
fn a_stage_rebuild_redoes_to_one_set() {
    let mut app = staged_scene();
    batch_ok(
        &mut app,
        json!({ "stage": "Inn", "calls": [cube_call("Wall", 0.0)] }),
    );
    batch_ok(
        &mut app,
        json!({ "stage": "Inn", "calls": [cube_call("Wall", 1.0)] }),
    );
    undo(&mut app);
    app.world_mut()
        .operator("history.redo")
        .call()
        .expect("history.redo dispatches")
        .assert_finished();
    app.update();
    the_one(&mut app, "Wall");
    undo(&mut app);
    undo(&mut app);
    assert!(named(&mut app, "Wall").is_empty());
    assert!(stage_groups(&mut app, "Inn").is_empty());
}

fn project_scene() -> (App, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("jackdaw-batches-")
        .tempdir()
        .expect("temp dir");
    std::fs::create_dir_all(dir.path().join("assets")).expect("an assets dir");
    let mut app = staged_scene();
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    app.update();
    (app, dir)
}

/// A rollback undoes what the calls did outside the scene document too: a
/// state added to the open animation graph is gone again.
#[test]
fn a_rollback_takes_back_edits_outside_the_scene_document() {
    use jackdaw::animation::graph_doc::AnimationGraphDoc;
    let (mut app, _dir) = project_scene();
    app.world_mut()
        .operator("animation.graph.new")
        .param("name", "walk")
        .call()
        .expect("animation.graph.new dispatches")
        .assert_finished();
    app.update();
    let states = |app: &App| app.world().resource::<AnimationGraphDoc>().def.states.len();
    assert!(app.world().resource::<AnimationGraphDoc>().path.is_some());
    let before = states(&app);

    batch_err(
        &mut app,
        json!({
            "atomic": true,
            "calls": [
                { "id": "animation.graph.add_state", "params": { "name": "Idle" } },
                cube_call("Crate", 0.0),
                { "id": "entity.add.cube", "params": { "parent": "$1.entities.5" } },
            ],
        }),
    );
    assert_eq!(states(&app), before, "the graph kept the rolled-back state");
}

/// Ids read before a dry run or a failed atomic batch still name the same
/// nodes afterwards: a rollback touches only what the batch changed.
#[test]
fn ids_read_before_a_rollback_still_resolve_after_it() {
    let mut app = staged_scene();
    let lamp = hand_made_cube(&mut app, "Lamp", 0.0);
    let bench = hand_made_cube(&mut app, "Bench", 2.0);
    let names_of = |app: &App| -> Vec<Option<String>> {
        [lamp, bench]
            .into_iter()
            .map(|bits| {
                let entity = Entity::try_from_bits(bits)?;
                app.world()
                    .get::<Name>(entity)
                    .map(|name| name.as_str().to_string())
            })
            .collect()
    };
    let expected = vec![Some("Lamp".to_string()), Some("Bench".to_string())];

    batch_ok(
        &mut app,
        json!({
            "dry_run": true,
            "calls": [
                cube_call("Crate", 0.0),
                { "id": "entity.set_transform", "params": { "entity": lamp, "x": 5.0 } },
            ],
        }),
    );
    assert_eq!(names_of(&app), expected, "a dry run invalidated held ids");
    let lamp_entity = Entity::try_from_bits(lamp).expect("an id");
    assert_eq!(
        app.world()
            .get::<Transform>(lamp_entity)
            .map(|t| t.translation.x),
        Some(0.0)
    );

    batch_err(
        &mut app,
        json!({
            "atomic": true,
            "calls": [
                { "id": "entity.set_transform", "params": { "entity": bench, "x": 7.0 } },
                { "id": "entity.add.cube", "params": { "parent": "$0.entities.5" } },
            ],
        }),
    );
    assert_eq!(names_of(&app), expected, "a rollback invalidated held ids");
}

/// A rollback puts the selection back even when the document never changed.
#[test]
fn a_rollback_restores_the_selection() {
    let mut app = staged_scene();
    let lamp = hand_made_cube(&mut app, "Lamp", 0.0);
    app.world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": "selection.select", "params": { "entity": lamp } })),
        )
        .expect("the handler ran")
        .expect("the lamp is selected");
    app.update();
    let selected = |app: &App| {
        app.world()
            .resource::<jackdaw::selection::Selection>()
            .entities
            .iter()
            .map(|entity| entity.to_bits())
            .collect::<Vec<_>>()
    };
    assert_eq!(selected(&app), [lamp]);

    batch_err(
        &mut app,
        json!({
            "atomic": true,
            "calls": [
                { "id": "selection.clear" },
                {
                    "id": "component.set",
                    "params": {
                        "entity": lamp,
                        "type_path": "no::such::Component",
                        "field": "x",
                        "value": "1",
                    },
                },
            ],
        }),
    );
    assert_eq!(
        selected(&app),
        [lamp],
        "the rollback left the selection cleared"
    );
}

/// Undo and redo inside a batch that must roll back are refused before
/// anything runs; a plain batch still steps the history as it did.
#[test]
fn history_steps_are_refused_only_where_a_rollback_is_promised() {
    let mut app = staged_scene();
    hand_made_cube(&mut app, "Lamp", 0.0);
    let before = undo_depth(&app);
    for id in ["history.undo", "history.redo"] {
        let err = batch_err(
            &mut app,
            json!({ "atomic": true, "calls": [cube_call("Crate", 0.0), { "id": id }] }),
        );
        assert!(
            err.message.contains(&format!("call 1 ({id})")),
            "{}",
            err.message
        );
        assert_eq!(undo_depth(&app), before);
    }
    the_one(&mut app, "Lamp");

    batch_ok(&mut app, json!({ "calls": [{ "id": "history.undo" }] }));
    assert!(
        named(&mut app, "Lamp").is_empty(),
        "a plain batch did not undo"
    );
}

/// A plain batch with checks that stops early is an error that says the
/// checks were not run, rather than an answer that reads as a pass.
#[test]
fn a_plain_batch_that_stops_early_says_its_checks_did_not_run() {
    let mut app = staged_scene();
    let err = batch_err(
        &mut app,
        json!({
            "checks": [{ "count": { "filter": { "name_prefix": "Crate" }, "eq": 1 } }],
            "calls": [
                cube_call("Crate", 0.0),
                {
                    "id": "component.set",
                    "params": {
                        "entity": "$0.entities.0",
                        "type_path": "no::such::Component",
                        "field": "x",
                        "value": "1",
                    },
                },
            ],
        }),
    );
    assert!(
        err.message.contains("checks were not run"),
        "{}",
        err.message
    );
    assert_eq!(err.data.expect("data")["rolled_back"], json!(false));
    the_one(&mut app, "Crate");
}

/// Operators whose ids read like a write but that only read files or edit
/// what is in memory.
const READS_ONLY: [&str; 7] = [
    "viewport.bookmark.save",
    "terrain.import.pick",
    "terrain.import.image.add",
    "terrain.import.image.remove",
    "terrain.import.image.target",
    "terrain.scatter.import",
    "terrain.scatter.import.pick",
];

/// Every operator whose id says it saves, packs, imports, exports, bakes,
/// screenshots or builds declares that it writes to disk, so a batch that
/// must roll back refuses it; scene and history switches declare that they
/// leave the history.
#[test]
fn operators_that_write_files_declare_it() {
    use jackdaw_api_internal::lifecycle::OperatorEntity;
    let mut app = staged_scene();
    let mut ops = app.world_mut().query::<&OperatorEntity>();
    let writing_words = [
        "save",
        "pack",
        "import",
        "export",
        "bake",
        "screenshot",
        "build",
        "capture",
    ];
    let mut unflagged = Vec::new();
    for op in ops.iter(app.world()) {
        let id = op.id();
        let words: Vec<&str> = id.split(['.', '_']).collect();
        let writes = writing_words.iter().any(|word| words.contains(word))
            && !id.starts_with("launcher.")
            && !READS_ONLY.contains(&id);
        if writes && !op.writes_disk() {
            unflagged.push(id);
        }
        let switches = matches!(
            id,
            "scene.new"
                | "scene.open"
                | "scene.close"
                | "scene.switch"
                | "history.undo"
                | "history.redo"
        );
        if switches && !op.leaves_history() {
            unflagged.push(id);
        }
    }
    unflagged.sort_unstable();
    assert!(unflagged.is_empty(), "undeclared: {unflagged:?}");
}

/// References resolve item by item inside list parameters: an `entities` list
/// built from two earlier calls selects both, and a check's `entities` list
/// measures both.
#[test]
fn references_inside_a_list_parameter_resolve_item_by_item() {
    let mut app = staged_scene();
    let answer = batch_ok(
        &mut app,
        json!({
            "verbose": true,
            "calls": [
                cube_call("Post A", 0.0),
                cube_call("Post B", 3.0),
                { "id": "selection.select", "params": {
                    "entities": ["$0.entities.0", "$1.entities.0"],
                } },
            ],
            "checks": [{ "no_overlap": { "entities": ["$0.entities.0", "$1.entities.0"] } }],
        }),
    );
    assert_eq!(answer["calls"][2]["result"], json!("finished"), "{answer}");
    assert_eq!(answer["checks"]["passed"], json!(1), "{answer}");
    let mut selected = app
        .world()
        .resource::<jackdaw::selection::Selection>()
        .entities
        .clone();
    selected.sort();
    let mut posts = vec![the_one(&mut app, "Post A"), the_one(&mut app, "Post B")];
    posts.sort();
    assert_eq!(selected, posts);
}

/// A reference inside a nested list parameter, here the points of a
/// `mesh.add_brush` outline, takes the value it names: a cap laid over what a
/// cut left of a wall, from the box the cut reported.
#[test]
fn references_inside_an_outline_take_the_numbers_they_name() {
    let mut app = staged_scene();
    let corner = |at: &str, axis: usize| format!("$2.bounds.0.{at}.{axis}");
    let answer = batch_ok(
        &mut app,
        json!({
            "verbose": true,
            "calls": [
                { "id": "entity.add.cube", "params": { "name": "Wall", "size": [4.0, 2.0, 2.0] } },
                { "id": "entity.add.cube", "params": {
                    "name": "Cutter", "size": [1.0, 4.0, 4.0], "pos": [1.5, 0.0, 0.0],
                } },
                { "id": "brush.csg_subtract", "params": {
                    "target": "$0.entities.0", "cutters": "$1.entities.0",
                } },
                { "id": "mesh.add_brush", "params": {
                    "name": "Cap",
                    "plane": "xz",
                    "depth": 0.5,
                    "pos": [0.0, 1.25, 0.0],
                    "outline": [
                        [corner("0", 0), corner("0", 2)],
                        [corner("1", 0), corner("0", 2)],
                        [corner("1", 0), corner("1", 2)],
                        [corner("0", 0), corner("1", 2)],
                    ],
                } },
            ],
        }),
    );
    assert_eq!(answer["calls"][3]["outcome"], json!("changed"), "{answer}");
    let cap = the_one(&mut app, "Cap");
    let global = app
        .world()
        .get::<GlobalTransform>(cap)
        .expect("a transform");
    let (min, max) = app
        .world()
        .get::<jackdaw_scene_types::Brush>(cap)
        .expect("a brush")
        .topology
        .vertices
        .iter()
        .map(|vert| global.transform_point(vert.position))
        .fold((Vec3::MAX, Vec3::MIN), |(min, max), point| {
            (min.min(point), max.max(point))
        });
    assert!(
        min.abs_diff_eq(Vec3::new(-2.0, 1.0, -1.0), 1e-3)
            && max.abs_diff_eq(Vec3::new(1.0, 1.5, 1.0), 1e-3),
        "the cap spans {min}..{max}"
    );
}

/// A scene with the system that plays paced batches, which the remote
/// plugin adds beside the method in the editor.
fn paced_scene() -> App {
    let mut app = staged_scene();
    app.add_systems(Update, jackdaw::remote::paced::drive_paced_batch);
    app
}

/// Poll a paced batch once, then let a frame pass.
fn paced_poll(app: &mut App, params: &Value) -> BrpResult<Option<Value>> {
    let answer = app
        .world_mut()
        .run_system_cached_with(paced_batch_handler, Some(params.clone()))
        .expect("the handler ran");
    app.update();
    answer
}

/// Poll a paced batch until it answers.
#[track_caller]
fn paced_answer(app: &mut App, params: &Value) -> BrpResult {
    for _ in 0..600 {
        if let Some(answer) = paced_poll(app, params).transpose() {
            return answer;
        }
    }
    panic!("the paced batch never answered");
}

/// A paced batch runs one call a frame with no pause, and between them the
/// scene holds the calls so far; the whole batch is still one undo entry, and
/// the poll after the answer does not run it again.
#[test]
fn a_paced_batch_builds_one_call_at_a_time_as_one_undo_entry() {
    let mut app = paced_scene();
    let before = undo_depth(&app);
    let params = json!({
        "request": "posts",
        "pace_ms": 0,
        "label": "Posts",
        "calls": [cube_call("Post 1", 0.0), cube_call("Post 2", 2.0), cube_call("Post 3", 4.0)],
    });
    let mut seen = Vec::new();
    let answer = loop {
        let polled = paced_poll(&mut app, &params).expect("the paced batch runs");
        let built = (1..=3)
            .filter(|n| !named(&mut app, &format!("Post {n}")).is_empty())
            .count();
        seen.push(built);
        if let Some(answer) = polled {
            break answer;
        }
        assert!(seen.len() < 20, "the paced batch never answered: {seen:?}");
    };
    assert!(seen.contains(&1) && seen.contains(&2), "{seen:?}");
    assert_eq!(
        answer["calls"].as_array().map(Vec::len),
        Some(3),
        "{answer}"
    );
    assert_eq!(undo_depth(&app), before + 1);

    paced_poll(&mut app, &params).expect_err("a repeat poll is not a second batch");
    assert_eq!(named(&mut app, "Post 3").len(), 1);
    undo(&mut app);
    assert!(named(&mut app, "Post 1").is_empty());
}

/// `pace_ms` holds each call back by that long.
#[test]
fn a_paced_batch_waits_its_pace_between_calls() {
    let mut app = paced_scene();
    let params = json!({
        "request": "slow",
        "pace_ms": 40,
        "calls": [cube_call("Slow 1", 0.0), cube_call("Slow 2", 2.0), cube_call("Slow 3", 4.0)],
    });
    let started = std::time::Instant::now();
    paced_answer(&mut app, &params).expect("the paced batch finishes");
    assert!(
        started.elapsed() >= std::time::Duration::from_millis(80),
        "three calls 40 ms apart took {:?}",
        started.elapsed()
    );
}

/// An atomic paced batch that fails part way takes back what it had built.
#[test]
fn an_atomic_paced_batch_that_fails_takes_everything_back() {
    let mut app = paced_scene();
    let text = scene_text(&mut app);
    let before = undo_depth(&app);
    let params = json!({
        "request": "broken",
        "pace_ms": 0,
        "atomic": true,
        "calls": [
            cube_call("Shed 1", 0.0),
            cube_call("Shed 2", 2.0),
            { "id": "entity.set_transform", "params": { "entity": "No Such Shed", "x": 1.0 } },
        ],
    });
    let err = paced_answer(&mut app, &params).expect_err("the third call fails");
    assert_eq!(err.data.expect("data")["rolled_back"], json!(true));
    assert_eq!(scene_text(&mut app), text);
    assert_eq!(undo_depth(&app), before);
}

/// Pacing is refused over its cap, with a rehearsal, without a request id,
/// on the instant batch method, and while another paced batch plays.
#[test]
fn a_paced_batch_is_refused_where_it_cannot_be_played() {
    let mut app = paced_scene();
    let calls = json!([cube_call("Gate", 0.0)]);
    for (params, says) in [
        (
            json!({ "request": "a", "pace_ms": 60000, "calls": calls }),
            "limit",
        ),
        (
            json!({ "request": "a", "pace_ms": -5, "calls": calls }),
            "milliseconds",
        ),
        (
            json!({ "request": "a", "pace_ms": 10, "dry_run": true, "calls": calls }),
            "dry_run",
        ),
        (json!({ "pace_ms": 10, "calls": calls }), "request"),
    ] {
        let err = paced_poll(&mut app, &params).expect_err("refused");
        assert!(err.message.contains(says), "{params}: {}", err.message);
    }
    let err = batch_err(&mut app, json!({ "pace_ms": 10, "calls": calls }));
    assert!(
        err.message.contains("jackdaw/paced_batch"),
        "{}",
        err.message
    );

    let first = json!({ "request": "first", "pace_ms": 2000, "calls": [cube_call("Gate", 0.0), cube_call("Gate 2", 1.0)] });
    assert!(paced_poll(&mut app, &first).expect("starts").is_none());
    let err = paced_poll(
        &mut app,
        &json!({ "request": "second", "pace_ms": 0, "calls": calls }),
    )
    .expect_err("one plays at a time");
    assert!(err.message.contains("still playing"), "{}", err.message);
    assert!(named(&mut app, "Gate").len() <= 1);
}

/// While a paced batch plays, undo and the other history steps are refused
/// with a reason and reported unavailable, so an undo cannot fold an older
/// entry into the batch; remote edits are refused too. Afterwards one undo
/// takes back the batch alone.
#[test]
fn a_playing_paced_batch_holds_the_history_and_refuses_other_edits() {
    let mut app = paced_scene();
    hand_made_cube(&mut app, "Older", -4.0);
    let before = undo_depth(&app);
    let params = json!({
        "request": "held",
        "pace_ms": 0,
        "calls": [cube_call("Held 1", 0.0), cube_call("Held 2", 2.0), cube_call("Held 3", 4.0)],
    });
    assert!(paced_poll(&mut app, &params).expect("starts").is_none());
    assert!(!named(&mut app, "Held 1").is_empty(), "the first call ran");

    assert!(
        !app.world_mut()
            .operator("history.undo")
            .is_available()
            .expect("a known operator")
    );
    let undo_result = app
        .world_mut()
        .operator("history.undo")
        .call()
        .expect("history.undo dispatches");
    assert!(matches!(
        undo_result,
        jackdaw_api_internal::operator::OperatorResult::Cancelled
    ));
    app.update();

    let refused = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": "entity.add.cube", "params": { "name": "Stray" } })),
        )
        .expect("the handler ran")
        .expect_err("a remote edit waits for the batch");
    assert!(
        refused.message.contains("paced batch"),
        "{}",
        refused.message
    );
    let refused = batch_err(&mut app, json!({ "calls": [cube_call("Stray", 9.0)] }));
    assert!(
        refused.message.contains("paced batch"),
        "{}",
        refused.message
    );

    paced_answer(&mut app, &params).expect("the batch finishes");
    assert_eq!(undo_depth(&app), before + 1);
    the_one(&mut app, "Older");
    assert!(named(&mut app, "Stray").is_empty());
    undo(&mut app);
    assert!(named(&mut app, "Held 1").is_empty());
    the_one(&mut app, "Older");
}

/// A request id that was answered is refused when it comes back rather than
/// left waiting, and is forgotten after a while however often it is polled.
#[test]
fn a_reused_paced_request_id_is_refused_then_forgotten() {
    let mut app = paced_scene();
    let params = json!({ "request": "once", "pace_ms": 0, "calls": [cube_call("Once", 0.0)] });
    paced_answer(&mut app, &params).expect("the first run answers");
    for _ in 0..3 {
        let err = paced_poll(&mut app, &params).expect_err("already answered");
        assert!(err.message.contains("already answered"), "{}", err.message);
    }
    app.world_mut()
        .resource_mut::<bevy::diagnostic::FrameCount>()
        .0 += 1000;
    app.update();
    paced_answer(&mut app, &params).expect("the id is new again");
    assert_eq!(named(&mut app, "Once").len(), 2);
}
