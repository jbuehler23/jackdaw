//! Recording a recipe: the calls made by hand between `recipe.record.start`
//! and `recipe.record.stop` become a recipe file and a root that rebuilds from
//! it, and a literal in a step becomes an input with `recipe.promote`.

use bevy::prelude::*;
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::{PropertyValue, RecipeInstance};
use serde_json::{Value, json};

use crate::util;
use crate::util::OperatorResultExt as _;

use super::recipe_ops::{
    child, contents, finished, recipe_project, refused, run, undo, undo_depth,
};

fn id_of(answer: &Value) -> Value {
    answer["entities"][0].clone()
}

fn entity(answer: &Value) -> Entity {
    Entity::from_bits(answer["entities"][0].as_u64().expect("an entity"))
}

fn warnings(answer: &Value) -> String {
    answer["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("; ")
}

/// The starter house built by hand with its default inputs, each call as a
/// person makes it: naming nodes by id, not by the names the recipe uses.
fn build_house_by_hand(app: &mut App) {
    let cube = |app: &mut App, name: &str, size: [f64; 3], pos: [f64; 3]| {
        id_of(&finished(
            app,
            "entity.add.cube",
            json!({ "name": name, "size": size, "pos": pos }),
        ))
    };
    cube(app, "Floor", [6.0, 0.2, 5.0], [0.0, 0.1, 0.0]);
    let front = cube(app, "Wall_Front", [6.0, 2.8, 0.2], [0.0, 1.6, 2.4]);
    let back = cube(app, "Wall_Back", [6.0, 2.8, 0.2], [0.0, 1.6, -2.4]);
    let left = cube(app, "Wall_Left", [0.2, 2.8, 4.6], [-2.9, 1.6, 0.0]);
    let right = cube(app, "Wall_Right", [0.2, 2.8, 4.6], [2.9, 1.6, 0.0]);
    let roof = id_of(&finished(
        app,
        "mesh.add_brush",
        json!({
            "name": "Roof",
            "plane": "xy",
            "outline": "[[-3.3, 0], [3.3, 0], [0, 2.4]]",
            "depth": 5.6,
            "pos": [0.0, 3.0, 0.0],
        }),
    ));
    finished(
        app,
        "entity.tag",
        json!({ "entities": [front, back, left, right], "add": "wall" }),
    );
    finished(
        app,
        "entity.tag",
        json!({ "entities": roof, "add": "roof" }),
    );
}

/// A house recorded by hand is the starter house: the root the recording
/// leaves and a fresh run of the saved recipe both hold what a run of the
/// starter holds, and the saved calls name nodes by name, never by id.
#[test]
fn a_house_recorded_by_hand_builds_what_the_starter_builds() {
    let (mut app, dir) = recipe_project();
    let starter = run(&mut app, json!({ "recipe": "house" }));
    let built = contents(&app, starter);

    finished(&mut app, "recipe.record.start", json!({}));
    finished(
        &mut app,
        "selection.select",
        json!({ "entity": starter.to_bits() }),
    );
    build_house_by_hand(&mut app);
    finished(
        &mut app,
        "entity.rename",
        json!({ "entity": starter.to_bits(), "name": "Starter" }),
    );
    let stopped = finished(
        &mut app,
        "recipe.record.stop",
        json!({ "name": "my_house" }),
    );
    assert!(
        warnings(&stopped).contains("entity.rename"),
        "renaming a node the recording did not make is left out: {stopped}"
    );

    let text = std::fs::read_to_string(dir.path().join("assets/recipes/my_house.recipe.bsn"))
        .expect("the recipe is saved under assets");
    assert_eq!(
        text.matches("jackdaw_editor_core::recipes::RecipeCall")
            .count(),
        8,
        "{text}"
    );
    assert!(
        !text.contains("selection."),
        "selection calls are not kept: {text}"
    );
    assert!(
        text.contains("\"Wall_Front\""),
        "entities are named: {text}"
    );
    assert!(
        !text.contains(&starter.to_bits().to_string()),
        "no ids in the recipe: {text}"
    );

    let recorded = entity(&stopped);
    assert_eq!(
        app.world()
            .get::<RecipeInstance>(recorded)
            .map(|i| i.recipe.as_str()),
        Some("recipes/my_house.recipe.bsn"),
        "the recorded nodes are gathered under a root that remembers the recipe"
    );
    assert_eq!(contents(&app, recorded), built);

    let again = run(
        &mut app,
        json!({ "recipe": "my_house", "pos": [20.0, 0.0, 0.0] }),
    );
    assert_eq!(contents(&app, again), built);

    let roots_before = contents(&app, recorded);
    finished(
        &mut app,
        "recipe.rerun",
        json!({ "entity": recorded.to_bits(), "inputs": {} }),
    );
    assert_eq!(
        contents(&app, recorded),
        roots_before,
        "a rerun rebuilds it the same"
    );
}

/// Undo during a recording takes the last kept call back out of it, and a
/// recording with no name is refused while a discarded one leaves no file.
#[test]
fn undo_during_a_recording_takes_the_call_back() {
    let (mut app, dir) = recipe_project();
    finished(&mut app, "recipe.record.start", json!({}));
    assert!(
        refused(&mut app, "recipe.record.start", json!({})).contains("already"),
        "one recording at a time"
    );
    finished(&mut app, "entity.add.cube", json!({ "name": "Kept" }));
    let depth = undo_depth(&app);
    finished(&mut app, "entity.add.sphere", json!({ "name": "Dropped" }));
    undo(&mut app);
    assert_eq!(undo_depth(&app), depth);
    assert!(refused(&mut app, "recipe.record.stop", json!({})).contains("name"));
    let stopped = finished(&mut app, "recipe.record.stop", json!({ "name": "one" }));
    let text =
        std::fs::read_to_string(dir.path().join("assets/recipes/one.recipe.bsn")).expect("saved");
    assert_eq!(text.matches("RecipeCall").count(), 1, "{text}");
    assert!(text.contains("Kept") && !text.contains("Dropped"), "{text}");
    let root = entity(&stopped);
    child(&app, root, "Kept");

    finished(&mut app, "recipe.record.start", json!({}));
    finished(&mut app, "entity.add.cube", json!({ "name": "Gone" }));
    finished(&mut app, "recipe.record.stop", json!({ "discard": true }));
    assert!(
        refused(&mut app, "recipe.record.stop", json!({ "name": "late" })).contains("nothing"),
        "the discarded recording is over"
    );
    assert!(!dir.path().join("assets/recipes/late.recipe.bsn").exists());
}

/// A node made by a call and named by one, when its name is shared, is
/// written as the call that made it.
#[test]
fn a_shared_name_is_written_as_the_call_that_made_it() {
    let (mut app, dir) = recipe_project();
    finished(&mut app, "recipe.record.start", json!({}));
    let first = id_of(&finished(&mut app, "entity.add.cube", json!({})));
    finished(&mut app, "entity.add.cube", json!({}));
    finished(
        &mut app,
        "entity.tag",
        json!({ "entities": first, "add": "first" }),
    );
    let stopped = finished(&mut app, "recipe.record.stop", json!({ "name": "pair" }));
    let text =
        std::fs::read_to_string(dir.path().join("assets/recipes/pair.recipe.bsn")).expect("saved");
    assert!(text.contains("$0.entities.0"), "{text}");

    let again = run(&mut app, json!({ "recipe": "pair" }));
    assert_eq!(contents(&app, again), contents(&app, entity(&stopped)));
}

/// Promoting a literal makes it an input with the name, kind, default and
/// range given; the root keeps its id and rebuilds with the input, and a
/// rerun with a new value changes what it builds.
#[test]
fn a_promoted_literal_becomes_an_input() {
    let (mut app, dir) = recipe_project();
    finished(&mut app, "recipe.record.start", json!({}));
    finished(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Post", "size": [0.5, 2.0, 0.5] }),
    );
    let root = entity(&finished(
        &mut app,
        "recipe.record.stop",
        json!({ "name": "post" }),
    ));

    assert!(
        refused(
            &mut app,
            "recipe.promote",
            json!({ "entity": root.to_bits(), "step": 0, "param": "size", "min": 1.0 }),
        )
        .contains("min"),
        "a range takes both ends"
    );
    let file = dir.path().join("assets/recipes/post.recipe.bsn");
    let unpromoted = std::fs::read_to_string(&file).expect("saved");
    refused(
        &mut app,
        "recipe.promote",
        json!({ "entity": root.to_bits(), "step": 0, "param": "size", "kind": "int", "default": "2" }),
    );
    assert_eq!(
        std::fs::read_to_string(&file).expect("saved"),
        unpromoted,
        "a promote whose rebuild fails leaves the file as it was"
    );
    let depth = undo_depth(&app);
    finished(
        &mut app,
        "recipe.promote",
        json!({
            "entity": root.to_bits(), "step": 0, "param": "size",
            "input": "post_size", "min": 0.1, "max": 10.0,
        }),
    );
    assert_eq!(undo_depth(&app), depth + 1, "the rebuild is one undo step");
    let text =
        std::fs::read_to_string(dir.path().join("assets/recipes/post.recipe.bsn")).expect("saved");
    assert!(text.contains("\"$post_size\""), "{text}");
    assert!(
        text.contains(
            "name: \"post_size\", kind: \"vec3\", default: \"[0.5, 2, 0.5]\", range: [0.1, 10.0]"
        ),
        "{text}"
    );
    let instance = app
        .world()
        .get::<RecipeInstance>(root)
        .expect("the root keeps its id");
    assert_eq!(
        instance.inputs.get("post_size").map(String::as_str),
        Some("0.5, 2, 0.5")
    );

    let before = format!(
        "{:?}",
        app.world()
            .get::<jackdaw_scene_types::Brush>(child(&app, root, "Post"))
    );
    finished(
        &mut app,
        "recipe.rerun",
        json!({ "entity": root.to_bits(), "inputs": { "post_size": [0.5, 6.0, 0.5] } }),
    );
    let after = format!(
        "{:?}",
        app.world()
            .get::<jackdaw_scene_types::Brush>(child(&app, root, "Post"))
    );
    assert_ne!(before, after, "the input drives the size");
    assert!(
        refused(
            &mut app,
            "recipe.promote",
            json!({ "entity": root.to_bits(), "step": 0, "param": "size" }),
        )
        .contains("already"),
        "a parameter reading an input is not a literal"
    );
    let starter = run(&mut app, json!({ "recipe": "tree" }));
    assert!(
        refused(
            &mut app,
            "recipe.promote",
            json!({ "entity": starter.to_bits(), "step": 0, "param": "name" }),
        )
        .contains("ships with the editor")
    );
}

/// A gesture: a modal that adds a cube from inside its second tick and
/// finishes on its third.
#[derive(Default)]
struct GestureExtension;

#[derive(Resource, Default)]
struct GestureTicks(u32);

impl JackdawExtension for GestureExtension {
    fn id(&self) -> String {
        "test.recipe_gesture".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        ctx.register_operator::<GestureOp>();
    }
}

#[operator(id = "test.recipe_gesture", label = "Gesture", modal = true)]
fn gesture(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let tick = {
        let mut ticks = world.get_resource_or_init::<GestureTicks>();
        ticks.0 += 1;
        ticks.0
    };
    match tick {
        1 => OperatorResult::Running,
        2 => {
            let _ = world
                .operator("entity.add.cube")
                .param("name", "Gestured")
                .call();
            OperatorResult::Running
        }
        _ => {
            world.resource_mut::<GestureTicks>().0 = 0;
            OperatorResult::Finished
        }
    }
}

/// [`recipe_project`] with the gesture registered.
fn gesture_project() -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(dir.path().join("assets/recipes")).expect("an assets dir");
    let mut app = util::headless_app();
    util::register_and_enable_extension::<GestureExtension>(&mut app);
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    app.update();
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    (app, dir)
}

fn run_gesture(app: &mut App) {
    let started = app
        .world_mut()
        .operator("test.recipe_gesture")
        .settings(CallOperatorSettings {
            creates_history_entry: true,
            execution_context: ExecutionContext::Invoke,
        })
        .call()
        .expect("the gesture dispatches");
    assert_eq!(started, OperatorResult::Running);
    for _ in 0..4 {
        app.update();
    }
}

fn saved(dir: &tempfile::TempDir, name: &str) -> String {
    std::fs::read_to_string(dir.path().join(format!("assets/recipes/{name}.recipe.bsn")))
        .expect("saved")
}

/// A call a gesture makes is not a call the person made, the nodes the
/// gesture made stay where they are, and undoing the gesture leaves the kept
/// calls alone.
#[test]
fn a_gesture_is_left_out_with_what_it_made() {
    let (mut app, dir) = gesture_project();
    finished(&mut app, "recipe.record.start", json!({}));
    finished(&mut app, "entity.add.cube", json!({ "name": "Kept" }));
    run_gesture(&mut app);
    let stopped = finished(
        &mut app,
        "recipe.record.stop",
        json!({ "name": "with_gesture" }),
    );
    let text = saved(&dir, "with_gesture");
    assert_eq!(text.matches("RecipeCall").count(), 1, "{text}");
    assert!(!text.contains("Gestured"), "{text}");
    let said = warnings(&stopped);
    assert!(
        said.contains("test.recipe_gesture") && said.contains("`Gestured`"),
        "{said}"
    );
    let root = entity(&stopped);
    child(&app, root, "Kept");
    let gestured: Vec<Option<Entity>> = app
        .world_mut()
        .query::<(&Name, Option<&ChildOf>)>()
        .iter(app.world())
        .filter(|(name, _)| name.as_str() == "Gestured")
        .map(|(_, parent)| parent.map(ChildOf::parent))
        .collect();
    assert_eq!(
        gestured,
        [None],
        "the gesture's cube stays at the top level"
    );

    finished(&mut app, "recipe.record.start", json!({}));
    finished(&mut app, "entity.add.cube", json!({ "name": "Kept" }));
    run_gesture(&mut app);
    undo(&mut app);
    finished(
        &mut app,
        "recipe.record.stop",
        json!({ "name": "undone_gesture" }),
    );
    let text = saved(&dir, "undone_gesture");
    assert_eq!(text.matches("RecipeCall").count(), 1, "{text}");
}

/// Undoing a call the recording left out leaves the kept calls alone.
#[test]
fn undoing_a_left_out_call_keeps_the_recording() {
    let (mut app, dir) = recipe_project();
    let old = entity(&finished(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Old" }),
    ));
    finished(&mut app, "recipe.record.start", json!({}));
    finished(&mut app, "entity.add.cube", json!({ "name": "Kept" }));
    finished(
        &mut app,
        "entity.rename",
        json!({ "entity": old.to_bits(), "name": "Renamed" }),
    );
    undo(&mut app);
    finished(&mut app, "recipe.record.stop", json!({ "name": "kept" }));
    let text = saved(&dir, "kept");
    assert_eq!(text.matches("RecipeCall").count(), 1, "{text}");
    assert!(text.contains("Kept"), "{text}");
}

/// A refused call and a stop made from inside the recording leave nothing
/// collecting spawns behind, so the next recording numbers its calls' results
/// from its own calls.
#[test]
fn spawns_are_counted_per_call_across_recordings() {
    let (mut app, dir) = recipe_project();
    let call = |app: &mut App, id: &str, params: &[(&str, PropertyValue)]| {
        let mut call = app.world_mut().operator(id.to_string());
        for (key, value) in params {
            call = call.param(key.to_string(), value.clone());
        }
        let result = call.call().expect("dispatches");
        app.update();
        result
    };
    call(&mut app, "recipe.record.start", &[]).assert_finished();
    assert_eq!(
        call(
            &mut app,
            "entity.add.cube",
            &[("size", PropertyValue::Vec3(Vec3::new(-1.0, 1.0, 1.0)))]
        ),
        OperatorResult::Cancelled
    );
    call(
        &mut app,
        "recipe.record.stop",
        &[("name", PropertyValue::from("empty".to_string()))],
    )
    .assert_finished();

    call(&mut app, "recipe.record.start", &[]).assert_finished();
    call(&mut app, "entity.add.cube", &[]).assert_finished();
    call(&mut app, "entity.add.cube", &[]).assert_finished();
    let second = app
        .world()
        .resource::<jackdaw::selection::Selection>()
        .entities
        .last()
        .copied()
        .expect("the new cube is selected");
    call(
        &mut app,
        "entity.tag",
        &[
            (
                "entities",
                PropertyValue::from(second.to_bits().to_string()),
            ),
            ("add", PropertyValue::from("second".to_string())),
        ],
    )
    .assert_finished();
    call(
        &mut app,
        "recipe.record.stop",
        &[("name", PropertyValue::from("pair_again".to_string()))],
    )
    .assert_finished();
    let text = saved(&dir, "pair_again");
    assert!(text.contains("$1.entities.0"), "{text}");
}

/// A list naming one node by name and one only by the call that made it is
/// written with both.
#[test]
fn a_list_mixes_names_and_results() {
    let (mut app, dir) = recipe_project();
    finished(&mut app, "recipe.record.start", json!({}));
    let named = id_of(&finished(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Named" }),
    ));
    let first = id_of(&finished(&mut app, "entity.add.cube", json!({})));
    finished(&mut app, "entity.add.cube", json!({}));
    finished(
        &mut app,
        "entity.tag",
        json!({ "entities": [named, first], "add": "both" }),
    );
    let root = entity(&finished(
        &mut app,
        "recipe.record.stop",
        json!({ "name": "mixed" }),
    ));
    let text = saved(&dir, "mixed");
    assert!(
        text.contains("$1.entities.0") && text.contains("Named"),
        "{text}"
    );
    let again = run(&mut app, json!({ "recipe": "mixed" }));
    assert_eq!(contents(&app, again), contents(&app, root));
}
