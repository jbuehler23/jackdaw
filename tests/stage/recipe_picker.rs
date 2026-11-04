//! The Add menu's Recipe entry: one searchable list of the project's recipes
//! and the ones the editor ships, building the one picked.

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw_api::prelude::*;
use jackdaw_feathers::picker::{PickerItems, PickerSelect};
use jackdaw_scene_types::RecipeInstance;

use crate::util;
use crate::util::OperatorResultExt as _;

const LINE: &str = "// jackdaw asset jackdaw_editor_core::recipes::Recipe\n#post\njackdaw_editor_core::recipes::Recipe {\n    description: \"A post.\",\n    inputs: [],\n    calls: [\n        jackdaw_editor_core::recipes::RecipeCall { id: \"entity.add.cube\", params: map[(\"name\", \"Post\")] },\n    ],\n}\n";

fn settle(app: &mut App) {
    for _ in 0..4 {
        app.update();
    }
}

fn listed(app: &mut App) -> (Entity, Vec<String>) {
    let lists: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<jackdaw::recipes::RecipeList>>()
        .iter(app.world())
        .collect();
    assert_eq!(lists.len(), 1, "one recipe list is open");
    let lines = app
        .world()
        .get::<PickerItems<String>>(lists[0])
        .map(|items| items.items().to_vec())
        .unwrap_or_default();
    (lists[0], lines)
}

#[test]
fn the_recipe_entry_lists_starters_and_project_recipes_and_builds_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("assets/props")).expect("assets");
    std::fs::write(dir.path().join("assets/props/post.recipe.bsn"), LINE).expect("a recipe");
    let mut app = util::editor_test_app();
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

    let items = jackdaw::add_entity_picker::collect_add_menu_items(app.world_mut());
    let entry: Vec<_> = items
        .iter()
        .filter(|item| item.action == "op:recipe.picker")
        .collect();
    assert_eq!(entry.len(), 1, "one Recipe entry");
    assert_eq!(entry[0].label, "Recipe");

    app.world_mut()
        .operator("recipe.picker")
        .call()
        .expect("recipe.picker dispatches")
        .assert_finished();
    settle(&mut app);
    let (_, lines) = listed(&mut app);
    assert_eq!(
        lines,
        [
            "post  props/post.recipe.bsn",
            "tree  starter",
            "rock  starter",
            "fence  starter",
            "house  starter",
            "stairs  starter",
        ]
    );

    let depth = app.world().resource::<CommandHistory>().undo_stack.len();
    for (line, recipe, name) in [
        ("house  starter", "starter:house", "House"),
        (
            "post  props/post.recipe.bsn",
            "props/post.recipe.bsn",
            "Post",
        ),
    ] {
        app.world_mut()
            .operator("recipe.picker")
            .call()
            .expect("recipe.picker dispatches")
            .assert_finished();
        settle(&mut app);
        let (list_now, lines) = listed(&mut app);
        let index = lines.iter().position(|held| held == line).expect("listed");
        app.world_mut().trigger(PickerSelect {
            entity: list_now,
            index,
        });
        settle(&mut app);
        let found: Vec<(Entity, String, Transform)> = app
            .world_mut()
            .query::<(Entity, &RecipeInstance, &Name, &Transform)>()
            .iter(app.world())
            .filter(|(_, instance, ..)| instance.recipe == recipe)
            .map(|(entity, _, held, transform)| (entity, held.to_string(), *transform))
            .collect();
        assert_eq!(found.len(), 1, "{recipe} was built");
        assert_eq!(found[0].1, name);
        assert_eq!(
            found[0].2.translation,
            Vec3::ZERO,
            "where an add puts a node"
        );
    }
    assert_eq!(
        app.world().resource::<CommandHistory>().undo_stack.len(),
        depth + 2,
        "one undo step per pick"
    );
}
