//! The batch machinery a recipe runs on: its calls checked up front as an
//! atomic batch's are, then run as one undo entry into the recipe's root,
//! which holds everything they build.

use bevy::prelude::*;
use jackdaw_api_internal::operator::{with_batch_transaction, with_outer_call_reports_set_aside};
use serde_json::{Value, json};

use super::server::{BatchPlan, BatchRunner, Pending, call_from_json, dispatch};
use super::stage::OpenStage;
use crate::commands::SpawnedEntities;

/// Where a run builds.
pub(crate) enum RecipeRoot {
    /// A new group, added by `entity.add.group` with these parameters.
    New(Value),
    /// A root a recipe built before, emptied first.
    Existing(Entity),
}

/// What a run built.
pub(crate) struct Built {
    pub root: Entity,
    /// Nodes the run deleted from the root before building.
    pub replaced: usize,
}

/// A recipe's calls, checked as an atomic batch's are and ready to run.
pub(crate) struct RecipePlan {
    plan: BatchPlan,
    prepared: Vec<Pending>,
}

impl RecipePlan {
    /// Check `calls`, refusing what an atomic batch refuses, before anything
    /// runs.
    pub(crate) fn new(world: &mut World, calls: &[Value]) -> Result<Self, String> {
        let (plan, prepared) =
            BatchPlan::from_params(world, &json!({ "calls": calls, "atomic": true }))
                .map_err(|err| err.message)?;
        Ok(Self { plan, prepared })
    }
}

/// Run `plan` into the root as one undo entry labelled `label`, then `finish`
/// on the root. Anything that fails takes the whole run
/// back and says why; the scene and the history are left as they were.
///
/// The calls report to this run, not to the call that started it; the root
/// is the one entity that call is told it added.
pub(crate) fn build(
    world: &mut World,
    label: String,
    root: RecipeRoot,
    plan: RecipePlan,
    finish: impl FnOnce(&mut World, Entity),
) -> Result<Built, String> {
    let held = SpawnedEntities::set_aside(world);
    let new_root = matches!(root, RecipeRoot::New(_));
    let built = with_batch_transaction(world, label, true, |world| {
        let built =
            with_outer_call_reports_set_aside(world, |world| build_in(world, root, plan, finish));
        let keep = built.is_ok();
        (built, keep)
    });
    SpawnedEntities::put_back(world, held);
    if let Ok(built) = &built {
        if new_root {
            SpawnedEntities::record(world, built.root);
        }
        crate::selection::select_only(world, built.root);
    }
    built
}

fn build_in(
    world: &mut World,
    root: RecipeRoot,
    plan: RecipePlan,
    finish: impl FnOnce(&mut World, Entity),
) -> Result<Built, String> {
    let root = match root {
        RecipeRoot::Existing(root) => root,
        RecipeRoot::New(params) => add_root(world, params)?,
    };
    let name = world
        .get::<Name>(root)
        .map_or_else(|| root.to_string(), |name| name.as_str().to_string());
    let open = OpenStage::open_root(world, root, name)?;
    let replaced = open.replaced();
    let RecipePlan { plan, prepared } = plan;
    let mut runner = BatchRunner::start_in(prepared, open, root);
    while !runner.is_done() {
        runner.step(world);
    }
    let run = runner.finish(world, &[]);
    if let Some(failure) = plan.failure(&run) {
        return Err(failure.what);
    }
    finish(world, root);
    Ok(Built { root, replaced })
}

fn add_root(world: &mut World, params: Value) -> Result<Entity, String> {
    let call = json!({ "id": "entity.add.group", "params": params });
    let outcome = call_from_json(world, &call)
        .and_then(|call| dispatch(world, call, false))
        .map_err(|err| format!("the root could not be added: {}", err.message))?;
    let answer = outcome.to_json(true);
    outcome
        .entities
        .first()
        .and_then(|bits| Entity::try_from_bits(*bits))
        .ok_or_else(|| {
            let warnings: Vec<&str> = answer["warnings"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect();
            format!("the root could not be added: {}", warnings.join("; "))
        })
}

/// Run one call from inside an operator, answering the entities it made, or
/// why it did not finish.
pub(crate) fn call_inside(world: &mut World, call: Value) -> Result<Vec<Entity>, String> {
    let id = call["id"].as_str().unwrap_or_default().to_string();
    let outcome = call_from_json(world, &call)
        .and_then(|call| dispatch(world, call, false))
        .map_err(|err| format!("{id}: {}", err.message))?;
    let answer = outcome.to_json(true);
    if answer["result"] != json!("finished") {
        let warnings: Vec<&str> = answer["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        return Err(format!("{id} did not finish: {}", warnings.join("; ")));
    }
    Ok(outcome
        .entities
        .iter()
        .filter_map(|bits| Entity::try_from_bits(*bits))
        .collect())
}
