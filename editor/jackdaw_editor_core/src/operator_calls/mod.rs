//! Operator calls written as JSON: one `{id, params}` call typed from the
//! operator's declared parameters and dispatched, or a batch of them run as
//! one undo entry with `$N.path` references between calls, an optional build
//! stage and geometric checks. The editor's remote surface and saved recipes
//! both run calls through here.

pub mod stage;
pub mod summary;
pub mod topology;

use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult, error_codes};
use jackdaw_api_internal::lifecycle::OperatorEntity;
use jackdaw_api_internal::operator::{
    BooleanReport, CallOperatorError, CallOperatorSettings, ExecutionContext,
    OperatorBooleanReport, OperatorOutcome, OperatorParameters, OperatorReplacements,
    OperatorReports, OperatorResult, OperatorWarnings, OperatorWorldExt, ParamSpec,
    with_batch_transaction, with_history_span,
};
use jackdaw_scene_types::PropertyValue;
use serde::Serialize;
use serde_json::{Value, json};

use crate::boot_ops::{BootOp, resolve_entity_params};

pub fn invalid_params(message: impl Into<String>) -> BrpError {
    BrpError {
        code: error_codes::INVALID_PARAMS,
        message: message.into(),
        data: None,
    }
}

pub fn internal_error(message: impl Into<String>) -> BrpError {
    BrpError {
        code: error_codes::INTERNAL_ERROR,
        message: message.into(),
        data: None,
    }
}

/// What is registered under `id`: its declared parameter schemas, and whether
/// anything answers to it at all. Every registration is read, since a few ids
/// are declared twice.
fn lookup_operator(
    In(id): In<String>,
    ops: Query<&OperatorEntity>,
) -> (bool, Vec<&'static ParamSpec>) {
    let mut found = false;
    let mut specs = Vec::new();
    for op in &ops {
        if op.id() != id {
            continue;
        }
        found = true;
        specs.extend(op.parameters().iter());
    }
    (found, specs)
}

/// Type one JSON value as the parameter `spec` declares it. Only an undeclared
/// parameter falls back to guessing from the spelling.
pub fn property_from_json(spec: Option<&ParamSpec>, value: &Value) -> Option<PropertyValue> {
    let Some(spec) = spec else {
        return untyped_property(value);
    };
    match spec.ty {
        "Bool" => match value {
            Value::Bool(flag) => Some(PropertyValue::Bool(*flag)),
            Value::String(text) => text.parse().ok().map(PropertyValue::Bool),
            Value::Number(number) => number.as_i64().map(|n| PropertyValue::Bool(n != 0)),
            _ => None,
        },
        "Int" => match value {
            Value::Number(number) => number.as_i64().map(PropertyValue::Int),
            Value::String(text) => text.trim().parse().ok().map(PropertyValue::Int),
            Value::Bool(flag) => Some(PropertyValue::Int(i64::from(*flag))),
            _ => None,
        },
        "Float" => match value {
            Value::Number(number) => number.as_f64().map(PropertyValue::Float),
            Value::String(text) => text.trim().parse().ok().map(PropertyValue::Float),
            _ => None,
        },
        "String" => Some(PropertyValue::String(
            match value {
                Value::String(text) => text.clone(),
                Value::Null => return None,
                other => other.to_string(),
            }
            .into(),
        )),
        "Vec2" => vec_of(value, 2).map(|v| PropertyValue::Vec2(Vec2::new(v[0], v[1]))),
        "Vec3" => vec_of(value, 3).map(|v| PropertyValue::Vec3(Vec3::new(v[0], v[1], v[2]))),
        "Color" => vec_of(value, 4)
            .map(|v| PropertyValue::Color(Color::srgba(v[0], v[1], v[2], v[3])))
            .or_else(|| {
                vec_of(value, 3).map(|v| PropertyValue::Color(Color::srgb(v[0], v[1], v[2])))
            }),
        // Everything else, `Entity` included, is read by its spelling; an entity
        // is resolved after typing.
        _ => untyped_property(value),
    }
}

/// A value with no declared type, read the way a text clause reads one.
fn untyped_property(value: &Value) -> Option<PropertyValue> {
    match value {
        Value::Bool(flag) => Some(PropertyValue::Bool(*flag)),
        Value::Number(number) => number
            .as_i64()
            .map(PropertyValue::Int)
            .or_else(|| number.as_f64().map(PropertyValue::Float)),
        Value::String(text) => Some(crate::boot_ops::parse_value(text)),
        _ => None,
    }
}

/// `len` floats out of a JSON array, or out of a comma-separated string.
pub fn vec_of(value: &Value, len: usize) -> Option<Vec<f32>> {
    let parts: Vec<f32> = match value {
        Value::Array(items) => items
            .iter()
            .filter_map(|item| item.as_f64().map(|f| f as f32))
            .collect(),
        Value::String(text) => text
            .split(',')
            .filter_map(|part| part.trim().parse::<f32>().ok())
            .collect(),
        _ => return None,
    };
    (parts.len() == len).then_some(parts)
}
/// The entity a `{"entity": <id>}` or `{"name": "<reference>"}` field means;
/// the reference is an id, a name path or a query matching one node.
pub fn entity_from_params(world: &mut World, params: &Value) -> Result<Entity, BrpError> {
    if let Some(bits) = params.get("entity").and_then(Value::as_u64) {
        return Entity::try_from_bits(bits)
            .filter(|entity| world.get_entity(*entity).is_ok())
            .ok_or_else(|| invalid_params(format!("no entity with id {bits}")));
    }
    let Some(name) = params
        .get("name")
        .or_else(|| params.get("entity"))
        .and_then(Value::as_str)
    else {
        return Err(invalid_params(
            "expected an \"entity\" id or a \"name\"".to_string(),
        ));
    };
    crate::entity_refs::resolve_one(world, name).map_err(invalid_params)
}

/// One operator call, typed from JSON and dispatched on the main world.
pub fn call_operator(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let params = params.ok_or_else(|| invalid_params("expected {\"id\": \"<operator id>\"}"))?;
    let history = params
        .get("history")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let call = call_from_json(world, &params)?;
    let outcome = dispatch(world, call, history)?;
    Ok(outcome.to_json(verbose(&params)))
}

/// Run a batch of calls as one undo entry. A call that stops the batch leaves
/// the earlier ones done, and the error says so.
///
/// A string parameter spelled `$N.path` takes the value at `path` in call
/// `N`'s outcome, e.g. `$0.entities.0` for the first entity call 0 added. A
/// malformed reference, or one to a call that does not run earlier, refuses
/// the whole batch before anything runs.
///
/// `atomic` takes the whole batch back when a call fails, does not finish or a
/// check fails: the scene and the history are left as they were. `dry_run`
/// runs the calls and checks, reports them, and takes everything back.
/// `stage` builds the calls' new nodes into the group of that name, deleting
/// what an earlier run built there, and refuses a call that changes anything
/// outside it; `after` names stages that must already exist. A staged batch
/// is atomic. `checks` are `jackdaw/check` assertions evaluated after the
/// calls; one that fails fails the batch. Atomic, dry-run and staged batches
/// refuse calls a rollback cannot take back, such as saving a file. A batch
/// played with a pause between calls goes to `jackdaw/paced_batch`.
pub fn run_batch(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    let params = params.ok_or_else(|| invalid_params("expected {\"calls\": [...]}"))?;
    if params.get("pace_ms").is_some_and(|pace| !pace.is_null()) {
        return Err(invalid_params(
            "`pace_ms` plays a batch over frames: send it to `jackdaw/paced_batch` with a \
             `request` id",
        ));
    }
    let (plan, prepared) = BatchPlan::from_params(world, &params)?;
    let run = with_batch_transaction(world, plan.label.clone(), plan.atomic, |world| {
        let mut runner = BatchRunner::start(world, prepared, plan.stage.as_ref());
        while !runner.is_done() {
            runner.step(world);
        }
        let run = runner.finish(world, &plan.checks);
        let keep = plan.keeps(&run);
        (run, keep)
    });
    plan.answer(run)
}

/// Whether the caller asked for the whole outcome rather than the compact one.
pub fn verbose(params: &Value) -> bool {
    params
        .get("verbose")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// A batch's parameters, checked, and what it needs to answer once it has run.
pub struct BatchPlan {
    calls: Vec<Value>,
    pub label: String,
    verbose: bool,
    dry_run: bool,
    pub atomic: bool,
    pub stage: Option<stage::StageSpec>,
    pub checks: Vec<Value>,
}

impl BatchPlan {
    /// Check a batch's parameters and type every call that can be typed before
    /// anything runs.
    pub fn from_params(
        world: &mut World,
        params: &Value,
    ) -> Result<(Self, Vec<Pending>), BrpError> {
        let Some(calls) = params.get("calls").and_then(Value::as_array) else {
            return Err(invalid_params("expected {\"calls\": [...]}"));
        };
        let label = params
            .get("label")
            .and_then(Value::as_str)
            .unwrap_or("Batch")
            .to_string();
        let dry_run = flag(params, "dry_run")?;
        let stage = stage::StageSpec::from_params(params)?;
        let atomic = flag(params, "atomic")? || dry_run || stage.is_some();
        let checks = match params.get("checks") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(checks)) => checks.clone(),
            Some(_) => return Err(invalid_params("`checks` is a list of assertions")),
        };
        for (index, check) in checks.iter().enumerate() {
            let refused = |err: BrpError| invalid_params(format!("check {index}: {}", err.message));
            summary::check_shape(check).map_err(refused)?;
            check_references(Some(check), calls.len()).map_err(refused)?;
        }
        if atomic {
            refuse_unrecoverable(world, calls)?;
        }
        if let Some(stage) = &stage {
            stage.check(world)?;
        }

        let mut prepared = Vec::with_capacity(calls.len());
        for (index, call) in calls.iter().enumerate() {
            let refused = |err: BrpError| invalid_params(format!("call {index}: {}", err.message));
            let referenced = check_references(call.get("params"), index).map_err(refused)?;
            if referenced {
                // Everything but the referencing parameters is typed now, so a
                // mistake there refuses the batch before anything runs.
                let mut known = call.clone();
                if let Some(params) = known.get_mut("params").and_then(Value::as_object_mut) {
                    params.retain(|_, value| !holds_reference(value));
                }
                typed_call(world, &fill_references(&known, &[]).map_err(refused)?)
                    .map_err(refused)?;
                prepared.push(Pending::Referencing(call.clone()));
            } else {
                // Typed now so a mistake refuses the batch, but its entity
                // references resolve when it runs, against what earlier calls left.
                let call = fill_references(call, &[]).map_err(refused)?;
                typed_call(world, &call).map_err(refused)?;
                prepared.push(Pending::Ready(call));
            }
        }
        let plan = Self {
            calls: calls.clone(),
            label,
            verbose: verbose(params),
            dry_run,
            atomic,
            stage,
            checks,
        };
        Ok((plan, prepared))
    }

    /// Whether what `run` did stays.
    pub fn keeps(&self, run: &BatchRun) -> bool {
        match run.failure {
            Some(_) => !self.atomic,
            None => !self.dry_run,
        }
    }

    /// The answer, or the error, for a batch that ran as `run`.
    pub fn answer(&self, run: BatchRun) -> BrpResult {
        let (dry_run, atomic, verbose) = (self.dry_run, self.atomic, self.verbose);
        let checks = &self.checks;
        let done: Vec<Value> = run.done.iter().map(|call| call.to_json(verbose)).collect();
        let checks_skipped = !checks.is_empty()
            && matches!(
                run.failure,
                Some(BatchFailure::Call(..) | BatchFailure::Stopped(_) | BatchFailure::Stage(_))
            );
        let Some(failure) = self.failure(&run) else {
            return Ok(batch_answer(done, dry_run, run.stage, run.checks));
        };
        let mut what = failure.what;
        if checks_skipped {
            what.push_str(", so the checks were not run");
        }
        let message = if atomic {
            format!(
                "{what}. The batch was rolled back: the scene and the undo history are as they were."
            )
        } else {
            format!(
                "{what}. The {} call(s) that ran are done and are committed as one undo entry; undo \
                 once to take them back rather than retrying the whole batch.",
                done.len()
            )
        };
        let mut data =
            json!({ "failed_at": failure.failed_at, "calls": done, "rolled_back": atomic });
        if dry_run {
            data["dry_run"] = json!(true);
        }
        if let Some(checks) = run.checks {
            data["checks"] = checks;
        }
        Err(BrpError {
            code: failure.code,
            message,
            data: Some(data),
        })
    }

    /// What stopped `run`, or `None` for a run that did all it was asked, or
    /// a plain batch whose call merely did not finish.
    pub fn failure(&self, run: &BatchRun) -> Option<BatchFailureText> {
        let calls = &self.calls;
        let call_id = |index: usize| calls[index]["id"].as_str().unwrap_or("?");
        let (code, what, failed_at) = match run.failure.as_ref()? {
            BatchFailure::Stopped(_) if !self.atomic && self.checks.is_empty() => return None,
            BatchFailure::Call(index, err) => (
                err.code,
                format!("call {index} ({}) failed: {}", call_id(*index), err.message),
                json!(*index),
            ),
            BatchFailure::Stopped(index) => {
                let outcome = run.done[*index].to_json(true);
                let mut what = format!(
                    "call {index} ({}) did not finish: {}",
                    call_id(*index),
                    outcome["result"].as_str().unwrap_or("?")
                );
                if let Some(warnings) = outcome["warnings"].as_array()
                    && !warnings.is_empty()
                {
                    let warnings: Vec<&str> = warnings.iter().filter_map(Value::as_str).collect();
                    what.push_str(&format!(" ({})", warnings.join("; ")));
                }
                (error_codes::INVALID_PARAMS, what, json!(*index))
            }
            BatchFailure::Stage(why) => (
                error_codes::INTERNAL_ERROR,
                format!("the stage could not be opened: {why}"),
                Value::Null,
            ),
            BatchFailure::Checks => {
                let results = run
                    .checks
                    .as_ref()
                    .and_then(|checks| checks["results"].as_array());
                let failed: Vec<String> = results
                    .into_iter()
                    .flatten()
                    .enumerate()
                    .filter(|(_, result)| result["pass"] != json!(true))
                    .map(|(index, result)| {
                        format!("{index} ({})", result["check"].as_str().unwrap_or("?"))
                    })
                    .collect();
                (
                    error_codes::INVALID_PARAMS,
                    format!(
                        "{} of {} checks failed: {}",
                        failed.len(),
                        results.map_or(0, Vec::len),
                        failed.join(", ")
                    ),
                    Value::Null,
                )
            }
            BatchFailure::CheckError(err) => (
                err.code,
                format!("the checks could not run: {}", err.message),
                Value::Null,
            ),
        };
        Some(BatchFailureText {
            code,
            what,
            failed_at,
        })
    }
}

/// Why a batch failed, as its error says it.
pub struct BatchFailureText {
    pub code: i16,
    /// What failed, without what became of the calls that ran.
    pub what: String,
    pub failed_at: Value,
}

/// What a batch that did all it was asked answers.
fn batch_answer(
    done: Vec<Value>,
    dry_run: bool,
    stage: Option<Value>,
    checks: Option<Value>,
) -> Value {
    let mut answer = json!({ "calls": done });
    if dry_run {
        answer["dry_run"] = json!(true);
    }
    if let Some(stage) = stage {
        answer["stage"] = stage;
    }
    if let Some(checks) = checks {
        answer["checks"] = checks;
    }
    answer
}

/// A boolean parameter, false when absent.
pub fn flag(params: &Value, key: &str) -> Result<bool, BrpError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(other) => Err(invalid_params(format!(
            "`{key}` is true or false, not {other}"
        ))),
    }
}

/// Why a rollback could not take a call to `id` back, from the flags the
/// operator declares; `None` when it could.
fn beyond_rollback(In(id): In<String>, ops: Query<&OperatorEntity>) -> Option<&'static str> {
    let op = ops.iter().find(|op| op.id() == id)?;
    if op.writes_disk() {
        Some("writes to disk")
    } else if op.leaves_history() {
        Some("leaves the history the batch records into")
    } else {
        None
    }
}

/// Refuse a batch that must be able to roll back but holds a call that writes
/// to disk or leaves the batch's history, naming every such call.
fn refuse_unrecoverable(world: &mut World, calls: &[Value]) -> Result<(), BrpError> {
    let mut found: Vec<(usize, String, &'static str)> = Vec::new();
    for (index, call) in calls.iter().enumerate() {
        let Some(id) = call.get("id").and_then(Value::as_str) else {
            continue;
        };
        let why = world
            .run_system_cached_with(beyond_rollback, id.to_string())
            .map_err(|err| internal_error(err.to_string()))?;
        if let Some(why) = why {
            found.push((index, id.to_string(), why));
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    let listed: Vec<String> = found
        .iter()
        .map(|(index, id, why)| format!("call {index} ({id}) {why}"))
        .collect();
    Err(BrpError {
        code: error_codes::INVALID_PARAMS,
        message: format!(
            "{}; a rollback cannot take that back, so an atomic, dry-run or staged batch refuses \
             it. Run it in a plain batch or call once this one is done.",
            listed.join("; ")
        ),
        data: Some(json!({
            "unrecoverable": found
                .iter()
                .map(|(index, id, why)| json!({ "call": index, "id": id, "why": why }))
                .collect::<Vec<_>>(),
        })),
    })
}

/// What running a batch's calls did.
#[derive(Default)]
pub struct BatchRun {
    /// Every call that ran, the one that stopped the batch included.
    done: Vec<CallOutcome>,
    failure: Option<BatchFailure>,
    stage: Option<Value>,
    checks: Option<Value>,
}

/// Why a batch did not do all it was asked.
enum BatchFailure {
    /// The call at this index failed, or a stage refused what it did.
    Call(usize, BrpError),
    /// The call at this index ended without finishing.
    Stopped(usize),
    Stage(String),
    /// An assertion failed; the results say which.
    Checks,
    /// An assertion could not be evaluated.
    CheckError(BrpError),
}

/// A batch's calls run one step at a time, so a paced batch can let frames
/// pass between them.
pub struct BatchRunner {
    pending: std::vec::IntoIter<Pending>,
    index: usize,
    earlier: Vec<Value>,
    open: Option<stage::OpenStage>,
    /// The root references are held to while a call runs, and whether the
    /// selection is cleared before each call so none acts on what an earlier
    /// one selected.
    scope: Option<Entity>,
    run: BatchRun,
}

impl BatchRunner {
    /// Ready to run `prepared`, with the stage open if one is named.
    pub fn start(
        world: &mut World,
        prepared: Vec<Pending>,
        stage: Option<&stage::StageSpec>,
    ) -> Self {
        let mut run = BatchRun::default();
        let open = match stage.map(|spec| stage::OpenStage::open(world, spec)) {
            None => None,
            Some(Ok(open)) => Some(open),
            Some(Err(why)) => {
                run.failure = Some(BatchFailure::Stage(why));
                None
            }
        };
        Self {
            earlier: Vec::with_capacity(prepared.len()),
            pending: prepared.into_iter(),
            index: 0,
            open,
            scope: None,
            run,
        }
    }

    /// Ready to run `prepared` into `root`, opened as a stage: every entity
    /// reference a call makes names only nodes under it, and the selection is
    /// cleared before each call.
    pub fn start_in(prepared: Vec<Pending>, open: stage::OpenStage, root: Entity) -> Self {
        Self {
            earlier: Vec::with_capacity(prepared.len()),
            pending: prepared.into_iter(),
            index: 0,
            open: Some(open),
            scope: Some(root),
            run: BatchRun::default(),
        }
    }

    /// Whether every call has run or one stopped the batch.
    pub fn is_done(&self) -> bool {
        self.run.failure.is_some() || self.pending.len() == 0
    }

    /// Run the next call.
    pub fn step(&mut self, world: &mut World) {
        if self.run.failure.is_some() {
            return;
        }
        let Some(pending) = self.pending.next() else {
            return;
        };
        let index = self.index;
        self.index += 1;
        if self.scope.is_some() {
            crate::selection::clear_selection_in_world(world);
        }
        let outer_scope = world
            .get_resource_or_init::<crate::entity_refs::ReferenceScope>()
            .0;
        world.resource_mut::<crate::entity_refs::ReferenceScope>().0 = self.scope.or(outer_scope);
        let call = match pending {
            Pending::Ready(call) => call_from_json(world, &call),
            Pending::Referencing(call) => {
                fill_references(&call, &self.earlier).and_then(|call| call_from_json(world, &call))
            }
        };
        let dispatched = call.and_then(|call| dispatch(world, call, false));
        world.resource_mut::<crate::entity_refs::ReferenceScope>().0 = outer_scope;
        let outcome = match dispatched {
            Ok(outcome) => outcome,
            Err(err) => {
                self.run.failure = Some(BatchFailure::Call(index, err));
                return;
            }
        };
        if outcome.result == "running" {
            // A modal operator waits for a pointer that is never coming, and
            // holds off every later modal call.
            if let Err(err) = world.cancel_active_modal() {
                warn!("jackdaw/batch: could not cancel the modal call: {err}");
            }
        }
        let finished = outcome.result == "finished";
        self.earlier.push(outcome.to_json(true));
        self.run.done.push(outcome);
        if !finished {
            self.run.failure = Some(BatchFailure::Stopped(index));
            return;
        }
        if let Some(open) = self.open.as_mut()
            && let Err(why) = open.after_call(world)
        {
            self.run.failure = Some(BatchFailure::Call(index, invalid_params(why)));
        }
    }

    /// Report the stage and run `checks` against what the calls left.
    pub fn finish(mut self, world: &mut World, checks: &[Value]) -> BatchRun {
        if self.run.failure.is_some() {
            return self.run;
        }
        self.run.stage = self.open.as_ref().map(stage::OpenStage::report);
        if checks.is_empty() {
            return self.run;
        }
        let checked =
            fill_references(&json!({ "params": checks }), &self.earlier).and_then(|filled| {
                let assertions = filled["params"].as_array().cloned().unwrap_or_default();
                summary::run_checks(world, &assertions)
            });
        match checked {
            Ok(result) => {
                if result["failed"] != json!(0) {
                    self.run.failure = Some(BatchFailure::Checks);
                }
                self.run.checks = Some(result);
            }
            Err(err) => self.run.failure = Some(BatchFailure::CheckError(err)),
        }
        self.run
    }
}

/// A batch call, checked up front, or held until the outcomes it references
/// are in.
pub enum Pending {
    Ready(Value),
    Referencing(Value),
}

/// The fields of an outcome a reference can start from.
const OUTCOME_FIELDS: [&str; 10] = [
    "id", "result", "warnings", "entities", "reports", "outcome", "removed", "created", "bounds",
    "report",
];

/// A `$N.path` reference: the value at `path` in call `N`'s outcome.
#[derive(Debug, PartialEq, Eq)]
struct ResultRef<'a> {
    call: usize,
    path: Vec<&'a str>,
}

/// Read `text` as a reference. `Ok(None)` is a plain string: one that does not
/// start with `$` and a digit, or a `$$` that escapes a literal `$`.
fn parse_reference(text: &str) -> Result<Option<ResultRef<'_>>, String> {
    let Some(rest) = text.strip_prefix('$') else {
        return Ok(None);
    };
    if !rest.starts_with(|c: char| c.is_ascii_digit()) {
        return Ok(None);
    }
    let mut parts = rest.split('.');
    let call = parts
        .next()
        .and_then(|index| index.parse::<usize>().ok())
        .ok_or_else(|| {
            format!(
                "`{text}` is not a reference: expected `$N.field`; write `$${rest}` for text \
                 that starts with `$`"
            )
        })?;
    let path: Vec<&str> = parts.collect();
    let Some(field) = path.first() else {
        return Err(format!(
            "`{text}` names a whole outcome; name a value in it, e.g. `${call}.entities.0`"
        ));
    };
    if path.iter().any(|segment| segment.is_empty()) {
        return Err(format!("`{text}` has an empty step in its path"));
    }
    if !OUTCOME_FIELDS.contains(field) {
        return Err(format!(
            "`{text}`: an outcome has no `{field}`; it has {}",
            OUTCOME_FIELDS.join(", ")
        ));
    }
    Ok(Some(ResultRef { call, path }))
}

/// Check every reference in a call's parameters against the calls that run
/// before it. Returns whether the call has any.
fn check_references(params: Option<&Value>, index: usize) -> Result<bool, BrpError> {
    let mut found = false;
    let mut pending: Vec<&Value> = params.into_iter().collect();
    while let Some(value) = pending.pop() {
        match value {
            Value::String(text) => {
                let Some(reference) = parse_reference(text).map_err(invalid_params)? else {
                    continue;
                };
                if reference.call >= index {
                    return Err(invalid_params(format!(
                        "`{text}` names call {}, which does not run before call {index}",
                        reference.call
                    )));
                }
                found = true;
            }
            Value::Array(items) => pending.extend(items),
            Value::Object(fields) => pending.extend(fields.values()),
            _ => {}
        }
    }
    Ok(found)
}

/// Whether `value` holds a reference anywhere inside it.
fn holds_reference(value: &Value) -> bool {
    match value {
        Value::String(text) => matches!(parse_reference(text), Ok(Some(_))),
        Value::Array(items) => items.iter().any(holds_reference),
        Value::Object(fields) => fields.values().any(holds_reference),
        _ => false,
    }
}

/// A call with every reference in its parameters replaced by the value it
/// names in `earlier`, and every `$$` escape read as one `$`.
fn fill_references(call: &Value, earlier: &[Value]) -> Result<Value, BrpError> {
    fn fill(value: &mut Value, earlier: &[Value]) -> Result<(), BrpError> {
        match value {
            Value::String(text) => {
                if let Some(literal) = text.strip_prefix("$$") {
                    *text = format!("${literal}");
                    return Ok(());
                }
                let Some(reference) = parse_reference(text).map_err(invalid_params)? else {
                    return Ok(());
                };
                let mut found = earlier.get(reference.call);
                for segment in &reference.path {
                    found = found.and_then(|at| match at {
                        Value::Array(items) => {
                            segment.parse::<usize>().ok().and_then(|i| items.get(i))
                        }
                        other => other.get(segment),
                    });
                }
                let found = found.filter(|found| !found.is_null()).ok_or_else(|| {
                    invalid_params(format!(
                        "`{text}`: call {} reported no such value",
                        reference.call
                    ))
                })?;
                *value = found.clone();
            }
            Value::Array(items) => {
                for item in items {
                    fill(item, earlier)?;
                }
            }
            Value::Object(fields) => {
                for field in fields.values_mut() {
                    fill(field, earlier)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    let mut call = call.clone();
    if let Some(params) = call.get_mut("params") {
        fill(params, earlier)?;
    }
    Ok(call)
}

/// One prepared call: an id, its typed parameters, and how each `Entity`
/// parameter was filled in.
pub struct PreparedCall {
    op: BootOp,
    unresolved: Vec<String>,
    /// A reference named nothing, or too much, so the call is refused unrun.
    blocked: bool,
}

/// What a call did.
#[derive(Serialize)]
pub struct CallOutcome {
    id: String,
    /// `finished`, `cancelled` or `running`.
    result: String,
    /// Anything the resolver could not fill in, plus whatever the operator
    /// reported through `OperatorWarnings`. Empty on a clean call.
    warnings: Vec<String>,
    /// Scene entities the call added, as ids the other methods take.
    pub entities: Vec<u64>,
    /// What the operator did, where the amount is the answer and the scene does
    /// not say it. Kept apart from `warnings` so a receipt is not read as a
    /// refusal.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reports: Vec<String>,
    /// `changed`, `empty`, `unchanged` or `invalid`, from an operator that says
    /// which.
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<&'static str>,
    /// Scene entities the call took away, from an operator that replaces them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    removed: Vec<u64>,
    /// Scene entities the call made in their place.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    created: Vec<u64>,
    /// The world-space `[min, max]` box of each of `created`, in order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bounds: Vec<Value>,
    /// What a boolean made or, on a dry run, would make.
    #[serde(skip_serializing_if = "Option::is_none")]
    report: Option<Value>,
}

impl CallOutcome {
    /// The outcome as a caller reads it. Compact leaves out the operator id the
    /// caller already sent and every empty list but `entities`.
    pub fn to_json(&self, verbose: bool) -> Value {
        let mut value = json!(self);
        if !verbose && let Some(fields) = value.as_object_mut() {
            fields.remove("id");
            fields.retain(|name, field| {
                name == "entities" || field.as_array().is_none_or(|items| !items.is_empty())
            });
        }
        value
    }
}

/// Type a `{id, params}` object into a call the dispatcher can take.
pub fn call_from_json(world: &mut World, call: &Value) -> Result<PreparedCall, BrpError> {
    let mut op = typed_call(world, call)?;
    let outcomes = resolve_entity_params(world, &mut op);
    let blocked = crate::boot_ops::blocked(&outcomes);
    let unresolved = outcomes
        .into_iter()
        .filter(crate::boot_ops::EntityParam::is_refusal)
        .filter_map(|outcome| outcome.line(&op.id))
        .collect();
    Ok(PreparedCall {
        op,
        unresolved,
        blocked,
    })
}

/// Check a call's operator exists and type its parameters from the schema,
/// leaving entity parameters unresolved.
fn typed_call(world: &mut World, call: &Value) -> Result<BootOp, BrpError> {
    let id = call
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid_params("expected an \"id\""))?
        .to_string();
    let (exists, specs) = world
        .run_system_cached_with(lookup_operator, id.clone())
        .map_err(|err| internal_error(err.to_string()))?;
    if !exists {
        return Err(invalid_params(format!("unknown operator: {id}")));
    }

    let mut params = Vec::new();
    if let Some(given) = call.get("params").and_then(Value::as_object) {
        for (key, value) in given {
            let spec = specs.iter().copied().find(|spec| spec.name == key);
            let Some(typed) = property_from_json(spec, value) else {
                return Err(invalid_params(format!(
                    "{id}: `{key}` = {value} is not a {}",
                    spec.map_or("value", |spec| spec.ty)
                )));
            };
            params.push((key.clone(), typed));
        }
    }

    Ok(BootOp { id, params })
}

/// Dispatch a prepared call and describe what happened.
pub fn dispatch(
    world: &mut World,
    call: PreparedCall,
    history: bool,
) -> Result<CallOutcome, BrpError> {
    let PreparedCall {
        op,
        unresolved,
        blocked,
    } = call;
    let id = op.id.clone();
    if blocked {
        return Ok(CallOutcome {
            id,
            result: "cancelled".to_string(),
            warnings: unresolved,
            entities: Vec::new(),
            reports: Vec::new(),
            outcome: Some(jackdaw_api_internal::operator::Outcome::Invalid.as_str()),
            removed: Vec::new(),
            created: Vec::new(),
            bounds: Vec::new(),
            report: None,
        });
    }
    // Anything the operator itself wants the caller told. Cleared first,
    // so a warning from an earlier call is not reported against this one.
    world.get_resource_or_init::<OperatorWarnings>().0.clear();
    world.get_resource_or_init::<OperatorReports>().0.clear();
    world
        .get_resource_or_init::<OperatorBooleanReport>()
        .request();
    crate::commands::SpawnedEntities::watch(world);
    let params = OperatorParameters(op.params.into_iter().collect());
    let run = |world: &mut World| {
        world
            .operator(id.clone())
            .settings(CallOperatorSettings {
                creates_history_entry: history,
                execution_context: ExecutionContext::Execute,
            })
            .params(params)
            .call()
    };
    // One call is one undo entry: some operators push their own commands and
    // also let the framework snapshot around them, and the span reconciles the
    // two shapes.
    let result = if history {
        with_history_span(world, id.clone(), run)
    } else {
        run(world)
    }
    .map_err(|err| match err {
        CallOperatorError::UnknownId(_) => invalid_params(format!("unknown operator: {id}")),
        other => internal_error(other.to_string()),
    })?;

    let mut warnings = unresolved;
    warnings.append(&mut world.get_resource_or_init::<OperatorWarnings>().0);
    let (removed, created) = world.get_resource_or_init::<OperatorReplacements>().take();
    let created: Vec<Entity> = created
        .into_iter()
        .filter(|&entity| world.get_entity(entity).is_ok())
        .collect();
    let bounds = created
        .iter()
        .map(|&entity| {
            summary::world_bounds(world, entity).map_or(Value::Null, |(min, max)| {
                json!([summary::vector(min), summary::vector(max)])
            })
        })
        .collect();

    Ok(CallOutcome {
        id,
        result: match result {
            OperatorResult::Finished => "finished",
            OperatorResult::Cancelled => "cancelled",
            OperatorResult::Running => "running",
        }
        .to_string(),
        warnings,
        entities: spawned_by_the_call(world)
            .into_iter()
            .map(Entity::to_bits)
            .collect(),
        reports: std::mem::take(&mut world.get_resource_or_init::<OperatorReports>().0),
        outcome: world
            .get_resource_or_init::<OperatorOutcome>()
            .take()
            .map(jackdaw_api_internal::operator::Outcome::as_str),
        removed: removed.into_iter().map(Entity::to_bits).collect(),
        created: created.into_iter().map(Entity::to_bits).collect(),
        bounds,
        report: world
            .get_resource_or_init::<OperatorBooleanReport>()
            .take()
            .as_ref()
            .map(boolean_report_json),
    })
}

fn boolean_report_json(report: &BooleanReport) -> Value {
    use summary::{round, vector};
    json!({
        "outcome": report.outcome.as_str(),
        "fragments": report.fragments,
        "volume": [round(report.volume_before), round(report.volume_after)],
        "bounds": report
            .bounds
            .map_or(Value::Null, |(min, max)| json!([vector(min), vector(max)])),
        "watertight": report.watertight,
        "cut_faces": report.cut_faces,
        "coplanar": report.coplanar,
        "degenerate": report.degenerate,
    })
}

/// The scene entities a call just added, as the call itself recorded them. An
/// operator that rebuilds the scene from the document mints new ids, so diffing
/// the roots afterwards would call every one of them new.
fn spawned_by_the_call(world: &mut World) -> Vec<Entity> {
    crate::commands::SpawnedEntities::take(world)
        .into_iter()
        .filter(|entity| world.get_entity(*entity).is_ok())
        .collect()
}

/// The nodes a read's `filter` reference matches, in outliner order. A
/// reference that matches nothing reads as no nodes; a malformed one is
/// refused.
pub fn filter_param(world: &mut World, params: &Value) -> Result<Option<Vec<Entity>>, BrpError> {
    let text = match params.get("filter") {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::String(text)) => text,
        Some(other) => {
            return Err(invalid_params(format!(
                "`filter` is an entity reference such as \"#roof\", not {other}"
            )));
        }
    };
    reference_matches(world, text).map(Some)
}

/// What a reference in a read matches: nothing is an answer, malformed text is
/// refused.
pub fn reference_matches(world: &mut World, text: &str) -> Result<Vec<Entity>, BrpError> {
    match crate::entity_refs::resolve(world, text) {
        Ok(found) => Ok(found),
        Err(crate::entity_refs::RefError::NoMatch) => Ok(Vec::new()),
        Err(err) => Err(invalid_params(err.describe(text))),
    }
}

/// Whether any ancestor of `entity` (not itself) is in `set`.
pub fn has_ancestor_in(world: &World, entity: Entity, set: &EntityHashSet) -> bool {
    let mut current = world.get::<ChildOf>(entity).map(ChildOf::parent);
    while let Some(at) = current {
        if set.contains(&at) {
            return true;
        }
        current = world.get::<ChildOf>(at).map(ChildOf::parent);
    }
    false
}

/// Whether `entity` is `ancestor` or sits somewhere below it.
pub fn is_at_or_under(world: &World, entity: Entity, ancestor: Entity) -> bool {
    let mut current = Some(entity);
    while let Some(at) = current {
        if at == ancestor {
            return true;
        }
        current = world.get::<ChildOf>(at).map(ChildOf::parent);
    }
    false
}

/// The `depth` a read asks for: generations below its root, all when absent.
pub fn depth_param(params: &Value) -> Result<u32, BrpError> {
    match params.get("depth") {
        None | Some(Value::Null) => Ok(u32::MAX),
        Some(given) => given
            .as_u64()
            .and_then(|depth| u32::try_from(depth).ok())
            .ok_or_else(|| {
                invalid_params(format!(
                    "`depth` is a non-negative whole number, not {given}"
                ))
            }),
    }
}

/// The entity a read's `root` (or `entity` / `name`) names, if any.
pub fn root_param(world: &mut World, params: &Value) -> Result<Option<Entity>, BrpError> {
    named_root(params)?
        .map(|root| entity_from_params(world, &root))
        .transpose()
}

/// The node a `root` field asks for, as an entity id or a name. `entity` and
/// `name` are read as well.
fn named_root(params: &Value) -> Result<Option<Value>, BrpError> {
    match params.get("root") {
        Some(Value::Number(bits)) => Ok(Some(json!({ "entity": bits }))),
        Some(Value::String(name)) => Ok(Some(json!({ "name": name }))),
        Some(Value::Null) | None => Ok(params
            .get("entity")
            .or_else(|| params.get("name"))
            .filter(|given| !given.is_null())
            .map(|_| params.clone())),
        Some(other) => Err(invalid_params(format!(
            "`root` is an entity id or a name, not {other}"
        ))),
    }
}

/// Whether the outliner would draw a root row for `entity`, asking the same
/// questions `crate::hierarchy::queue_root_row_spawn` does.
pub fn is_scene_node(world: &World, entity: Entity) -> bool {
    if is_editor_furniture(world, entity) {
        return false;
    }
    world.get::<Transform>(entity).is_some()
        || world
            .get::<jackdaw_scene_types::UiSceneRoot>(entity)
            .is_some()
}

/// The scene nodes at the top of the hierarchy, in the document's order as the
/// outliner lists them, then any the document does not hold, by id.
pub fn scene_roots(world: &mut World) -> Vec<Entity> {
    let mut query = world.query_filtered::<Entity, Without<ChildOf>>();
    let mut rest: Vec<Entity> = query
        .iter(world)
        .filter(|entity| is_scene_node(world, *entity))
        .collect();
    rest.sort();
    let mut roots: Vec<Entity> = world
        .get_resource::<jackdaw_bsn::SceneBsnAst>()
        .map(|ast| {
            ast.roots
                .iter()
                .filter_map(|&node| ast.ecs_for_ast(node))
                .collect()
        })
        .unwrap_or_default();
    roots.retain(|entity| rest.binary_search(entity).is_ok());
    let mut seen = EntityHashSet::default();
    roots.retain(|entity| seen.insert(*entity));
    rest.retain(|entity| !seen.contains(entity));
    roots.extend(rest);
    roots
}

/// Whether a node under a scene node belongs in the tree. A UI node carries
/// `UiTransform` rather than `Transform`, so under a scene root `Node` counts
/// too; at the top level it would take in the editor's own chrome, which is
/// `Node` all the way down and carries no scene marker.
pub fn is_scene_descendant(world: &World, entity: Entity) -> bool {
    if is_editor_furniture(world, entity) {
        return false;
    }
    is_scene_node(world, entity) || world.get::<Node>(entity).is_some()
}

/// Whether `entity` is in the open scene: a scene node, or a UI node under
/// one. The editor's own chrome is `Node` all the way up to a root that is no
/// scene node at all, so naming a piece of it reaches nothing.
pub fn belongs_to_the_scene(world: &World, entity: Entity) -> bool {
    let mut current = entity;
    for _ in 0..MAX_SCENE_DEPTH {
        if is_editor_furniture(world, current) {
            return false;
        }
        if is_scene_node(world, current) {
            return true;
        }
        if world.get::<Node>(current).is_none() {
            return false;
        }
        match world.get::<ChildOf>(current) {
            Some(parent) => current = parent.parent(),
            None => return false,
        }
    }
    false
}

/// How far up a UI hierarchy the walk looks for the scene root it hangs from.
const MAX_SCENE_DEPTH: usize = 64;

/// The editor's own entities, which no caller reading the scene asked for.
fn is_editor_furniture(world: &World, entity: Entity) -> bool {
    world.get::<crate::EditorEntity>(entity).is_some()
        || world
            .get::<jackdaw_scene_types::EditorHidden>(entity)
            .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &'static str, ty: &'static str) -> ParamSpec {
        ParamSpec {
            name,
            ty,
            default: None,
            doc: "",
        }
    }

    #[test]
    fn a_reference_names_a_call_and_a_path_into_its_outcome() {
        assert_eq!(
            parse_reference("$3.entities.0"),
            Ok(Some(ResultRef {
                call: 3,
                path: vec!["entities", "0"],
            }))
        );
        assert_eq!(parse_reference("plain"), Ok(None));
        assert_eq!(parse_reference("$name"), Ok(None), "a `$` word is text");
        for malformed in ["$1", "$1.", "$1..entities", "$1x.entities", "$1.colour"] {
            assert!(parse_reference(malformed).is_err(), "{malformed}");
        }
    }

    #[test]
    fn a_reference_must_name_an_earlier_call() {
        let params = json!({ "parent": "$0.entities.0", "list": ["$1.entities.0"] });
        assert!(check_references(Some(&params), 2).expect("both are earlier"));
        assert!(check_references(Some(&params), 1).is_err());
        assert!(!check_references(Some(&json!({ "name": "Cube" })), 0).expect("no references"));
    }

    #[test]
    fn a_reference_is_filled_from_the_outcome_it_names() {
        let earlier = vec![json!({ "id": "entity.add.group", "entities": [42, 43] })];
        let call = json!({ "id": "entity.add.cube", "params": { "parent": "$0.entities.1" } });
        let filled = fill_references(&call, &earlier).expect("the value is there");
        assert_eq!(filled["params"]["parent"], json!(43));

        let missing = json!({ "id": "x", "params": { "parent": "$0.entities.5" } });
        assert!(fill_references(&missing, &earlier).is_err());
    }

    #[test]
    fn a_doubled_dollar_is_text_that_starts_with_one() {
        assert_eq!(parse_reference("$$100 Prop"), Ok(None));
        assert!(parse_reference("$100 Prop").is_err());
        let call = json!({ "id": "x", "params": { "name": "$$1.50", "tags": ["$$x"] } });
        let filled = fill_references(&call, &[]).expect("no references");
        assert_eq!(filled["params"]["name"], json!("$1.50"));
        assert_eq!(filled["params"]["tags"][0], json!("$x"));
        assert!(!holds_reference(&call["params"]));
    }

    /// The declared type decides, not the JSON's.
    #[test]
    fn a_declared_type_beats_the_json_spelling() {
        let float = spec("radius", "Float");
        assert_eq!(
            property_from_json(Some(&float), &json!("5")),
            Some(PropertyValue::Float(5.0))
        );
        assert_eq!(
            property_from_json(Some(&float), &json!(5)),
            Some(PropertyValue::Float(5.0))
        );
        let text = spec("name", "String");
        assert_eq!(
            property_from_json(Some(&text), &json!(7)),
            Some(PropertyValue::String("7".into()))
        );
        let int = spec("seed", "Int");
        assert_eq!(
            property_from_json(Some(&int), &json!(7)),
            Some(PropertyValue::Int(7))
        );
        assert_eq!(property_from_json(Some(&int), &json!("nope")), None);
    }

    /// An undeclared parameter is read the way a `JACKDAW_RUN_OP` clause
    /// reads one, so the two text surfaces agree.
    #[test]
    fn an_undeclared_parameter_is_typed_by_its_spelling() {
        assert_eq!(
            property_from_json(None, &json!("7")),
            Some(PropertyValue::Int(7))
        );
        assert_eq!(
            property_from_json(None, &json!("true")),
            Some(PropertyValue::Bool(true))
        );
        assert_eq!(
            property_from_json(None, &json!("kit/fence.gltf")),
            Some(PropertyValue::String("kit/fence.gltf".into()))
        );
    }

    /// Vectors arrive as arrays from a JSON client and as comma lists
    /// from a shell, and both have to reach the same operator.
    #[test]
    fn a_vector_reads_as_an_array_or_as_a_comma_list() {
        let position = spec("position", "Vec3");
        assert_eq!(
            property_from_json(Some(&position), &json!([1.0, 2.0, 3.0])),
            Some(PropertyValue::Vec3(Vec3::new(1.0, 2.0, 3.0)))
        );
        assert_eq!(
            property_from_json(Some(&position), &json!("1,2,3")),
            Some(PropertyValue::Vec3(Vec3::new(1.0, 2.0, 3.0)))
        );
        assert_eq!(property_from_json(Some(&position), &json!("1,2")), None);
    }
}
