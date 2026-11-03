//! Recipes: a saved list of operator calls with named inputs, run as one undo
//! step into a root node that remembers how it was built, so it can be built
//! again with new inputs.
//!
//! A `.recipe.bsn` file holds one [`Recipe`]. Every call parameter is text.
//! Text that reads as a whole expression (`$width * 0.5`, `4 * 2`, `[1, 2]`),
//! or starts with `=`, is evaluated; `$N.path` takes a value from call `N`'s
//! outcome as in a batch; anything else is passed through as it is written.
//! A parameter that comes out as empty text is left out, so the operator's
//! own default applies. While a call runs, the entity references it makes
//! name only nodes under the recipe's root, and a path starts from the
//! root's children.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bevy::asset::embedded_asset;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_recipe::{InputSpec, SEED_INPUT, Scope, Value as RecipeValue, value_text};
use jackdaw_scene_types::{RECIPE_INSTANCE_TYPE_PATH, RecipeInstance};
use serde_json::{Value, json};

use crate::remote::recipe_run::{self, RecipePlan, RecipeRoot};

pub mod picker;
mod promote;
pub mod record;

pub(crate) use picker::RecipePickerOp;
pub use picker::{RecipeList, recipe_choices};
pub(crate) use promote::RecipePromoteOp;
pub use record::RecipeRecording;
use record::{RecipeRecordStartOp, RecipeRecordStopOp};

/// The recipes the editor ships, by name.
pub const STARTER_RECIPES: [&str; 5] = ["tree", "rock", "fence", "house", "stairs"];

/// Where `embedded_asset!` registers the starter recipes.
const STARTER_DIR: &str = "jackdaw/recipes/starter";

/// The file extension a recipe is saved under.
pub const RECIPE_EXTENSION: &str = ".recipe.bsn";

/// A recipe as its file holds it.
#[derive(Reflect, Default, Clone, Debug, PartialEq)]
#[reflect(Default)]
pub struct Recipe {
    pub description: String,
    pub inputs: Vec<RecipeInput>,
    pub calls: Vec<RecipeCall>,
}

/// One named input: `kind` is float, int, bool, vec2, vec3 or text, the
/// default is written as text, and `range` is empty or `[min, max]`.
#[derive(Reflect, Default, Clone, Debug, PartialEq)]
#[reflect(Default)]
pub struct RecipeInput {
    pub name: String,
    pub kind: String,
    pub default: String,
    pub range: Vec<f64>,
}

/// One operator call, every parameter written as text.
#[derive(Reflect, Default, Clone, Debug, PartialEq)]
#[reflect(Default)]
pub struct RecipeCall {
    pub id: String,
    pub params: BTreeMap<String, String>,
}

pub(crate) fn plugin(app: &mut App) {
    embedded_asset!(app, "starter/tree.recipe.bsn");
    embedded_asset!(app, "starter/rock.recipe.bsn");
    embedded_asset!(app, "starter/fence.recipe.bsn");
    embedded_asset!(app, "starter/house.recipe.bsn");
    embedded_asset!(app, "starter/stairs.recipe.bsn");
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<RecipeRunOp>()
        .register_operator::<RecipeRerunOp>()
        .register_operator::<RecipeRecordStartOp>()
        .register_operator::<RecipeRecordStopOp>()
        .register_operator::<RecipePromoteOp>()
        .register_operator::<RecipePickerOp>();
}

/// A recipe read and checked: its inputs and its calls.
struct Loaded {
    /// How the root remembers the recipe.
    reference: String,
    inputs: Vec<InputSpec>,
    calls: Vec<RecipeCall>,
}

impl Loaded {
    fn read(world: &mut World, reference: &str) -> Result<Self, String> {
        let (reference, text, origin) = recipe_text(world, reference)?;
        let recipe: Recipe = jackdaw_bsn::read_asset_text(&text, &origin)
            .map_err(|err| format!("recipe `{reference}`: {err}"))?;
        let mut inputs = Vec::with_capacity(recipe.inputs.len());
        for input in &recipe.inputs {
            let spec = InputSpec::new(&input.name, &input.kind, &input.default, &input.range)
                .map_err(|why| format!("recipe `{reference}`: {why}"))?;
            if inputs
                .iter()
                .any(|known: &InputSpec| known.name == spec.name)
            {
                return Err(format!(
                    "recipe `{reference}` declares `{}` twice",
                    spec.name
                ));
            }
            inputs.push(spec);
        }
        if let Some((index, call)) = recipe
            .calls
            .iter()
            .enumerate()
            .find(|(_, call)| call.id.starts_with("recipe."))
        {
            return Err(format!(
                "recipe `{reference}` call {index} runs {}; a recipe cannot run recipes",
                call.id
            ));
        }
        Ok(Self {
            reference,
            inputs,
            calls: recipe.calls,
        })
    }

    /// The display name a root takes when the caller gives none.
    fn root_name(&self) -> String {
        display_name(&self.reference)
    }

    /// Every input's value: `base` where it holds one, else the default, with
    /// `given` laid over them and each checked.
    fn resolve(
        &self,
        base: &BTreeMap<String, String>,
        given: &serde_json::Map<String, Value>,
    ) -> Result<BTreeMap<String, RecipeValue>, String> {
        if let Some(unknown) = given
            .keys()
            .find(|name| !self.inputs.iter().any(|spec| &spec.name == *name))
        {
            let known: Vec<&str> = self.inputs.iter().map(|spec| spec.name.as_str()).collect();
            return Err(format!(
                "`{unknown}` is not an input of recipe `{}`; it takes {}",
                self.reference,
                if known.is_empty() {
                    "none".to_string()
                } else {
                    known.join(", ")
                }
            ));
        }
        let mut values = BTreeMap::new();
        for spec in &self.inputs {
            let value = match (given.get(&spec.name), base.get(&spec.name)) {
                (Some(value), _) => given_value(spec, value)?,
                (None, Some(text)) => spec.read_text(text)?,
                (None, None) => spec.default.clone(),
            };
            values.insert(spec.name.clone(), value);
        }
        Ok(values)
    }

    /// The calls with every expression worked out; `$N` references are left
    /// for the batch to fill.
    fn evaluate(
        &self,
        inputs: &BTreeMap<String, RecipeValue>,
        seed: u64,
    ) -> Result<Vec<Value>, String> {
        let mut scope_inputs = inputs.clone();
        scope_inputs.insert(SEED_INPUT.to_string(), RecipeValue::Number(seed as f64));
        let mut calls = Vec::with_capacity(self.calls.len());
        for (index, call) in self.calls.iter().enumerate() {
            let mut params = serde_json::Map::new();
            for (name, text) in &call.params {
                let site = format!("{index}.{name}");
                let value = if jackdaw_recipe::is_expression(text) {
                    let scope = Scope {
                        inputs: &scope_inputs,
                        seed,
                        site: &site,
                    };
                    let value = jackdaw_recipe::eval(text, &scope).map_err(|why| {
                        format!("call {index} ({}) `{name}` = `{text}`: {why}", call.id)
                    })?;
                    to_json(&value)
                } else if let Some(list) = reference_list(text) {
                    list
                } else {
                    json!(text)
                };
                if value.as_str() == Some("") {
                    continue;
                }
                params.insert(name.clone(), value);
            }
            calls.push(json!({ "id": call.id, "params": params }));
        }
        Ok(calls)
    }
}

/// The name a recipe goes by: its stem with a capital first letter.
pub fn display_name(reference: &str) -> String {
    let reference = reference.trim_start_matches(STARTER_PREFIX);
    let stem = jackdaw_bsn::asset_stem(reference);
    let mut chars = stem.chars();
    match chars.next() {
        Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
        None => "Recipe".to_string(),
    }
}

/// A recipe as its file holds it and its inputs read, for a surface showing
/// it.
pub struct RecipeView {
    /// How a root names it: `starter:<name>` or a path under the assets
    /// directory.
    pub reference: String,
    pub inputs: Vec<InputSpec>,
    pub calls: Vec<RecipeCall>,
}

impl RecipeView {
    pub fn read(world: &mut World, reference: &str) -> Result<Self, String> {
        let loaded = Loaded::read(world, reference)?;
        Ok(Self {
            reference: loaded.reference,
            inputs: loaded.inputs,
            calls: loaded.calls,
        })
    }

    /// Whether the editor ships it, so it cannot be changed in place.
    pub fn is_starter(&self) -> bool {
        self.reference.starts_with(STARTER_PREFIX)
    }
}

/// The text a `.recipe.bsn` file holds for `recipe`, under the name `stem`.
pub fn recipe_file_text(stem: &str, recipe: &Recipe) -> String {
    let quote =
        |text: &str| jackdaw_bsn::emit_value_text(&jackdaw_bsn::BsnValue::String(text.into()));
    let mut body = format!(
        "#{stem}\n{RECIPE_TYPE_PATH} {{\n    description: {},\n    inputs: [\n",
        quote(&recipe.description)
    );
    for input in &recipe.inputs {
        let range: Vec<String> = input.range.iter().map(|n| format!("{n:?}")).collect();
        body.push_str(&format!(
            "        jackdaw::recipes::RecipeInput {{ name: {}, kind: {}, default: {}, range: [{}] }},\n",
            quote(&input.name),
            quote(&input.kind),
            quote(&input.default),
            range.join(", ")
        ));
    }
    body.push_str("    ],\n    calls: [\n");
    for call in &recipe.calls {
        body.push_str(&format!(
            "        jackdaw::recipes::RecipeCall {{ id: {}, params: map[\n",
            quote(&call.id)
        ));
        for (name, value) in &call.params {
            body.push_str(&format!(
                "            ({}, {}),\n",
                quote(name),
                quote(value)
            ));
        }
        body.push_str("        ] },\n");
    }
    body.push_str("    ],\n}\n");
    crate::asset_files::asset_file_text(RECIPE_TYPE_PATH, &body)
}

/// The type a `.recipe.bsn` file holds.
pub const RECIPE_TYPE_PATH: &str = "jackdaw::recipes::Recipe";

/// Read the recipe file at `path`.
fn read_recipe_file(path: &Path) -> Result<Recipe, String> {
    let text = jackdaw_bsn::read_document_text(path)
        .map_err(|err| format!("{} could not be read: {err}", path.display()))?;
    jackdaw_bsn::read_asset_text(&text, path).map_err(|err| format!("{}: {err}", path.display()))
}

/// Literal text that is a JSON list of text holding a `$N` result
/// reference, as the list, so the batch fills the reference in.
fn reference_list(text: &str) -> Option<Value> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(text.trim()) else {
        return None;
    };
    let is_reference = |item: &Value| {
        item.as_str()
            .and_then(|text| text.strip_prefix('$'))
            .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))
    };
    (items.iter().all(Value::is_string) && items.iter().any(is_reference))
        .then_some(Value::Array(items))
}

/// A value given for `spec` as JSON, checked.
fn given_value(spec: &InputSpec, value: &Value) -> Result<RecipeValue, String> {
    let read = match value {
        Value::String(text) => return spec.read_text(text),
        other => from_json(other),
    };
    match read {
        Some(value) => spec.check(value),
        None => Err(format!(
            "`{}` is {}, not {value}",
            spec.name,
            spec.describe()
        )),
    }
}

fn from_json(value: &Value) -> Option<RecipeValue> {
    match value {
        Value::Number(number) => number.as_f64().map(RecipeValue::Number),
        Value::Bool(flag) => Some(RecipeValue::Bool(*flag)),
        Value::String(text) => Some(RecipeValue::Text(text.clone())),
        Value::Array(items) => items
            .iter()
            .map(from_json)
            .collect::<Option<Vec<_>>>()
            .map(RecipeValue::List),
        _ => None,
    }
}

/// An evaluated value as a call parameter. A whole number is sent as an
/// integer, so an integer parameter takes it.
fn to_json(value: &RecipeValue) -> Value {
    match value {
        RecipeValue::Number(number) if number.fract() == 0.0 && number.abs() < 9.0e15 => {
            json!(*number as i64)
        }
        RecipeValue::Number(number) => json!(number),
        RecipeValue::Bool(flag) => json!(flag),
        RecipeValue::Text(text) | RecipeValue::Ref(text) => json!(text),
        RecipeValue::List(items) => Value::Array(items.iter().map(to_json).collect()),
    }
}

/// The prefix a stored reference to a starter carries, so a project recipe
/// of the same name never takes its place.
const STARTER_PREFIX: &str = "starter:";

/// Find a recipe: its stored reference, its text and the path errors name.
///
/// `starter:<name>` is a starter. A path names a file under the assets
/// directory, and may not leave it. A bare name is the one project recipe
/// with that stem, else the starter of that name. The stored reference says
/// which was found, so a root always rebuilds from the same recipe.
fn recipe_text(world: &mut World, reference: &str) -> Result<(String, String, PathBuf), String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Err("`recipe` names no recipe".to_string());
    }
    if let Some(name) = reference.strip_prefix(STARTER_PREFIX) {
        return starter(world, name);
    }
    if reference.contains(['/', '\\']) || reference.ends_with(".bsn") {
        let assets = assets_dir(world, reference)?;
        let path = inside_assets(&assets, reference)?;
        return read_project_recipe(&assets, &path, reference);
    }
    let found: Vec<PathBuf> = world
        .get_resource::<crate::asset_index::AssetIndex>()
        .map(|index| index.stem_paths(reference))
        .unwrap_or_default()
        .into_iter()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(RECIPE_EXTENSION))
        })
        .collect();
    match found.as_slice() {
        [] => starter(world, reference),
        [path] => {
            let assets = assets_dir(world, reference)?;
            read_project_recipe(&assets, &assets.join(path), reference)
        }
        [first, second, ..] => Err(format!(
            "`{reference}` names {} recipes, {} and {}; give its path",
            found.len(),
            first.display(),
            second.display()
        )),
    }
}

fn assets_dir(world: &World, reference: &str) -> Result<PathBuf, String> {
    world
        .get_resource::<crate::project::ProjectRoot>()
        .map(crate::project::ProjectRoot::assets_dir)
        .ok_or_else(|| format!("`{reference}` is a project recipe, and no project is open"))
}

/// The file `reference` names under `assets`, refused when it is absolute or
/// leads outside the assets directory.
fn inside_assets(assets: &Path, reference: &str) -> Result<PathBuf, String> {
    let outside = || format!("recipe `{reference}` is outside the assets directory");
    if Path::new(reference).is_absolute() {
        return Err(outside());
    }
    let unreadable = |err: std::io::Error| format!("recipe `{reference}` could not be read: {err}");
    let path = assets.join(reference).canonicalize().map_err(unreadable)?;
    let assets = assets.canonicalize().map_err(unreadable)?;
    if !path.starts_with(&assets) {
        return Err(outside());
    }
    Ok(path)
}

fn read_project_recipe(
    assets: &Path,
    path: &Path,
    reference: &str,
) -> Result<(String, String, PathBuf), String> {
    let text = jackdaw_bsn::read_document_text(path)
        .map_err(|err| format!("recipe `{reference}` could not be read: {err}"))?;
    let base = assets
        .canonicalize()
        .unwrap_or_else(|_| assets.to_path_buf());
    let relative = path
        .strip_prefix(&base)
        .or_else(|_| path.strip_prefix(assets))
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/");
    Ok((relative, text, path.to_path_buf()))
}

fn starter(world: &World, name: &str) -> Result<(String, String, PathBuf), String> {
    if !STARTER_RECIPES.contains(&name) {
        return Err(format!(
            "no recipe is called `{name}`; the editor ships {}",
            STARTER_RECIPES.join(", ")
        ));
    }
    let path = PathBuf::from(format!("{STARTER_DIR}/{name}{RECIPE_EXTENSION}"));
    let text = embedded_text(world, &path)?;
    Ok((format!("{STARTER_PREFIX}{name}"), text, path))
}

/// Read a file `embedded_asset!` registered.
fn embedded_text(world: &World, path: &Path) -> Result<String, String> {
    let server = world
        .get_resource::<AssetServer>()
        .ok_or("there is no asset server to read the starter recipes from")?;
    let source = server
        .get_source("embedded")
        .map_err(|err| err.to_string())?;
    let reader = source.reader();
    let bytes = bevy::tasks::block_on(async {
        let mut file = reader.read(path).await.map_err(|err| err.to_string())?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .await
            .map_err(|err| err.to_string())?;
        Ok::<_, String>(bytes)
    })
    .map_err(|err| format!("starter recipe {} could not be read: {err}", path.display()))?;
    String::from_utf8(bytes).map_err(|err| err.to_string())
}

/// The `inputs` parameter as a JSON object; absent reads as none.
fn given_inputs(params: &OperatorParameters) -> Result<serde_json::Map<String, Value>, String> {
    let Some(text) = params.as_str("inputs") else {
        return match params.get("inputs") {
            None => Ok(serde_json::Map::new()),
            Some(other) => Err(format!("`inputs` is a JSON object, not {other}")),
        };
    };
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(fields)) => Ok(fields),
        _ => Err(format!(
            "`inputs` is a JSON object such as {{\"width\": 6}}, not `{text}`"
        )),
    }
}

fn seed_param(params: &OperatorParameters) -> Result<Option<u64>, String> {
    crate::primitive_ops::int_param(params, "seed", 0..=i64::from(u32::MAX))
        .map(|seed| seed.map(|seed| seed as u64))
}

fn refuse_unknown(params: &OperatorParameters, keys: &[&str]) -> Result<(), String> {
    match params.keys().find(|key| !keys.contains(&key.as_str())) {
        Some(key) => Err(format!("this takes no `{key}`")),
        None => Ok(()),
    }
}

/// Put `instance` on `root` in the ECS and the document together.
fn write_instance(world: &mut World, root: Entity, instance: &RecipeInstance) {
    if let Ok(mut entity) = world.get_entity_mut(root) {
        entity.insert(instance.clone());
    }
    crate::commands::sync_component_to_ast(world, root, RECIPE_INSTANCE_TYPE_PATH, instance);
}

fn instance_of(
    reference: &str,
    inputs: &BTreeMap<String, RecipeValue>,
    seed: u64,
) -> RecipeInstance {
    RecipeInstance {
        recipe: reference.to_string(),
        inputs: inputs
            .iter()
            .map(|(name, value)| (name.clone(), value_text(value)))
            .collect(),
        seed,
    }
}

fn refuse(world: &mut World, op: &str, why: String) -> OperatorResult {
    warn_caller(world, format!("{op}: {why}"));
    set_outcome(world, Outcome::Invalid);
    OperatorResult::Cancelled
}

const RUN_KEYS: [&str; 7] = ["recipe", "inputs", "pos", "rot", "parent", "name", "seed"];

/// Build a recipe into a new root, as one undo step.
#[operator(
    id = "recipe.run",
    label = "Run Recipe",
    description = "Build a saved recipe of operator calls into a new root node, from named \
                   inputs, as one undo step. The root remembers the recipe and its inputs.",
    allows_undo = false,
    params(
        recipe(
            String,
            doc = "Recipe to run: a .recipe.bsn path under the assets directory, or a name; a \
                   project recipe of that name first, else one the editor ships (tree, rock, \
                   fence, house, stairs)."
        ),
        inputs(
            String,
            doc = "Input values as a JSON object, e.g. {\"width\": 6}. Left out, each takes its \
                   default."
        ),
        pos(Vec3, doc = "Root position [x, y, z]. Defaults to the origin."),
        rot(
            Vec3,
            doc = "Root rotation [yaw, pitch, roll] in degrees. Defaults to none."
        ),
        parent(Entity, doc = "Node that adopts the root. Defaults to top level."),
        name(String, doc = "Name for the root. Defaults to the recipe's name."),
        seed(
            i64,
            doc = "Seed for the recipe's rand(), 0 to 4294967295, read as $seed. Defaults to 0."
        ),
    )
)]
pub(crate) fn recipe_run(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    match run_recipe(world, &params) {
        Ok(()) => OperatorResult::Finished,
        Err(why) => refuse(world, RecipeRunOp::ID, why),
    }
}

fn run_recipe(world: &mut World, params: &OperatorParameters) -> Result<(), String> {
    refuse_unknown(params, &RUN_KEYS)?;
    let reference = params
        .as_str("recipe")
        .ok_or("`recipe` names the recipe to run")?
        .to_string();
    let recipe = Loaded::read(world, &reference)?;
    let inputs = recipe.resolve(&BTreeMap::new(), &given_inputs(params)?)?;
    let seed = seed_param(params)?.unwrap_or(0);
    let calls = recipe.evaluate(&inputs, seed)?;
    let count = calls.len();
    let plan = RecipePlan::new(world, &calls)?;

    let mut root = serde_json::Map::new();
    let name = params
        .as_str("name")
        .map(str::to_string)
        .unwrap_or_else(|| recipe.root_name());
    root.insert("name".into(), json!(name));
    for key in ["pos", "rot"] {
        if let Some(value) = crate::primitive_ops::vec3_param(params, key)? {
            root.insert(key.into(), json!([value.x, value.y, value.z]));
        }
    }
    if params.get("parent").is_some() {
        let parent = params
            .as_entity("parent")
            .ok_or("`parent` names no entity")?;
        root.insert("parent".into(), json!(parent.to_bits()));
    }

    let instance = instance_of(&recipe.reference, &inputs, seed);
    recipe_run::build(
        world,
        format!("Run Recipe {name}"),
        RecipeRoot::New(Value::Object(root)),
        plan,
        |world, root| write_instance(world, root, &instance),
    )?;
    report_to_caller(world, format!("{}: {count} calls", recipe.reference));
    set_outcome(world, Outcome::Changed);
    Ok(())
}

const RERUN_KEYS: [&str; 3] = ["entity", "inputs", "seed"];

/// Build a recipe's root again with new inputs, as one undo step.
#[operator(
    id = "recipe.rerun",
    label = "Rerun Recipe",
    description = "Rebuild what a recipe built under its root with new inputs, as one undo \
                   step. The root keeps its id, name, transform and tags; what it holds is \
                   replaced.",
    allows_undo = false,
    params(
        entity(Entity, doc = "Root a recipe built."),
        inputs(
            String,
            doc = "Inputs to change, as a JSON object, e.g. {\"floors\": 3}. The rest keep the \
                   values the root was built with."
        ),
        seed(
            i64,
            doc = "New seed, 0 to 4294967295. Defaults to the one the root was built with."
        ),
    )
)]
pub(crate) fn recipe_rerun(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    match rerun_recipe(world, &params) {
        Ok(()) => OperatorResult::Finished,
        Err(why) => refuse(world, RecipeRerunOp::ID, why),
    }
}

fn rerun_recipe(world: &mut World, params: &OperatorParameters) -> Result<(), String> {
    refuse_unknown(params, &RERUN_KEYS)?;
    let root = params
        .as_entity("entity")
        .ok_or("`entity` names no entity")?;
    crate::primitive_ops::scene_node(world, root, "entity")?;
    let stored = world
        .get::<RecipeInstance>(root)
        .cloned()
        .ok_or("that entity is not the root of a recipe")?;
    let recipe = Loaded::read(world, &stored.recipe)?;
    let dropped: Vec<&String> = stored
        .inputs
        .keys()
        .filter(|name| !recipe.inputs.iter().any(|spec| &spec.name == *name))
        .collect();
    if !dropped.is_empty() {
        let names: Vec<&str> = dropped.iter().map(|name| name.as_str()).collect();
        warn_caller(
            world,
            format!(
                "recipe.rerun: recipe `{}` no longer takes {}; dropped",
                recipe.reference,
                names.join(", ")
            ),
        );
    }
    let mut base = stored.inputs.clone();
    base.retain(|name, _| recipe.inputs.iter().any(|spec| &spec.name == name));
    let inputs = recipe.resolve(&base, &given_inputs(params)?)?;
    let seed = seed_param(params)?.unwrap_or(stored.seed);
    let calls = recipe.evaluate(&inputs, seed)?;
    let count = calls.len();
    let plan = RecipePlan::new(world, &calls)?;

    let instance = instance_of(&recipe.reference, &inputs, seed);
    let name = world
        .get::<Name>(root)
        .map_or_else(|| recipe.root_name(), |name| name.as_str().to_string());
    let built = recipe_run::build(
        world,
        format!("Rerun Recipe {name}"),
        RecipeRoot::Existing(root),
        plan,
        |world, root| write_instance(world, root, &instance),
    )?;
    report_to_caller(
        world,
        format!(
            "{}: {count} calls, {} replaced",
            recipe.reference, built.replaced
        ),
    );
    set_outcome(world, Outcome::Changed);
    Ok(())
}
