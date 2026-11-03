//! Promoting a literal parameter of a recipe step to a named input, so a
//! recorded recipe gains the inputs a person picks after recording it.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::ParamSpec;
use jackdaw_recipe::{InputKind, InputSpec};
use jackdaw_scene_types::{PropertyValue, RecipeInstance};

use super::{
    RecipeInput, STARTER_PREFIX, assets_dir, inside_assets, read_recipe_file, recipe_file_text,
    refuse, rerun_recipe,
};

/// Turn one step's literal parameter into an input of the recipe.
#[operator(
    id = "recipe.promote",
    label = "Promote to Input",
    description = "Turn a literal parameter of one step of a project recipe into a named \
                   input, save the recipe, and rebuild the root with it.",
    allows_undo = false,
    writes_disk = true,
    params(
        entity(Entity, doc = "Root a recipe built."),
        step(i64, doc = "Index of the step among the recipe's calls."),
        param(String, doc = "Parameter of that step to promote."),
        input(
            String,
            doc = "Name for the new input. Defaults to the parameter's name."
        ),
        kind(
            String,
            doc = "float, int, bool, vec2, vec3 or text. Defaults to what the parameter takes."
        ),
        default(
            String,
            doc = "Default value, as text. Defaults to the value the step holds."
        ),
        min(f64, doc = "Least value, given with max. Defaults to no range."),
        max(f64, doc = "Greatest value, given with min. Defaults to no range."),
    )
)]
pub(crate) fn recipe_promote(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    match promote(world, &params) {
        Ok(()) => OperatorResult::Finished,
        Err(why) => refuse(world, RecipePromoteOp::ID, why),
    }
}

fn promote(world: &mut World, params: &OperatorParameters) -> Result<(), String> {
    let root = params
        .as_entity("entity")
        .ok_or("`entity` names no entity")?;
    let reference = world
        .get::<RecipeInstance>(root)
        .map(|instance| instance.recipe.clone())
        .ok_or("that entity is not the root of a recipe")?;
    if reference.starts_with(STARTER_PREFIX) {
        return Err(format!(
            "`{reference}` ships with the editor and cannot be changed; record your own recipe \
             to promote its values"
        ));
    }
    let assets = assets_dir(world, &reference)?;
    let path = inside_assets(&assets, &reference)?;
    let mut recipe = read_recipe_file(&path)?;

    let step = params.as_int("step").ok_or("`step` names the step")?;
    let count = recipe.calls.len();
    let call = usize::try_from(step)
        .ok()
        .and_then(|index| recipe.calls.get_mut(index))
        .ok_or_else(|| format!("the recipe has {count} steps; there is no step {step}"))?;
    let param = params
        .as_str("param")
        .ok_or("`param` names the parameter to promote")?
        .to_string();
    let Some(text) = call.params.get(&param).cloned() else {
        let held: Vec<&str> = call.params.keys().map(String::as_str).collect();
        return Err(format!(
            "step {step} ({}) has no `{param}`; it has {}",
            call.id,
            held.join(", ")
        ));
    };
    if text.contains('$') {
        return Err(format!(
            "step {step} `{param}` is `{text}`, which already reads inputs or results"
        ));
    }
    let name = params.as_str("input").unwrap_or(&param).trim().to_string();
    if recipe.inputs.iter().any(|input| input.name == name) {
        return Err(format!("the recipe already has an input called `{name}`"));
    }
    let kind = match params.as_str("kind") {
        Some(kind) => kind.trim().to_string(),
        None => kind_for(world, &call.id, &param)?.name().to_string(),
    };
    let default = params
        .as_str("default")
        .map_or(text.as_str(), str::trim)
        .to_string();
    let range = match (params.as_float("min"), params.as_float("max")) {
        (Some(min), Some(max)) => vec![min, max],
        (None, None) => Vec::new(),
        _ => return Err("a range takes both `min` and `max`".to_string()),
    };
    InputSpec::new(&name, &kind, &default, &range)?;

    call.params.insert(param, format!("${name}"));
    recipe.inputs.push(RecipeInput {
        name: name.clone(),
        kind,
        default,
        range,
    });
    let stem = jackdaw_bsn::path_stem(&path);
    let original =
        std::fs::read(&path).map_err(|err| format!("{reference} could not be read: {err}"))?;
    std::fs::write(&path, recipe_file_text(&stem, &recipe))
        .map_err(|err| format!("{reference} could not be written: {err}"))?;

    let mut rerun = OperatorParameters::default();
    rerun.insert("entity".to_string(), PropertyValue::Entity(root));
    if let Err(why) = rerun_recipe(world, &rerun) {
        let _ = std::fs::write(&path, original);
        return Err(why);
    }
    report_to_caller(world, format!("{reference}: `{name}` is an input"));
    Ok(())
}

/// The input kind a parameter's declared type takes.
fn kind_for(world: &mut World, op: &str, param: &str) -> Result<InputKind, String> {
    let spec: Option<&'static ParamSpec> = {
        let mut ops = world.query::<&jackdaw_api_internal::lifecycle::OperatorEntity>();
        ops.iter(world)
            .find(|entity| entity.id() == op)
            .and_then(|entity| entity.parameters().iter().find(|spec| spec.name == param))
    };
    match spec.map(|spec| spec.ty) {
        Some("Float") => Ok(InputKind::Float),
        Some("Int") => Ok(InputKind::Int),
        Some("Bool") => Ok(InputKind::Bool),
        Some("Vec2") => Ok(InputKind::Vec2),
        Some("Vec3") => Ok(InputKind::Vec3),
        Some("String") => Ok(InputKind::Text),
        Some(other) => Err(format!(
            "`{param}` of {op} takes {other}; give `kind` to promote it"
        )),
        None => Err(format!(
            "{op} declares no `{param}`; give `kind` to promote it"
        )),
    }
}
