//! Recipes: `recipe.run` builds a saved list of calls into a new root as one
//! undo step, `recipe.rerun` rebuilds that root with new inputs, and the root
//! keeps the `RecipeInstance` that says how, through save and reopen.

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::remote::server::{call_operator_handler, scene_bsn_handler};
use jackdaw_api_internal::operator::OperatorWorldExt as _;
use jackdaw_scene_types::{Brush, RecipeInstance, Tags};
use serde_json::{Value, json};

use crate::util;
use crate::util::OperatorResultExt as _;

/// An editor with a scene open in a project of its own, and the project.
pub(super) fn recipe_project() -> (App, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("jackdaw-recipes-")
        .tempdir()
        .expect("temp dir");
    std::fs::create_dir_all(dir.path().join("assets/recipes")).expect("an assets dir");
    let mut app = util::editor_test_app();
    open_project(&mut app, &dir);
    app.world_mut()
        .operator("scene.new")
        .call()
        .expect("scene.new dispatches")
        .assert_finished();
    app.update();
    (app, dir)
}

fn open_project(app: &mut App, dir: &tempfile::TempDir) {
    app.world_mut().insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    app.update();
}

/// Write a project recipe and index it, as the project watcher would.
pub(super) fn write_recipe(app: &mut App, dir: &tempfile::TempDir, name: &str, body: &str) {
    let text = format!(
        "// jackdaw asset jackdaw_editor_core::recipes::Recipe\n#{name}\njackdaw_editor_core::recipes::Recipe {{\n{body}\n}}\n"
    );
    std::fs::write(
        dir.path().join(format!("assets/recipes/{name}.recipe.bsn")),
        text,
    )
    .expect("write the recipe");
    jackdaw::asset_index::rescan_asset_index(app.world_mut());
    app.update();
}

#[track_caller]
pub(super) fn call(app: &mut App, id: &str, params: Value) -> Value {
    let answer = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": id, "params": params })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("{id} refused: {}", err.message));
    app.update();
    answer
}

#[track_caller]
pub(super) fn finished(app: &mut App, id: &str, params: Value) -> Value {
    let answer = call(app, id, params);
    assert_eq!(answer["result"], json!("finished"), "{id}: {answer}");
    answer
}

#[track_caller]
pub(super) fn refused(app: &mut App, id: &str, params: Value) -> String {
    let answer = call(app, id, params);
    assert_eq!(answer["result"], json!("cancelled"), "{id}: {answer}");
    answer["warnings"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("; ")
}

#[track_caller]
pub(super) fn run(app: &mut App, params: Value) -> Entity {
    let answer = finished(app, "recipe.run", params);
    let bits = answer["entities"][0].as_u64().expect("the root's id");
    Entity::from_bits(bits)
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

pub(super) fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

pub(super) fn undo(app: &mut App) {
    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("history.undo dispatches")
        .assert_finished();
    app.update();
}

fn roots_named(app: &mut App, name: &str) -> Vec<Entity> {
    app.world_mut()
        .query::<(Entity, &Name, &RecipeInstance)>()
        .iter(app.world())
        .filter(|(_, held, _)| held.as_str() == name)
        .map(|(entity, ..)| entity)
        .collect()
}

/// The child of `root` named `name`.
#[track_caller]
pub(super) fn child(app: &App, root: Entity, name: &str) -> Entity {
    let found: Vec<Entity> = app
        .world()
        .get::<Children>(root)
        .map(|children| {
            children
                .iter()
                .filter(|child| {
                    app.world()
                        .get::<Name>(*child)
                        .is_some_and(|held| held.as_str() == name)
                })
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(found.len(), 1, "expected one `{name}` under {root}");
    found[0]
}

/// What a root holds, as each child's name, local transform and brush, in
/// order, so two builds can be compared without their ids. A copy's number,
/// which counts copies across the whole scene, is left off its name.
pub(super) fn contents(app: &App, root: Entity) -> Vec<String> {
    let world = app.world();
    world
        .get::<Children>(root)
        .map(|children| {
            children
                .iter()
                .map(|child| {
                    let name = world.get::<Name>(child).map(|name| {
                        name.trim_end_matches(|c: char| c.is_ascii_digit())
                            .to_string()
                    });
                    let transform = world.get::<Transform>(child).map(|t| {
                        let r = |v: Vec3| (v * 1000.0).round() / 1000.0;
                        (r(t.translation), r(t.scale))
                    });
                    let brush = world
                        .get::<Brush>(child)
                        .map(|brush| format!("{:?}", brush.topology));
                    let tags = world.get::<Tags>(child).cloned();
                    format!("{name:?} {transform:?} {tags:?} {brush:?}")
                })
                .collect()
        })
        .unwrap_or_default()
}

const LINE_RECIPE: &str = r#"
    description: "Posts in a row.",
    inputs: [
        jackdaw_editor_core::recipes::RecipeInput { name: "count", kind: "int", default: "3", range: [2.0, 10.0] },
        jackdaw_editor_core::recipes::RecipeInput { name: "gap", kind: "float", default: "1.5", range: [] },
    ],
    calls: [
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.add.cube", params: map[
            ("name", "Post"),
            ("size", "[0.2, $gap * 0.5, 0.2]"),
            ("pos", "[0, $gap * 0.25, 0]"),
        ] },
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.duplicate", params: map[
            ("entity", "$0.entities.0"),
            ("count", "$count - 1"),
            ("offset", "[$gap, 0, 0]"),
        ] },
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.tag", params: map[
            ("entities", "$0.entities.0"),
            ("add", "first"),
        ] },
    ],"#;

/// Running a recipe builds what its calls build when run by hand into a group
/// placed the same way.
#[test]
fn a_recipe_run_builds_what_its_calls_build_by_hand() {
    let (mut app, dir) = recipe_project();
    write_recipe(&mut app, &dir, "line", LINE_RECIPE);
    let root = run(
        &mut app,
        json!({ "recipe": "line", "inputs": { "count": 4, "gap": 2.0 }, "pos": [3.0, 0.0, 1.0] }),
    );

    let group = finished(
        &mut app,
        "entity.add.group",
        json!({ "name": "By Hand", "pos": [3.0, 0.0, 1.0] }),
    )["entities"][0]
        .clone();
    let post = finished(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Post", "size": [0.2, 1.0, 0.2], "pos": [0.0, 0.5, 0.0], "parent": group }),
    )["entities"][0]
        .clone();
    finished(
        &mut app,
        "entity.duplicate",
        json!({ "entity": post, "count": 3, "offset": [2.0, 0.0, 0.0] }),
    );
    finished(
        &mut app,
        "entity.tag",
        json!({ "entities": post, "add": "first" }),
    );
    let group = Entity::from_bits(group.as_u64().expect("an id"));

    let built = contents(&app, root);
    assert_eq!(built.len(), 4, "{built:#?}");
    assert_eq!(built, contents(&app, group));
    assert_eq!(
        app.world().get::<Transform>(root).map(|t| t.translation),
        Some(Vec3::new(3.0, 0.0, 1.0))
    );
    assert_eq!(
        app.world().get::<Name>(root).map(Name::as_str),
        Some("Line"),
        "the root takes the recipe's name"
    );
    let instance = app
        .world()
        .get::<RecipeInstance>(root)
        .expect("the root remembers its recipe");
    assert_eq!(instance.recipe, "recipes/line.recipe.bsn");
    assert_eq!(instance.inputs.get("count").map(String::as_str), Some("4"));
    assert_eq!(instance.inputs.get("gap").map(String::as_str), Some("2"));
}

/// An input out of range, of the wrong type or not declared refuses the run
/// before anything changes, and the refusal names it.
#[test]
fn an_invalid_input_leaves_the_scene_unchanged_and_names_the_input() {
    let (mut app, _dir) = recipe_project();
    let text = scene_text(&mut app);
    let depth = undo_depth(&app);
    for (inputs, says) in [
        (json!({ "floors": 99 }), "`floors`"),
        (json!({ "floors": "two" }), "`floors`"),
        (json!({ "floors": 1.5 }), "`floors`"),
        (json!({ "width": [1, 2] }), "`width`"),
        (json!({ "storeys": 2 }), "`storeys`"),
    ] {
        let why = refused(
            &mut app,
            "recipe.run",
            json!({ "recipe": "house", "inputs": inputs }),
        );
        assert!(why.contains(says), "{inputs}: {why}");
        assert_eq!(scene_text(&mut app), text, "{inputs} changed the scene");
        assert_eq!(undo_depth(&app), depth, "{inputs} left an undo entry");
    }
    let why = refused(&mut app, "recipe.run", json!({ "recipe": "castle" }));
    assert!(why.contains("castle"), "{why}");
}

/// A call that fails part way takes the whole run back: no root, no entry.
#[test]
fn a_call_that_fails_rolls_the_whole_run_back() {
    let (mut app, dir) = recipe_project();
    write_recipe(
        &mut app,
        &dir,
        "broken",
        r#"
    inputs: [],
    calls: [
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.add.cube", params: map[("name", "Base")] },
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.duplicate", params: map[("entity", "$0.entities.4")] },
    ],"#,
    );
    let text = scene_text(&mut app);
    let depth = undo_depth(&app);
    let why = refused(&mut app, "recipe.run", json!({ "recipe": "broken" }));
    assert!(why.contains("call 1"), "{why}");
    assert_eq!(scene_text(&mut app), text);
    assert_eq!(undo_depth(&app), depth);
}

/// A recipe that would write to disk is refused before it runs, since a run
/// is taken back whole when it fails.
#[test]
fn a_recipe_that_writes_to_disk_is_refused_before_it_runs() {
    let (mut app, dir) = recipe_project();
    write_recipe(
        &mut app,
        &dir,
        "saver",
        r#"
    inputs: [],
    calls: [
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.add.cube", params: map[("name", "Base")] },
        jackdaw_editor_core::recipes::RecipeCall { id: "scene.save", params: map[("path", "scenes/out.bsn")] },
    ],"#,
    );
    let text = scene_text(&mut app);
    let why = refused(&mut app, "recipe.run", json!({ "recipe": "saver" }));
    assert!(why.contains("scene.save") && why.contains("disk"), "{why}");
    assert_eq!(scene_text(&mut app), text);
    assert!(!dir.path().join("assets/scenes/out.bsn").exists());
}

/// A run and a re-run are one undo step each; the re-run keeps the root's id,
/// name, transform and tags and replaces only what it holds.
#[test]
fn a_rerun_is_one_undo_step_and_keeps_the_root() {
    let (mut app, _dir) = recipe_project();
    let depth = undo_depth(&app);
    let root = run(
        &mut app,
        json!({ "recipe": "stairs", "inputs": { "steps": 3 }, "name": "Front Steps" }),
    );
    assert_eq!(undo_depth(&app), depth + 1, "one entry for the run");
    let bits = root.to_bits();
    finished(
        &mut app,
        "entity.set_transform",
        json!({ "entity": bits, "x": 4.0, "y": 0.0, "z": -2.0 }),
    );
    finished(
        &mut app,
        "entity.tag",
        json!({ "entities": bits, "add": "entrance" }),
    );
    assert_eq!(contents(&app, root).len(), 3);

    let depth = undo_depth(&app);
    let answer = finished(
        &mut app,
        "recipe.rerun",
        json!({ "entity": bits, "inputs": { "steps": 5 } }),
    );
    assert_eq!(undo_depth(&app), depth + 1, "one entry for the re-run");
    assert!(
        answer["reports"][0]
            .as_str()
            .is_some_and(|line| line.contains("3 replaced")),
        "{answer}"
    );
    assert!(app.world().get_entity(root).is_ok(), "the root kept its id");
    assert_eq!(contents(&app, root).len(), 5);
    assert_eq!(
        app.world().get::<Name>(root).map(Name::as_str),
        Some("Front Steps")
    );
    assert_eq!(
        app.world().get::<Transform>(root).map(|t| t.translation),
        Some(Vec3::new(4.0, 0.0, -2.0))
    );
    assert!(
        app.world()
            .get::<Tags>(root)
            .is_some_and(|tags| tags.contains("entrance"))
    );
    let instance = app.world().get::<RecipeInstance>(root).expect("instance");
    assert_eq!(instance.inputs.get("steps").map(String::as_str), Some("5"));
    assert_eq!(
        instance.inputs.get("width").map(String::as_str),
        Some("1.2")
    );

    undo(&mut app);
    let root = roots_named(&mut app, "Front Steps")[0];
    assert_eq!(
        contents(&app, root).len(),
        3,
        "undo brings the first build back"
    );
}

/// A bad input on a re-run leaves the root and what it holds alone.
#[test]
fn a_rerun_with_a_bad_input_changes_nothing() {
    let (mut app, _dir) = recipe_project();
    let root = run(&mut app, json!({ "recipe": "fence" }));
    let text = scene_text(&mut app);
    let depth = undo_depth(&app);
    let why = refused(
        &mut app,
        "recipe.rerun",
        json!({ "entity": root.to_bits(), "inputs": { "posts": 1 } }),
    );
    assert!(why.contains("`posts`"), "{why}");
    assert_eq!(scene_text(&mut app), text);
    assert_eq!(undo_depth(&app), depth);
}

/// Saving and reopening keeps the root's `RecipeInstance`, and a re-run
/// after the reopen rebuilds it.
#[test]
fn a_recipe_root_survives_save_and_reopen_and_reruns() {
    let (mut app, dir) = recipe_project();
    run(
        &mut app,
        json!({ "recipe": "house", "inputs": { "width": 8.0 }, "seed": 7 }),
    );
    let path = dir.path().join("assets/scenes/village.bsn");
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("a scenes dir");
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
    open_project(&mut app, &dir);
    app.world_mut()
        .operator("scene.open")
        .param("path", path.to_string_lossy().into_owned())
        .call()
        .expect("scene.open dispatches")
        .assert_finished();
    util::settle_scene_load(&mut app);
    let roots = roots_named(&mut app, "House");
    assert_eq!(roots.len(), 1, "the root and its RecipeInstance load");
    let root = roots[0];
    let instance = app.world().get::<RecipeInstance>(root).expect("instance");
    assert_eq!(instance.recipe, "starter:house");
    assert_eq!(instance.seed, 7);
    assert_eq!(instance.inputs.get("width").map(String::as_str), Some("8"));

    finished(
        &mut app,
        "recipe.rerun",
        json!({ "entity": root.to_bits(), "inputs": { "floors": 2 } }),
    );
    let wall = child(&app, root, "Wall_Front");
    let y = app
        .world()
        .get::<Transform>(wall)
        .expect("transform")
        .translation
        .y;
    assert!((y - (0.2 + 2.0 * 2.8 * 0.5)).abs() < 1e-4, "{y}");
    let instance = app.world().get::<RecipeInstance>(root).expect("instance");
    assert_eq!(instance.inputs.get("width").map(String::as_str), Some("8"));
    assert_eq!(instance.inputs.get("floors").map(String::as_str), Some("2"));
}

/// The same seed builds the same shape; another seed builds another.
#[test]
fn the_seed_fixes_the_result() {
    let (mut app, _dir) = recipe_project();
    let a = run(&mut app, json!({ "recipe": "rock", "seed": 3 }));
    let b = run(&mut app, json!({ "recipe": "rock", "seed": 3 }));
    let c = run(&mut app, json!({ "recipe": "rock", "seed": 4 }));
    assert_eq!(contents(&app, a), contents(&app, b));
    assert_ne!(contents(&app, a), contents(&app, c));

    finished(
        &mut app,
        "recipe.rerun",
        json!({ "entity": c.to_bits(), "seed": 3 }),
    );
    assert_eq!(
        contents(&app, a),
        contents(&app, c),
        "a re-run takes a new seed"
    );
}

/// Every recipe the editor ships runs cleanly with its defaults.
#[test]
fn every_starter_recipe_runs_cleanly() {
    let (mut app, _dir) = recipe_project();
    for recipe in jackdaw::recipes::STARTER_RECIPES {
        let answer = finished(&mut app, "recipe.run", json!({ "recipe": recipe }));
        assert!(
            answer.get("warnings").is_none(),
            "{recipe} ran with warnings: {answer}"
        );
        let root = Entity::from_bits(answer["entities"][0].as_u64().expect("an id"));
        assert!(
            !contents(&app, root).is_empty(),
            "{recipe} built nothing under its root"
        );
        assert_eq!(
            app.world()
                .get::<RecipeInstance>(root)
                .map(|instance| instance.recipe.clone()),
            Some(format!("starter:{recipe}"))
        );
    }
}

/// The starters that draw from the seed build cleanly across many seeds.
#[test]
fn seeded_starters_run_cleanly_across_seeds() {
    let (mut app, _dir) = recipe_project();
    for recipe in ["rock", "tree"] {
        for seed in 0..24 {
            let answer = call(
                &mut app,
                "recipe.run",
                json!({ "recipe": recipe, "seed": seed }),
            );
            assert_eq!(
                answer["result"],
                json!("finished"),
                "{recipe} seed {seed}: {answer}"
            );
        }
    }
}

/// The agent's loop: three different houses from three runs, then one of
/// them given another floor count. References inside each house reach only
/// that house, though every house has a `Roof`.
#[test]
fn three_houses_from_three_runs_and_one_rerun() {
    let (mut app, _dir) = recipe_project();
    let houses: Vec<Entity> = [
        json!({ "recipe": "house", "name": "House A", "inputs": { "width": 6.0 }, "pos": [0.0, 0.0, 0.0] }),
        json!({ "recipe": "house", "name": "House B", "inputs": { "width": 9.0, "floors": 2 }, "pos": [12.0, 0.0, 0.0] }),
        json!({ "recipe": "house", "name": "House C", "inputs": { "depth": 8.0 }, "pos": [24.0, 0.0, 0.0], "rot": [30.0, 0.0, 0.0] }),
    ]
    .into_iter()
    .map(|params| run(&mut app, params))
    .collect();
    let built: Vec<Vec<String>> = houses.iter().map(|&house| contents(&app, house)).collect();
    assert_ne!(built[0], built[1]);
    assert_ne!(built[0], built[2]);
    for &house in &houses {
        let roof = child(&app, house, "Roof");
        assert!(
            app.world()
                .get::<Tags>(roof)
                .is_some_and(|tags| tags.contains("roof")),
            "each house tags its own roof"
        );
    }

    let wall_y = |app: &App, house| {
        let wall = child(app, house, "Wall_Front");
        app.world()
            .get::<Transform>(wall)
            .expect("transform")
            .translation
            .y
    };
    let before = wall_y(&app, houses[0]);
    finished(
        &mut app,
        "recipe.rerun",
        json!({ "entity": houses[0].to_bits(), "inputs": { "floors": 3 } }),
    );
    assert!(wall_y(&app, houses[0]) > before + 2.0, "the walls rose");
    assert_eq!(
        contents(&app, houses[1]),
        built[1],
        "the other houses are alone"
    );
    assert_eq!(contents(&app, houses[2]), built[2]);
}

/// Inside a recipe a path starts at the recipe's root, an exact name holding
/// `&` still names its node, and a bare `name=` target reaches only the
/// root's own nodes, though a node of that name stands outside it.
#[test]
fn references_inside_a_recipe_are_held_to_its_root() {
    let (mut app, dir) = recipe_project();
    finished(&mut app, "entity.add.cube", json!({ "name": "Roof" }));
    finished(&mut app, "entity.add.group", json!({ "name": "Attic" }));
    write_recipe(
        &mut app,
        &dir,
        "porch",
        r#"
    inputs: [],
    calls: [
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.add.group", params: map[("name", "Attic")] },
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.add.cube", params: map[("name", "Window"), ("parent", "$0.entities.0")] },
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.tag", params: map[("entities", "Attic/Window"), ("add", "glass")] },
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.add.cube", params: map[("name", "Door&Frame"), ("pos", "[2, 0, 0]")] },
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.tag", params: map[("entities", "Door&Frame"), ("add", "door")] },
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.add.cube", params: map[("name", "Roof"), ("pos", "[0, 3, 0]")] },
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.delete", params: map[("name", "Roof")] },
    ],"#,
    );
    let root = run(&mut app, json!({ "recipe": "porch" }));
    let attic = child(&app, root, "Attic");
    let window = child(&app, attic, "Window");
    assert!(
        app.world()
            .get::<Tags>(window)
            .is_some_and(|tags| tags.contains("glass")),
        "Attic/Window starts at the recipe's root"
    );
    let door = child(&app, root, "Door&Frame");
    assert!(
        app.world()
            .get::<Tags>(door)
            .is_some_and(|tags| tags.contains("door"))
    );
    let built = contents(&app, root);
    assert!(
        !built.iter().any(|line| line.contains("\"Roof\"")),
        "the recipe's own Roof is deleted: {built:#?}"
    );
    let roofs = app
        .world_mut()
        .query::<&Name>()
        .iter(app.world())
        .filter(|name| name.as_str() == "Roof")
        .count();
    assert_eq!(roofs, 1, "the Roof outside the root is left alone");
}

/// A recipe path that is absolute or climbs out of the assets directory is
/// refused.
#[test]
fn a_recipe_path_outside_the_assets_directory_is_refused() {
    let (mut app, dir) = recipe_project();
    std::fs::write(
        dir.path().join("outside.recipe.bsn"),
        "// jackdaw asset jackdaw_editor_core::recipes::Recipe\n#outside\njackdaw_editor_core::recipes::Recipe { inputs: [], calls: [] }\n",
    )
    .expect("write a file beside the assets");
    let absolute = dir.path().join("outside.recipe.bsn");
    for reference in [
        "../outside.recipe.bsn".to_string(),
        absolute.to_string_lossy().into_owned(),
    ] {
        let why = refused(&mut app, "recipe.run", json!({ "recipe": reference }));
        assert!(why.contains("outside the assets"), "{reference}: {why}");
    }
}

/// A root built from a starter keeps rebuilding from it after a project
/// recipe of the same name appears.
#[test]
fn a_root_rebuilds_from_the_recipe_it_was_built_from() {
    let (mut app, dir) = recipe_project();
    let root = run(
        &mut app,
        json!({ "recipe": "stairs", "inputs": { "steps": 3 } }),
    );
    write_recipe(
        &mut app,
        &dir,
        "stairs",
        r#"
    inputs: [],
    calls: [
        jackdaw_editor_core::recipes::RecipeCall { id: "entity.add.cube", params: map[("name", "Impostor")] },
    ],"#,
    );
    let fresh = run(&mut app, json!({ "recipe": "stairs" }));
    assert_eq!(
        app.world()
            .get::<RecipeInstance>(fresh)
            .map(|i| i.recipe.clone()),
        Some("recipes/stairs.recipe.bsn".to_string()),
        "a bare name now finds the project recipe"
    );
    finished(
        &mut app,
        "recipe.rerun",
        json!({ "entity": root.to_bits(), "inputs": { "steps": 4 } }),
    );
    assert_eq!(
        app.world()
            .get::<RecipeInstance>(root)
            .map(|i| i.recipe.clone()),
        Some("starter:stairs".to_string())
    );
    assert_eq!(
        contents(&app, root).len(),
        4,
        "the old root still builds stairs"
    );
}
