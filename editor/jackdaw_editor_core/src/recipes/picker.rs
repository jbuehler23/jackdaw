//! The Add menu's Recipe entry: a searchable list of the project's recipes and
//! the ones the editor ships, building the one picked.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_feathers::picker::{PickerProps, SelectInput, SpawnItemInput, match_text, picker_item};

use super::{RECIPE_EXTENSION, RecipeRunOp, STARTER_PREFIX, STARTER_RECIPES};

/// The open recipe list, with the recipe each row stands for, in row order.
#[derive(Component)]
pub struct RecipeList {
    pub recipes: Vec<String>,
}

/// Open the recipe list.
#[operator(
    id = "recipe.picker",
    label = "Recipe",
    description = "Open the searchable list of recipes, the project's and the ones the editor \
                   ships, and build the one picked.",
    allows_undo = false
)]
pub(crate) fn recipe_picker(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(open_recipe_list);
    OperatorResult::Finished
}

/// Every recipe on offer, as the line a row shows and the reference
/// `recipe.run` takes: the project's first, then the starters.
pub fn recipe_choices(world: &World) -> Vec<(String, String)> {
    let mut choices: Vec<(String, String)> = Vec::new();
    if let Some(assets) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(crate::project::ProjectRoot::assets_dir)
    {
        let mut found: Vec<String> = jackdaw_bsn::walk_document_files(&assets)
            .into_iter()
            .filter_map(|path| {
                path.strip_prefix(&assets)
                    .ok()
                    .map(|relative| relative.to_string_lossy().replace('\\', "/"))
            })
            .filter(|relative| relative.ends_with(RECIPE_EXTENSION))
            .collect();
        found.sort();
        choices.extend(found.into_iter().map(|relative| {
            let stem = jackdaw_bsn::asset_stem(&relative).to_string();
            (format!("{stem}  {relative}"), relative)
        }));
    }
    choices.extend(STARTER_RECIPES.iter().map(|name| {
        (
            format!("{name}  starter"),
            format!("{STARTER_PREFIX}{name}"),
        )
    }));
    choices
}

/// Put up the recipe list in place of one already open.
pub fn open_recipe_list(world: &mut World) {
    let open: Vec<Entity> = world
        .query_filtered::<Entity, With<RecipeList>>()
        .iter(world)
        .collect();
    for list in open {
        if let Ok(entity) = world.get_entity_mut(list) {
            entity.despawn();
        }
    }
    let (lines, recipes): (Vec<String>, Vec<String>) = recipe_choices(world).into_iter().unzip();
    report_to_caller(world, lines.join("; "));
    world.commands().spawn((
        PickerProps::new(spawn_recipe_item, pick_recipe)
            .items(lines)
            .title("Recipe")
            .placeholder(Some("Search recipes..")),
        RecipeList { recipes },
        crate::EditorEntity,
        crate::BlocksCameraInput,
    ));
    world.flush();
}

fn spawn_recipe_item(
    In(SpawnItemInput { matched, entities }): In<SpawnItemInput>,
    mut commands: Commands,
) -> Result {
    commands.spawn((
        picker_item(matched.index),
        ChildOf(entities.list),
        children![match_text(matched.segments)],
    ));
    Ok(())
}

fn pick_recipe(
    input: In<SelectInput>,
    lists: Query<&RecipeList>,
    mut commands: Commands,
) -> Result {
    let picker = input.entities.picker;
    let chosen = lists.get(picker)?.recipes.get(input.index).cloned();
    commands.entity(picker).try_despawn();
    if let Some(recipe) = chosen {
        commands
            .operator(RecipeRunOp::ID)
            .param("recipe", recipe)
            .settings(CallOperatorSettings {
                creates_history_entry: true,
                execution_context: ExecutionContext::Invoke,
            })
            .call();
    }
    Ok(())
}
