//! Recording a recipe: the operator calls a person makes, kept as they are
//! made and saved as a `.recipe.bsn` file.
//!
//! Only a call made at the top level, not from inside another operator or a
//! gesture, that changes the scene document is kept; a call that changes only
//! the view, the selection or the editor's own UI is not. Each kept call is
//! written the way it was made, every parameter as text. An entity it names is
//! written as the node's name when that name is unique among the nodes the
//! recording made, else as its path from the topmost of those nodes, else as
//! `$N.entities.K` for the call that made it; never as an id. A call that
//! changes or names a node the recording did not make is left out and
//! reported, since a recipe only builds under its own root. So is a call that
//! acts on the selection without a parameter to name it by, a gesture, and a
//! recipe call.
//!
//! On stop, the top-level nodes the kept calls made are gathered under a new
//! group at the origin carrying the recipe's `RecipeInstance`: the root running
//! the recipe builds. Nodes a left-out call or a gesture made stay where they
//! are. Undo during a recording takes back a kept call when the entry it undoes
//! is that call's, and redo puts it back.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::OperatorEntity;
use jackdaw_api_internal::operator::{ParamSpec, TopLevelCallHook, with_batch_transaction};
use jackdaw_commands::{CommandHistory, EditorCommand};
use jackdaw_scene_types::{PropertyValue, RecipeInstance, SceneNodeId};
use serde_json::json;

use super::run::call_inside;
use super::{RECIPE_EXTENSION, Recipe, RecipeCall, display_name, recipe_file_text, refuse};
use crate::commands::SpawnedEntities;
use crate::document_units::{UnitKey, Units};

/// A recording under way.
#[derive(Resource, Default)]
pub struct RecipeRecording {
    /// Every node the scene held when it began.
    existing: HashSet<u64>,
    steps: Vec<Step>,
    /// Steps undo took back, the latest last, for redo to put back.
    undone: Vec<Step>,
    left_out: Vec<String>,
    next_step: u64,
    /// A gesture that started, and the document before it.
    gesture: Option<(String, Option<Arc<Units>>)>,
}

impl RecipeRecording {
    /// How many calls it holds.
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Why each call left out was left out.
    pub fn left_out(&self) -> &[String] {
        &self.left_out
    }
}

#[derive(Clone)]
struct Step {
    uid: u64,
    id: String,
    params: Vec<(String, Param)>,
    /// The nodes the call made, in the order its outcome lists them.
    spawned: Vec<u64>,
    /// Every node the document gained from the call.
    made: Vec<u64>,
    /// The undo entry the call pushed, by address, while it is in the history.
    entry: Option<usize>,
}

#[derive(Clone)]
enum Param {
    Text(String),
    One(Ref),
    List(Vec<Ref>),
}

/// A node a call names, by every spelling it can be written in.
#[derive(Clone)]
struct Ref {
    name: Option<String>,
    spawned: Option<(u64, usize)>,
    shown: String,
}

/// A top-level call seen starting, ended by the call's after-hook whatever
/// becomes of the call.
#[derive(Resource)]
struct PendingCall {
    kind: PendingKind,
    /// Whether this hook began collecting what the call spawns, and so has to
    /// stop.
    own_watch: bool,
}

enum PendingKind {
    Undo,
    Redo,
    Call {
        params: Result<Vec<(String, Param)>, String>,
        before: Option<Arc<Units>>,
        names_entities: bool,
        top_before: Option<usize>,
    },
}

/// Start recording the calls that change the scene.
#[operator(
    id = "recipe.record.start",
    label = "Record Recipe",
    description = "Start recording the operator calls that change the scene, to save them as \
                   a recipe with recipe.record.stop.",
    allows_undo = false
)]
pub(crate) fn recipe_record_start(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    if world.contains_resource::<RecipeRecording>() {
        return refuse(
            world,
            RecipeRecordStartOp::ID,
            "a recording is already under way; stop it first".to_string(),
        );
    }
    let existing = scene_nodes(world).into_iter().map(|(_, id)| id).collect();
    world.insert_resource(RecipeRecording {
        existing,
        ..default()
    });
    world.insert_resource(TopLevelCallHook {
        before: before_call,
        after: after_call,
    });
    report_to_caller(world, "recording");
    set_outcome(world, Outcome::Unchanged);
    OperatorResult::Finished
}

/// Stop recording and save what was recorded.
#[operator(
    id = "recipe.record.stop",
    label = "Stop Recording Recipe",
    description = "Stop recording and save the calls as a recipe file under the assets \
                   directory. The nodes the recording made are gathered under a new root that \
                   remembers the recipe.",
    allows_undo = false,
    writes_disk = true,
    params(
        name(
            String,
            doc = "Recipe name, saved as recipes/<name>.recipe.bsn, or a .recipe.bsn path under \
                   the assets directory."
        ),
        discard(
            bool,
            doc = "Throw the recording away instead of saving it. Defaults to false."
        ),
    )
)]
pub(crate) fn recipe_record_stop(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    match stop(world, &params) {
        Ok(()) => OperatorResult::Finished,
        Err(why) => refuse(world, RecipeRecordStopOp::ID, why),
    }
}

/// Saves the recording; a stop that fails keeps it, so it can be tried again.
fn stop(world: &mut World, params: &OperatorParameters) -> Result<(), String> {
    if !world.contains_resource::<RecipeRecording>() {
        return Err("nothing is being recorded".to_string());
    }
    if params.as_bool("discard") == Some(true) {
        end_recording(world);
        report_to_caller(world, "recording discarded");
        set_outcome(world, Outcome::Unchanged);
        return Ok(());
    }
    let name = params
        .as_str("name")
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or("`name` names the recipe to save")?
        .to_string();
    let assets = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(crate::project::ProjectRoot::assets_dir)
        .ok_or("no project is open to save the recipe in")?;
    let relative = recipe_path(&name)?;
    let stem = jackdaw_bsn::path_stem(&relative);
    let file = assets.join(&relative);
    if file.exists() {
        return Err(format!(
            "{} already exists; pick another name",
            slash(&relative)
        ));
    }
    if let Some(clash) = world
        .get_resource::<crate::asset_index::AssetIndex>()
        .map(|index| index.stem_paths(&stem))
        .unwrap_or_default()
        .into_iter()
        .find(|path| slash(path).ends_with(RECIPE_EXTENSION))
    {
        return Err(format!(
            "a recipe called `{stem}` already exists at {}; pick another name",
            slash(&clash)
        ));
    }

    settle_gesture(world);
    let (steps, existing, mut left_out) = {
        let recording = world
            .get_resource::<RecipeRecording>()
            .ok_or("nothing is being recorded")?;
        (
            recording.steps.clone(),
            recording.existing.clone(),
            recording.left_out.clone(),
        )
    };
    let finished = finish_steps(&steps);
    left_out.extend(finished.left_out);
    let recipe = Recipe {
        description: "Recorded in the editor.".to_string(),
        inputs: Vec::new(),
        calls: finished.calls,
    };
    let count = recipe.calls.len();
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| format!("{} could not be made: {err}", dir.display()))?;
    }
    std::fs::write(&file, recipe_file_text(&stem, &recipe))
        .map_err(|err| format!("{} could not be written: {err}", slash(&relative)))?;
    crate::asset_index::rescan_asset_index(world);

    let reference = slash(&relative);
    let mut tops = Vec::new();
    for (entity, node) in top_level_nodes(world, &existing) {
        if finished.made.contains(&node) {
            tops.push(entity);
        } else {
            let shown = world
                .get::<Name>(entity)
                .map_or_else(|| entity.to_string(), ToString::to_string);
            left_out.push(format!(
                "`{shown}`: no call kept made it, so it stays where it is"
            ));
        }
    }
    if !tops.is_empty()
        && let Err(why) = gather(world, &reference, &tops)
    {
        let _ = std::fs::remove_file(&file);
        crate::asset_index::rescan_asset_index(world);
        return Err(why);
    }
    end_recording(world);
    for why in left_out {
        warn_caller(world, format!("{}: left out {why}", RecipeRecordStopOp::ID));
    }
    report_to_caller(world, format!("{reference}: {count} calls"));
    set_outcome(world, Outcome::Changed);
    Ok(())
}

/// The file a recipe called `name` is saved as, relative to the assets
/// directory.
fn recipe_path(name: &str) -> Result<PathBuf, String> {
    let path = if name.contains(['/', '\\']) || name.ends_with(".bsn") {
        let trimmed = name.strip_suffix(RECIPE_EXTENSION).unwrap_or(name);
        PathBuf::from(format!("{trimmed}{RECIPE_EXTENSION}"))
    } else {
        PathBuf::from(format!("recipes/{name}{RECIPE_EXTENSION}"))
    };
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return Err(format!("`{name}` leads outside the assets directory"));
    }
    let stem = jackdaw_bsn::path_stem(&path);
    if stem.is_empty()
        || !stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err(format!(
            "`{stem}` is not a recipe name; use ASCII letters, digits, `_` and `-`"
        ));
    }
    Ok(path)
}

fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Take the recording away and stop watching calls.
fn end_recording(world: &mut World) {
    world.remove_resource::<TopLevelCallHook>();
    world.remove_resource::<RecipeRecording>();
}

/// Gather `tops` under a new root at the origin that remembers `reference`,
/// as one undo step.
fn gather(world: &mut World, reference: &str, tops: &[Entity]) -> Result<(), String> {
    let name = display_name(reference);
    let held = SpawnedEntities::set_aside(world);
    let built = with_batch_transaction(world, format!("Record Recipe {name}"), true, |world| {
        let built = (|| {
            let root = call_inside(
                world,
                json!({ "id": "entity.add.group", "params": { "name": name } }),
            )?
            .first()
            .copied()
            .ok_or("the root could not be added")?;
            for &child in tops {
                call_inside(
                    world,
                    json!({ "id": "entity.reparent", "params": {
                        "child": child.to_bits(), "parent": root.to_bits()
                    } }),
                )?;
            }
            super::write_instance(
                world,
                root,
                &RecipeInstance {
                    recipe: reference.to_string(),
                    ..default()
                },
            );
            Ok::<_, String>(root)
        })();
        let keep = built.is_ok();
        (built, keep)
    });
    SpawnedEntities::put_back(world, held);
    let root = built?;
    SpawnedEntities::record(world, root);
    crate::selection::select_only(world, root);
    Ok(())
}

/// The steps that could be written out.
struct Finished {
    calls: Vec<RecipeCall>,
    /// The nodes the kept calls made.
    made: HashSet<u64>,
    /// Why each step that could not be was left out.
    left_out: Vec<String>,
}

/// The steps as recipe calls, with every reference spelled out.
fn finish_steps(steps: &[Step]) -> Finished {
    let mut kept: Vec<u64> = Vec::new();
    let mut finished = Finished {
        calls: Vec::new(),
        made: HashSet::new(),
        left_out: Vec::new(),
    };
    for step in steps {
        let index_of = |uid: u64| kept.iter().position(|&kept| kept == uid);
        let mut params = std::collections::BTreeMap::new();
        let mut failed = None;
        for (name, param) in &step.params {
            match spell(param, &index_of) {
                Ok(text) => {
                    params.insert(name.clone(), text);
                }
                Err(why) => {
                    failed = Some(format!("{} `{name}`: {why}", step.id));
                    break;
                }
            }
        }
        match failed {
            Some(why) => finished.left_out.push(why),
            None => {
                kept.push(step.uid);
                finished.made.extend(step.made.iter().copied());
                finished.calls.push(RecipeCall {
                    id: step.id.clone(),
                    params,
                });
            }
        }
    }
    finished
}

/// A parameter as recipe text. A list whose every item has a name is written
/// as names; one mixing names and calls' results as a JSON list of both.
fn spell(param: &Param, index_of: &impl Fn(u64) -> Option<usize>) -> Result<String, String> {
    let by_call = |reference: &Ref| {
        reference
            .spawned
            .and_then(|(uid, at)| index_of(uid).map(|call| format!("${call}.entities.{at}")))
    };
    match param {
        Param::Text(text) => Ok(text.clone()),
        Param::One(reference) => reference
            .name
            .clone()
            .or_else(|| by_call(reference))
            .ok_or_else(|| unnamed(reference)),
        Param::List(items) => {
            if let Some(names) = items
                .iter()
                .map(|item| item.name.as_deref().map(|name| name.replace(',', "\\,")))
                .collect::<Option<Vec<_>>>()
            {
                return Ok(format!("[{}]", names.join(", ")));
            }
            let spelled = items
                .iter()
                .map(|item| {
                    item.name
                        .as_deref()
                        .map(literal_text)
                        .or_else(|| by_call(item))
                        .ok_or_else(|| unnamed(item))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(serde_json::Value::from(spelled).to_string())
        }
    }
}

fn unnamed(reference: &Ref) -> String {
    format!(
        "`{}` shares its name with another node the recording made, and no call kept made it; \
         give it a name of its own",
        reference.shown
    )
}

/// Every scene node and its id.
fn scene_nodes(world: &mut World) -> Vec<(Entity, u64)> {
    let mut nodes = world.query::<(Entity, &SceneNodeId)>();
    let found: Vec<(Entity, u64)> = nodes
        .iter(world)
        .map(|(entity, id)| (entity, id.0))
        .collect();
    found
        .into_iter()
        .filter(|(entity, _)| crate::entity_refs::is_scene_member(world, *entity))
        .collect()
}

/// The nodes made since the recording began.
fn recorded_nodes(world: &mut World, existing: &HashSet<u64>) -> Vec<(Entity, u64)> {
    scene_nodes(world)
        .into_iter()
        .filter(|(_, id)| !existing.contains(id))
        .collect()
}

/// The recorded nodes with no recorded parent, in the order the document
/// holds them.
fn top_level_nodes(world: &mut World, existing: &HashSet<u64>) -> Vec<(Entity, u64)> {
    let recorded: bevy::platform::collections::HashMap<Entity, u64> =
        recorded_nodes(world, existing).into_iter().collect();
    let Some(doc) = world.get_resource::<jackdaw_bsn::SceneBsnAst>() else {
        return Vec::new();
    };
    doc.roots
        .iter()
        .filter_map(|node| doc.ast_to_ecs.get(node).copied())
        .filter_map(|entity| recorded.get(&entity).map(|node| (entity, *node)))
        .collect()
}

fn capture(world: &mut World) -> Option<Arc<Units>> {
    let parent = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|root| root.root.clone())
        .unwrap_or_default();
    crate::document_capture::capture(world, &parent)
}

/// The node ids a change between two documents touched.
fn changed_nodes(before: &Units, after: &Units) -> Vec<u64> {
    before
        .changes_to(after)
        .into_iter()
        .filter_map(|change| match change.key {
            UnitKey::Node(id) => Some(id),
            UnitKey::Asset(_) => None,
        })
        .collect()
}

fn operator_params(world: &mut World, id: &str) -> Option<&'static [ParamSpec]> {
    let mut ops = world.query::<&OperatorEntity>();
    ops.iter(world)
        .find(|op| op.id() == id)
        .map(OperatorEntity::parameters)
}

/// Whether a parameter names entities: an entity, or the `entities` list.
fn names_entities(spec: &ParamSpec) -> bool {
    spec.ty == "Entity" || (spec.name == "entities" && spec.ty == "String")
}

/// The address of an undo entry, which names it while it moves between the
/// undo and redo stacks.
fn entry_address(entry: Option<&dyn EditorCommand>) -> Option<usize> {
    entry.map(|entry| std::ptr::from_ref::<dyn EditorCommand>(entry).cast::<()>() as usize)
}

fn top_of_undo(world: &World) -> Option<usize> {
    world
        .get_resource::<CommandHistory>()
        .and_then(|history| entry_address(history.undo_stack.last().map(|entry| &**entry)))
}

fn top_of_redo(world: &World) -> Option<usize> {
    world
        .get_resource::<CommandHistory>()
        .and_then(|history| entry_address(history.redo_stack.last().map(|entry| &**entry)))
}

fn before_call(world: &mut World, id: &str, params: &OperatorParameters) {
    if !world.contains_resource::<RecipeRecording>() || id.starts_with("recipe.record.") {
        return;
    }
    settle_gesture(world);
    let kind = match id {
        "history.undo" => PendingKind::Undo,
        "history.redo" => PendingKind::Redo,
        _ => {
            let Some(specs) = operator_params(world, id) else {
                return;
            };
            PendingKind::Call {
                params: translate(world, params, specs),
                before: capture(world),
                names_entities: specs.iter().any(names_entities),
                top_before: top_of_undo(world),
            }
        }
    };
    let own_watch =
        matches!(kind, PendingKind::Call { .. }) && !SpawnedEntities::is_watching(world);
    if own_watch {
        SpawnedEntities::watch(world);
    }
    world.insert_resource(PendingCall { kind, own_watch });
}

fn after_call(world: &mut World, id: &str, result: Option<OperatorResult>) {
    let Some(pending) = world.remove_resource::<PendingCall>() else {
        return;
    };
    let spawned = if pending.own_watch {
        SpawnedEntities::take(world)
    } else {
        SpawnedEntities::peek(world)
    };
    if !world.contains_resource::<RecipeRecording>() || result != Some(OperatorResult::Finished) {
        if result == Some(OperatorResult::Running)
            && let PendingKind::Call { before, .. } = pending.kind
            && let Some(mut recording) = world.get_resource_mut::<RecipeRecording>()
        {
            recording.gesture = Some((id.to_string(), before));
        }
        return;
    }
    let (params, before, names_entities, top_before) = match pending.kind {
        PendingKind::Undo => {
            let undone = top_of_redo(world);
            let mut recording = world.resource_mut::<RecipeRecording>();
            if undone.is_some() && recording.steps.last().and_then(|step| step.entry) == undone {
                let step = recording.steps.pop().expect("a last step");
                recording.undone.push(step);
            }
            return;
        }
        PendingKind::Redo => {
            let redone = top_of_undo(world);
            let mut recording = world.resource_mut::<RecipeRecording>();
            if redone.is_some() && recording.undone.last().and_then(|step| step.entry) == redone {
                let step = recording.undone.pop().expect("a last undone step");
                recording.steps.push(step);
            }
            return;
        }
        PendingKind::Call {
            params,
            before,
            names_entities,
            top_before,
        } => (params, before, names_entities, top_before),
    };
    let after = capture(world);
    let changed = match (&before, &after) {
        (Some(before), Some(after)) => Some(changed_nodes(before, after)),
        _ => None,
    };
    if changed
        .as_ref()
        .is_some_and(|nodes| nodes.is_empty() && before.as_deref() == after.as_deref())
    {
        return;
    }
    let changed = changed.unwrap_or_default();
    let made: Vec<u64> = changed
        .iter()
        .copied()
        .filter(|node| {
            before
                .as_ref()
                .is_some_and(|before| !before.0.contains_key(&UnitKey::Node(*node)))
        })
        .collect();
    let existing = world
        .get_resource::<RecipeRecording>()
        .map(|recording| recording.existing.clone())
        .unwrap_or_default();
    let outside = changed
        .iter()
        .find(|node| existing.contains(*node))
        .copied();
    let unnamed_change = !names_entities
        && changed
            .iter()
            .any(|node| !made.contains(node) && !existing.contains(node));
    let verdict = if id.starts_with("recipe.") {
        Err("a recipe cannot run recipes".to_string())
    } else if let Some(node) = outside {
        Err(format!(
            "it changes `{}`, which the recording did not make",
            node_name(world, node)
        ))
    } else if unnamed_change {
        Err("it acts on the selection and has no parameter to name it by".to_string())
    } else {
        params
    };
    let spawned: Vec<u64> = spawned
        .into_iter()
        .filter_map(|entity| world.get::<SceneNodeId>(entity).map(|node| node.0))
        .collect();
    let top_after = top_of_undo(world);
    let entry = (top_after != top_before).then_some(top_after).flatten();
    let Some(mut recording) = world.get_resource_mut::<RecipeRecording>() else {
        return;
    };
    match verdict {
        Ok(params) => {
            let uid = recording.next_step;
            recording.next_step += 1;
            recording.undone.clear();
            recording.steps.push(Step {
                uid,
                id: id.to_string(),
                params,
                spawned,
                made,
                entry,
            });
        }
        Err(why) => recording.left_out.push(format!("{id}: {why}")),
    }
}

/// Report a gesture that changed the document since it started; a gesture is
/// a pointer at work and has no values to record.
fn settle_gesture(world: &mut World) {
    let Some((id, before)) = world
        .get_resource_mut::<RecipeRecording>()
        .and_then(|mut recording| recording.gesture.take())
    else {
        return;
    };
    let after = capture(world);
    if before.is_some() && before == after {
        return;
    }
    if let Some(mut recording) = world.get_resource_mut::<RecipeRecording>() {
        recording.left_out.push(format!(
            "{id}: a gesture is not recorded; make the change with an operator and its values"
        ));
    }
}

fn node_name(world: &mut World, node: u64) -> String {
    scene_nodes(world)
        .into_iter()
        .find(|(_, id)| *id == node)
        .and_then(|(entity, _)| world.get::<Name>(entity).map(ToString::to_string))
        .unwrap_or_else(|| "a node".to_string())
}

/// The call's parameters as recipe text, with the selection written in for a
/// parameter left to it.
fn translate(
    world: &mut World,
    params: &OperatorParameters,
    specs: &[ParamSpec],
) -> Result<Vec<(String, Param)>, String> {
    let mut out = Vec::new();
    for (key, value) in params.iter() {
        let param = match value {
            PropertyValue::Entity(entity) => Param::One(reference(world, *entity)?),
            PropertyValue::String(text) => match entity_list(world, text) {
                Some(entities) => Param::List(
                    entities
                        .into_iter()
                        .map(|entity| reference(world, entity))
                        .collect::<Result<_, _>>()?,
                ),
                None => Param::Text(literal_text(text)),
            },
            other => Param::Text(literal(other)),
        };
        out.push((key.clone(), param));
    }
    let selected: Vec<Entity> = world
        .get_resource::<crate::selection::Selection>()
        .map(|selection| selection.entities.clone())
        .unwrap_or_default()
        .into_iter()
        .filter(|entity| crate::entity_refs::is_scene_member(world, *entity))
        .collect();
    if selected.is_empty() {
        return Ok(out);
    }
    let left_to_selection = |name: &str, ty: &str| {
        specs.iter().any(|spec| {
            spec.name == name
                && spec.ty == ty
                && spec.doc.contains("selection")
                && !params.contains_key(name)
        })
    };
    if left_to_selection("entities", "String") {
        let refs = selected
            .iter()
            .map(|entity| reference(world, *entity))
            .collect::<Result<_, _>>()?;
        out.push(("entities".to_string(), Param::List(refs)));
    } else if left_to_selection("entity", "Entity") {
        let [only] = selected.as_slice() else {
            return Err(format!(
                "it acts on {} selected nodes; name the one it means",
                selected.len()
            ));
        };
        out.push(("entity".to_string(), Param::One(reference(world, *only)?)));
    }
    Ok(out)
}

/// The scene nodes a text parameter lists by id, if it is such a list.
fn entity_list(world: &World, text: &str) -> Option<Vec<Entity>> {
    let items = crate::entity_refs::list_items(text).ok()?;
    if items.is_empty() {
        return None;
    }
    items
        .iter()
        .map(|item| {
            item.parse::<u64>()
                .ok()
                .and_then(Entity::try_from_bits)
                .filter(|entity| crate::entity_refs::is_scene_member(world, *entity))
        })
        .collect()
}

/// How a recipe names `entity`: never by id.
fn reference(world: &mut World, entity: Entity) -> Result<Ref, String> {
    let Some(id) = world.get::<SceneNodeId>(entity).map(|id| id.0) else {
        return Err(format!("it names {entity}, which is not a scene node"));
    };
    let shown = world
        .get::<Name>(entity)
        .map_or_else(|| entity.to_string(), ToString::to_string);
    let (existing, spawned) = {
        let Some(recording) = world.get_resource::<RecipeRecording>() else {
            return Err("nothing is being recorded".to_string());
        };
        let spawned = recording.steps.iter().rev().find_map(|step| {
            step.spawned
                .iter()
                .position(|made| *made == id)
                .map(|at| (step.uid, at))
        });
        (recording.existing.clone(), spawned)
    };
    if existing.contains(&id) {
        return Err(format!(
            "it names `{shown}`, which the recording did not make"
        ));
    }
    let recorded = recorded_nodes(world, &existing);
    let name = world.get::<Name>(entity).map(ToString::to_string);
    let by_name = name.filter(|name| {
        recorded
            .iter()
            .filter(|(other, _)| {
                world
                    .get::<Name>(*other)
                    .is_some_and(|held| held.as_str() == name)
            })
            .count()
            == 1
    });
    let by_path = || {
        let ids: HashSet<Entity> = recorded.iter().map(|(entity, _)| *entity).collect();
        let path = path_of(world, entity, &ids)?;
        let same = recorded
            .iter()
            .filter(|(other, _)| path_of(world, *other, &ids).as_deref() == Some(path.as_str()))
            .count();
        (same == 1).then_some(path)
    };
    Ok(Ref {
        name: by_name.or_else(by_path),
        spawned,
        shown,
    })
}

/// The path to `entity` from the topmost recorded node above it, one escaped
/// name per level.
fn path_of(world: &World, entity: Entity, recorded: &HashSet<Entity>) -> Option<String> {
    let mut segments = Vec::new();
    let mut at = entity;
    loop {
        segments.push(escape_segment(world.get::<Name>(at)?.as_str()));
        match world.get::<ChildOf>(at).map(ChildOf::parent) {
            Some(parent) if recorded.contains(&parent) => at = parent,
            _ => break,
        }
    }
    segments.reverse();
    Some(segments.join("/"))
}

/// One name of a path, every character a reference reads specially escaped.
fn escape_segment(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let digits = !name.is_empty() && name.bytes().all(|b| b.is_ascii_digit());
    for (at, c) in name.chars().enumerate() {
        if matches!(c, '/' | '&' | '#' | '!' | '*' | ',' | '\\') || (digits && at == 0) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Text a recipe passes through as written: a leading `$` is doubled, so it
/// is not read as a result reference.
fn literal_text(text: &str) -> String {
    match text.starts_with('$') {
        true => format!("${text}"),
        false => text.to_string(),
    }
}

/// A parameter value as recipe text.
fn literal(value: &PropertyValue) -> String {
    let vector = |values: &[f32]| {
        let parts: Vec<String> = values.iter().map(ToString::to_string).collect();
        format!("[{}]", parts.join(", "))
    };
    match value {
        PropertyValue::Bool(flag) => flag.to_string(),
        PropertyValue::Int(n) => n.to_string(),
        PropertyValue::Float(n) => number_text(*n),
        PropertyValue::String(text) => literal_text(text),
        PropertyValue::Vec2(v) => vector(&v.to_array()),
        PropertyValue::Vec3(v) => vector(&v.to_array()),
        PropertyValue::Color(color) => vector(&color.to_srgba().to_f32_array()),
        PropertyValue::Entity(entity) => entity.to_bits().to_string(),
    }
}

/// A number in its shortest spelling, reading one that came from a 32-bit
/// value as that value.
fn number_text(n: f64) -> String {
    let narrow = n as f32;
    if f64::from(narrow) == n {
        narrow.to_string()
    } else {
        n.to_string()
    }
}
