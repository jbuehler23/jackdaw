//! The recipe card: a recipe root's inputs as fields that rebuild it as they
//! change, one undo step per drag, and its calls under a collapsed Steps
//! section, where a literal can be promoted to an input.

use bevy::prelude::*;
use bevy::ui_widgets::ValueChange;
use jackdaw::commands::CommandHistory;
use jackdaw::inspector::recipe_card::{
    PromoteText, RecipeInputField, RecipePromoteButton, RecipePromoteField, RecipePromoteForm,
    RecipePromoteToggle, RecipeStepParam, RecipeSteps,
};
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw::remote::server::call_operator_handler;
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::ButtonClickEvent;
use jackdaw_feathers::text_edit::TextEditCommitEvent;
use jackdaw_scene_types::RecipeInstance;
use jackdaw_widgets::collapsible::CollapsibleSection;
use serde_json::{Value, json};

use crate::util;
use crate::util::OperatorResultExt as _;

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

/// An editor with the inspector up and a scene open in a project of its own.
fn recipe_inspector() -> (App, tempfile::TempDir) {
    let dir = tempfile::Builder::new()
        .prefix("jackdaw-recipe-card-")
        .tempdir()
        .expect("temp dir");
    std::fs::create_dir_all(dir.path().join("assets/recipes")).expect("an assets dir");
    let mut app = util::editor_test_app();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
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
    settle(&mut app);
    (app, dir)
}

#[track_caller]
fn finished(app: &mut App, id: &str, params: Value) -> Value {
    let answer = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": id, "params": params })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("{id} refused: {}", err.message));
    assert_eq!(answer["result"], json!("finished"), "{id}: {answer}");
    settle(app);
    answer
}

fn root_of(answer: &Value) -> Entity {
    Entity::from_bits(answer["entities"][0].as_u64().expect("the root"))
}

fn all<T: Component>(app: &mut App) -> Vec<Entity> {
    app.world_mut()
        .query_filtered::<Entity, With<T>>()
        .iter(app.world())
        .collect()
}

fn input_field(app: &mut App, input: &str) -> Entity {
    let found: Vec<Entity> = all::<RecipeInputField>(app)
        .into_iter()
        .filter(|&field| {
            app.world()
                .get::<RecipeInputField>(field)
                .is_some_and(|field| field.input == input)
        })
        .collect();
    assert_eq!(found.len(), 1, "one `{input}` field");
    found[0]
}

fn drag(app: &mut App, field: Entity, value: f64, is_final: bool) {
    app.world_mut().trigger(ValueChange {
        source: field,
        value,
        is_final,
    });
    settle(app);
}

fn undo_depth(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn input(app: &App, root: Entity, name: &str) -> Option<String> {
    app.world()
        .get::<RecipeInstance>(root)
        .and_then(|instance| instance.inputs.get(name).cloned())
}

/// The card offers every input as a field and keeps the calls folded away;
/// a drag rebuilds the root on every value, keeps its id, and is one undo
/// step.
#[test]
fn an_input_drag_reruns_the_root_as_one_undo_step() {
    let (mut app, _dir) = recipe_inspector();
    let root = root_of(&finished(
        &mut app,
        "recipe.run",
        json!({ "recipe": "house" }),
    ));

    assert_eq!(
        all::<RecipeInputField>(&mut app).len(),
        7,
        "one field per input"
    );
    let steps = all::<RecipeSteps>(&mut app);
    assert_eq!(steps.len(), 1);
    assert_eq!(
        app.world()
            .get::<CollapsibleSection>(steps[0])
            .map(|section| section.collapsed),
        Some(true),
        "the steps start folded"
    );
    assert_eq!(
        all::<RecipeStepParam>(&mut app).len(),
        30,
        "every call's parameters"
    );
    assert!(
        all::<RecipePromoteToggle>(&mut app).is_empty(),
        "a starter cannot be changed in place"
    );

    let depth = undo_depth(&app);
    let width = input_field(&mut app, "width");
    drag(&mut app, width, 7.0, false);
    assert_eq!(
        input(&app, root, "width").as_deref(),
        Some("7"),
        "live rerun"
    );
    drag(&mut app, width, 8.5, false);
    drag(&mut app, width, 9.0, true);
    assert_eq!(input(&app, root, "width").as_deref(), Some("9"));
    assert_eq!(undo_depth(&app), depth + 1, "one undo step for the drag");
    assert_eq!(
        app.world().get::<Children>(root).map(Children::len),
        Some(6),
        "the same root holds the rebuilt house"
    );

    let floors = input_field(&mut app, "floors");
    drag(&mut app, floors, 2.4, true);
    assert_eq!(
        input(&app, root, "floors").as_deref(),
        Some("2"),
        "an int input rounds"
    );
    assert_eq!(undo_depth(&app), depth + 2);

    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("undo")
        .assert_finished();
    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("undo")
        .assert_finished();
    settle(&mut app);
    let roots: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<RecipeInstance>>()
        .iter(app.world())
        .collect();
    assert_eq!(roots.len(), 1);
    assert_eq!(input(&app, roots[0], "width").as_deref(), Some("6"));
}

/// A literal in a recorded recipe's step is promoted from the card, with the
/// name and range typed into its form.
#[test]
fn a_step_literal_is_promoted_from_the_card() {
    let (mut app, dir) = recipe_inspector();
    finished(&mut app, "recipe.record.start", json!({}));
    finished(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Post", "size": [0.5, 2.0, 0.5] }),
    );
    finished(&mut app, "recipe.record.stop", json!({ "name": "post" }));

    let toggle = all::<RecipePromoteToggle>(&mut app)
        .into_iter()
        .find(|&toggle| {
            let form = app
                .world()
                .get::<RecipePromoteToggle>(toggle)
                .expect("a toggle")
                .form;
            app.world()
                .get::<RecipePromoteForm>(form)
                .is_some_and(|form| form.param == "size")
        })
        .expect("the size literal can be promoted");
    let form = app
        .world()
        .get::<RecipePromoteToggle>(toggle)
        .expect("a toggle")
        .form;
    assert_eq!(
        app.world().get::<Node>(form).map(|node| node.display),
        Some(Display::None),
        "the form waits to be asked for"
    );
    app.world_mut().trigger(ButtonClickEvent { entity: toggle });
    settle(&mut app);
    assert_eq!(
        app.world().get::<Node>(form).map(|node| node.display),
        Some(Display::Flex)
    );

    for (which, text) in [
        (PromoteText::Name, "post_size"),
        (PromoteText::Min, "0.1"),
        (PromoteText::Max, "10"),
    ] {
        let field = all::<RecipePromoteField>(&mut app)
            .into_iter()
            .find(|&field| {
                app.world()
                    .get::<RecipePromoteField>(field)
                    .is_some_and(|field| field.form == form && field.text == which)
            })
            .expect("the form's field");
        app.world_mut().trigger(TextEditCommitEvent {
            entity: field,
            text: text.to_string(),
        });
    }
    let button = all::<RecipePromoteButton>(&mut app)
        .into_iter()
        .find(|&button| {
            app.world()
                .get::<RecipePromoteButton>(button)
                .is_some_and(|button| button.form == form)
        })
        .expect("the form's button");
    app.world_mut().trigger(ButtonClickEvent { entity: button });
    settle(&mut app);

    let text =
        std::fs::read_to_string(dir.path().join("assets/recipes/post.recipe.bsn")).expect("saved");
    assert!(
        text.contains(
            "name: \"post_size\", kind: \"vec3\", default: \"[0.5, 2, 0.5]\", range: [0.1, 10.0]"
        ),
        "{text}"
    );
    assert_eq!(
        all::<RecipeInputField>(&mut app).len(),
        3,
        "the card shows the new input, one field per component"
    );
}

const FLAG_RECIPE: &str = "// jackdaw asset jackdaw_editor_core::recipes::Recipe\n#flag\njackdaw_editor_core::recipes::Recipe {\n    description: \"A box.\",\n    inputs: [\n        jackdaw_editor_core::recipes::RecipeInput { name: \"lit\", kind: \"bool\", default: \"false\", range: [] },\n        jackdaw_editor_core::recipes::RecipeInput { name: \"label\", kind: \"text\", default: \"Box\", range: [] },\n    ],\n    calls: [\n        jackdaw_editor_core::recipes::RecipeCall { id: \"entity.add.cube\", params: map[(\"name\", \"$label\")] },\n    ],\n}\n";

fn checked(app: &App, field: Entity) -> bool {
    app.world().get::<bevy::ui::Checked>(field).is_some()
}

fn shown_text(app: &mut App, field: Entity) -> String {
    let inner: Vec<Entity> = app
        .world()
        .get::<Children>(field)
        .into_iter()
        .flat_map(Children::iter)
        .filter_map(|child| {
            app.world()
                .get::<jackdaw_feathers::text_edit::TextEditWrapper>(child)
                .map(|wrapper| wrapper.0)
        })
        .collect();
    inner
        .first()
        .and_then(|inner| app.world().get::<bevy::text::EditableText>(*inner))
        .map(|editable| editable.value().into_owned())
        .expect("a text field")
}

/// A bool input's box follows its value both ways, and a text input's field
/// follows an undo.
#[test]
fn bool_and_text_inputs_follow_their_values() {
    let (mut app, dir) = recipe_inspector();
    std::fs::write(
        dir.path().join("assets/recipes/flag.recipe.bsn"),
        FLAG_RECIPE,
    )
    .expect("a recipe");
    jackdaw::asset_index::rescan_asset_index(app.world_mut());
    let root = root_of(&finished(
        &mut app,
        "recipe.run",
        json!({ "recipe": "flag" }),
    ));

    let lit = input_field(&mut app, "lit");
    assert!(!checked(&app, lit));
    for on in [true, false] {
        app.world_mut().trigger(ValueChange {
            source: lit,
            value: on,
            is_final: true,
        });
        settle(&mut app);
        assert_eq!(checked(&app, lit), on, "the box shows {on}");
        assert_eq!(input(&app, root, "lit"), Some(on.to_string()));
    }
    app.world_mut()
        .operator("history.undo")
        .call()
        .expect("undo")
        .assert_finished();
    settle(&mut app);
    let lit = input_field(&mut app, "lit");
    assert!(checked(&app, lit), "undo puts the box back");

    let label = input_field(&mut app, "label");
    app.world_mut().trigger(TextEditCommitEvent {
        entity: label,
        text: "Crate".to_string(),
    });
    settle(&mut app);
    let root = app
        .world_mut()
        .query_filtered::<Entity, With<RecipeInstance>>()
        .single(app.world())
        .expect("one root");
    assert_eq!(input(&app, root, "label").as_deref(), Some("Crate"));
    finished(
        &mut app,
        "recipe.rerun",
        json!({ "entity": root.to_bits(), "inputs": { "label": "Barrel" } }),
    );
    let label = input_field(&mut app, "label");
    assert_eq!(
        shown_text(&mut app, label),
        "Barrel",
        "a caller's rerun shows"
    );
}
