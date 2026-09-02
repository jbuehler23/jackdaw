//! `JACKDAW_RUN_OP`: run operators once at startup, with no mouse.
//!
//! Everything the editor does goes through an operator, and until now
//! every operator needed a click. That makes half the editor untestable
//! by anything but a human: a CI job cannot scatter a terrain, a bug
//! report cannot be replayed, and an agent capturing evidence has to
//! aim a synthetic pointer at a button.
//!
//! ```text
//! JACKDAW_RUN_OP="terrain.scatter density=1.2 seed=7"
//! JACKDAW_RUN_OP="terrain.scatter seed=1; viewport.screenshot path=/tmp/a.png"
//! ```
//!
//! Same family as [`crate::project::ENV_OPEN_PROJECT`],
//! [`crate::screenshot::ENV_SHOT`] and `JACKDAW_AUTO_OPEN`: read once at
//! startup, acted on after the editor settles. Unlike `JACKDAW_SHOT`
//! this one does **not** exit afterwards -- an operator run is usually
//! setup for something else (a screenshot, a manual look), not the whole
//! job.
//!
//! # Naming an entity
//!
//! An operator that acts on one thing in the scene declares an `Entity`
//! parameter, and a clause is text, so [`resolve_entity_params`] fills
//! those in: from `name=`, or from the selection for the operators that
//! act on it when a user runs them (see [`SELECTION_FALLBACK_OPS`]). A
//! `name=` that resolves also *selects* its target, so
//! `component.add name=Panel type_path=...` works from a cold start with
//! nothing selected.
//!
//! There is no quoting anywhere in a clause, so **a value cannot contain
//! a space**. The root a new UI scene seeds is named `UiRoot` for that
//! reason, so `name=UiRoot` reaches it; a node the user has renamed to
//! something with a space in it can only be addressed by leaving it
//! selected.

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::OperatorEntity;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::PropertyValue;

use crate::selection::Selection;

/// Names operators to run once the editor has settled.
pub const ENV_RUN_OP: &str = "JACKDAW_RUN_OP";

/// Frames to let pass after entering the editor before the first
/// operator runs. Matches [`crate::screenshot`]'s settle count: the
/// panels, the opened scene and the first rendered frame all have to
/// exist before an operator that reads the scene means anything.
const SETTLE_FRAMES: u32 = 90;

/// Frames between one clause and the next.
///
/// Not one per frame: a later clause routinely depends on the earlier
/// one having *finished*, not merely having run. An operator that queues
/// its real work through `Commands` needs a frame boundary, and one that
/// spawns glTF instances needs however long the asset server takes to
/// load and render them before a screenshot of the result means
/// anything. Same settle budget as the first clause, for the same
/// reason.
const GAP_FRAMES: u32 = SETTLE_FRAMES;

/// Overrides the default gap for the run.
///
/// A script that only drives the mouse and the keyboard waits a second
/// and a half between clauses for nothing: the gesture is already held to
/// the last beat, and no asset is loading. A
/// menu walk of thirty clauses is the difference between a run of seconds
/// and a run of a minute. The default is unchanged, so a script that
/// spawns scenes and screenshots them keeps the budget it was written
/// against.
pub const ENV_RUN_OP_GAP: &str = "JACKDAW_RUN_OP_GAP";

/// Longest gap the environment may ask for. A typo should cost a slow
/// run, not one that never reaches its second clause.
const MAX_GAP_FRAMES: u32 = 3600;

/// The gap a [`ENV_RUN_OP_GAP`] value names, or [`GAP_FRAMES`] when it
/// names nothing usable.
fn gap_frames(raw: Option<&str>) -> u32 {
    let Some(raw) = raw else {
        return GAP_FRAMES;
    };
    match raw.trim().parse::<u32>() {
        Ok(frames) => frames.min(MAX_GAP_FRAMES),
        Err(_) => {
            warn!("{ENV_RUN_OP_GAP}: {raw:?} is not a frame count; using {GAP_FRAMES}");
            GAP_FRAMES
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    if let Some(spec) = std::env::var_os(ENV_RUN_OP).and_then(|v| v.into_string().ok()) {
        let queue = parse_run_ops(&spec);
        if queue.is_empty() {
            warn!("{ENV_RUN_OP} is set but names no operator: {spec:?}");
        }
        app.insert_resource(BootOpQueue {
            queue,
            waiting: SETTLE_FRAMES,
            gap: gap_frames(
                std::env::var_os(ENV_RUN_OP_GAP)
                    .and_then(|value| value.into_string().ok())
                    .as_deref(),
            ),
        });
    }
    app.add_systems(
        Update,
        drive_boot_ops.run_if(in_state(crate::AppState::Editor)),
    );
}

/// One parsed `<id> [key=value ...]` clause.
#[derive(Clone, Debug, PartialEq)]
pub struct BootOp {
    pub id: String,
    pub params: Vec<(String, PropertyValue)>,
}

/// The pending startup runs. Absent when [`ENV_RUN_OP`] is unset, which
/// is every interactive launch.
#[derive(Resource)]
struct BootOpQueue {
    queue: Vec<BootOp>,
    /// Frames still to let pass before the next clause runs.
    waiting: u32,
    /// Frames to wait after a clause has run, from [`ENV_RUN_OP_GAP`].
    gap: u32,
}

/// Parse a `JACKDAW_RUN_OP` value.
///
/// Clauses are separated by `;`, tokens within a clause by whitespace,
/// and the first token is the operator id. A token containing `=` is a
/// parameter; anything else is ignored with a warning rather than
/// failing the boot, because a typo in an environment variable should
/// not cost a whole editor launch.
///
/// Values are typed by what they look like, not by a declaration:
/// `true`/`false` become `Bool`, an integer becomes `Int`, a decimal
/// becomes `Float`, everything else stays `String`. There is no quoting,
/// so a value cannot contain a space -- comma-separated lists (the shape
/// every list parameter in the editor already uses) work, sentences do
/// not.
pub fn parse_run_ops(spec: &str) -> Vec<BootOp> {
    let mut out = Vec::new();
    for clause in spec.split(';') {
        let mut tokens = clause.split_whitespace();
        let Some(id) = tokens.next() else {
            continue;
        };
        let mut params = Vec::new();
        for token in tokens {
            match token.split_once('=') {
                Some((key, value)) if !key.is_empty() => {
                    params.push((key.to_string(), parse_value(value)));
                }
                _ => warn!("{ENV_RUN_OP}: ignoring token {token:?}, expected key=value"),
            }
        }
        out.push(BootOp {
            id: id.to_string(),
            params,
        });
    }
    out
}

/// Type a parameter value by its spelling.
///
/// `i64` is tried before `f64` so `seed=7` arrives as an `Int` and
/// matches `OperatorParameters::as_int`; an operator that wants a float
/// there would read `7` as an int and miss it, which is why every
/// numeric operator parameter in the editor declares one type or the
/// other and callers write `1.0` when they mean a float.
fn parse_value(raw: &str) -> PropertyValue {
    match raw {
        "true" => return PropertyValue::Bool(true),
        "false" => return PropertyValue::Bool(false),
        _ => {}
    }
    if let Ok(int) = raw.parse::<i64>() {
        return PropertyValue::Int(int);
    }
    if let Ok(float) = raw.parse::<f64>() {
        return PropertyValue::Float(float);
    }
    PropertyValue::String(raw.to_string().into())
}

/// The one entity carrying `wanted`, or `None` when none does or more
/// than one does.
///
/// Ambiguity resolves to `None` rather than to one of the candidates.
pub(crate) fn unique_named_entity<'a>(
    named: impl Iterator<Item = (Entity, &'a Name)>,
    wanted: &str,
) -> Option<Entity> {
    let mut matches = named
        .filter(|(_, name)| name.as_str() == wanted)
        .map(|(entity, _)| entity);
    match (matches.next(), matches.next()) {
        (Some(entity), None) => Some(entity),
        _ => None,
    }
}

fn entity_named(world: &mut World, wanted: &str) -> Option<Entity> {
    let mut state = world.query_filtered::<(Entity, &Name), Without<crate::EditorEntity>>();
    unique_named_entity(state.iter(world), wanted)
}

/// The parameter schemas declared for `id`, across every registration
/// that answers to it. `scene.new` and `scene.open` are each declared
/// twice (see `tests/scene_op_ids.rs`), and only one of the two is
/// reachable by id, so both are read here rather than betting on which.
fn declared_params(world: &mut World, id: &str) -> Vec<&'static ParamSpec> {
    let mut state = world.query::<&OperatorEntity>();
    state
        .iter(world)
        .filter(|op| op.id() == id)
        .flat_map(|op| op.parameters().iter())
        .collect()
}

/// Operators that take the current selection when a clause names no
/// entity.
///
/// Membership follows what the operator does when a user runs it: these act
/// on the selection, and their availability gate `has_primary_selection`
/// refuses them when nothing is selected.
///
/// Everything else has to be told which entity it means, which is why this
/// is a list and not a blanket fallback. `prefab.apply_to_source`, for
/// instance, writes the prefab source document to disk, so a guessed target
/// would edit a file the author never pointed at.
///
/// One member's gate is not the selection: `hierarchy.rename_begin`'s is
/// `no_rename_in_progress`. It is listed because the operator itself resolves
/// a missing `entity` from the selection (`hierarchy::resolve_rename_target`,
/// what a bare F2 does), so filling it in here puts that resolution in the
/// log.
///
/// `widget.add` is deliberately absent. Its `parent` names the node that
/// *adopts* the widget, while a bare `widget.add` puts the widget beside the
/// selection instead (`ui_palette::instantiate_widget`). Filling `parent` in
/// from the selection would turn every clause into the adopting form, so a
/// run of three would build a chain rather than three siblings. It is in
/// [`OPTIONAL_ENTITY_PARAMS`] instead, because leaving it out is the other
/// form rather than an omission to warn about.
pub const SELECTION_FALLBACK_OPS: &[&str] = &[
    "animation.toggle_keyframe",
    "binding.add",
    "binding.set",
    "component.add",
    "component.remove",
    "component.revert_baseline",
    "field.set",
    "hierarchy.rename_begin",
    "physics.disable",
    "physics.enable",
];

/// `Entity` parameters that mean something by being left out, so a clause
/// without one is complete rather than short of a target.
///
/// The list is `(operator, parameter)`. Everything else declaring an
/// `Entity` needs one, from the clause or from the selection, and says so
/// when it gets neither.
pub const OPTIONAL_ENTITY_PARAMS: &[(&str, &str)] = &[("widget.add", "parent")];

/// How one declared `Entity` parameter was filled in.
///
/// Returned from [`resolve_entity_params`] rather than only logged, so a
/// caller can tell a resolver refusal from an availability gate that would
/// have refused anyway.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntityParam {
    /// The clause passed a real entity; nothing was resolved.
    Given { param: &'static str },
    /// A name resolved to one entity, which also becomes the selection.
    Named {
        param: &'static str,
        name: String,
        entity: Entity,
        entity_name: String,
    },
    /// The clause named nothing, so the selection filled the parameter in.
    FromSelection {
        param: &'static str,
        entity: Entity,
        entity_name: String,
    },
    /// The name the clause carried answers to no entity, or to two.
    NoSuchName { param: &'static str, name: String },
    /// The clause named nothing and this operator does not take the
    /// selection.
    NeedsAName { param: &'static str },
    /// The clause named nothing, and this parameter means something by
    /// being absent, so nothing was resolved and nothing was missing.
    LeftOut { param: &'static str },
    /// The clause named nothing, the operator does take the selection,
    /// and nothing is selected.
    NothingSelected { param: &'static str },
    /// The value given is neither an entity nor a name.
    NotAName { param: &'static str, value: String },
}

impl EntityParam {
    /// True when the parameter was left unresolved, so the operator will
    /// refuse rather than act.
    pub fn is_refusal(&self) -> bool {
        matches!(
            self,
            Self::NoSuchName { .. }
                | Self::NeedsAName { .. }
                | Self::NothingSelected { .. }
                | Self::NotAName { .. }
        )
    }

    /// The log line for this resolution. `None` for a parameter the clause
    /// spelled itself.
    pub fn line(&self, op: &str) -> Option<String> {
        Some(match self {
            Self::Given { .. } | Self::LeftOut { .. } => return None,
            Self::Named {
                param,
                name,
                entity,
                entity_name,
            } => format!("{op}: `{param}` = {entity} ({entity_name}), from name={name}"),
            Self::FromSelection {
                param,
                entity,
                entity_name,
            } => format!("{op}: `{param}` = {entity} ({entity_name}), from the selection"),
            Self::NoSuchName { param, name } => format!(
                "{op}: `{param}` was not set: `{name}` names no entity in this scene, or more \
                 than one"
            ),
            Self::NeedsAName { param } => format!(
                "{op}: `{param}` was not set: this operator does not take its target from the \
                 selection, so name it with `{param}=<Name>`"
            ),
            Self::NothingSelected { param } => format!(
                "{op}: `{param}` was not set: nothing is selected. Select a target first, or \
                 name it with `name=<Name>`."
            ),
            Self::NotAName { param, value } => {
                format!("{op}: `{param}` was not set: {value} is neither an entity nor a name")
            }
        })
    }
}

/// The `Name` an entity carries, for a log line that has to be readable
/// next to a bare entity index.
fn name_of(world: &World, entity: Entity) -> String {
    world
        .get::<Name>(entity)
        .map_or_else(|| "unnamed".to_string(), |name| name.as_str().to_string())
}

/// Fill in the `Entity` parameters a text harness cannot spell.
///
/// `JACKDAW_RUN_OP` carries text, and `PropertyValue::Entity` has no
/// spelling. Each declared `Entity` parameter is resolved here, once, before
/// dispatch: from the name the clause gives it, from a bare `name=`, or from
/// the current selection for the operators [`SELECTION_FALLBACK_OPS`] names.
///
/// A parameter resolved from a name also becomes the selection, when the
/// operator declares exactly one entity to act on, which is what makes
/// `component.add name=Panel type_path=...` work from a cold start. An
/// operator taking several entities gets no such thing: none of them is
/// "the" selection.
///
/// A pre-dispatch pass rather than a per-operator fallback, so operators
/// keep reading `OperatorParameters::as_entity`, which still refuses a
/// `PropertyValue::Int` (see `tests/operator_entity_params.rs`).
pub fn resolve_entity_params(world: &mut World, op: &mut BootOp) -> Vec<EntityParam> {
    let specs = declared_params(world, &op.id);
    let entity_params: Vec<&'static str> = specs
        .iter()
        .filter(|spec| spec.ty == "Entity")
        .map(|spec| spec.name)
        .collect();
    if entity_params.is_empty() {
        return Vec::new();
    }
    let sole_target = entity_params.len() == 1;
    let takes_selection = SELECTION_FALLBACK_OPS.contains(&op.id.as_str());

    // A bare `name=` is the entity's only when the operator has no `name`
    // parameter of its own; `selection.select` does, and it means that one.
    let bare_name = (!specs.iter().any(|spec| spec.name == "name"))
        .then(|| op.params.iter().find(|(key, _)| key == "name"))
        .flatten()
        .and_then(|(_, value)| match value {
            PropertyValue::String(name) => Some(name.to_string()),
            _ => None,
        });

    let mut outcomes = Vec::with_capacity(entity_params.len());
    for param in entity_params {
        let at = op.params.iter().position(|(key, _)| key == param);
        let wanted = match at.map(|index| &op.params[index].1) {
            Some(PropertyValue::Entity(_)) => {
                outcomes.push(EntityParam::Given { param });
                continue;
            }
            Some(PropertyValue::String(name)) => Some(name.to_string()),
            Some(other) => {
                outcomes.push(EntityParam::NotAName {
                    param,
                    value: other.to_string(),
                });
                continue;
            }
            None => bare_name.clone(),
        };

        let outcome = match wanted {
            Some(name) => match entity_named(world, &name) {
                Some(entity) => {
                    // Selecting the named entity satisfies the operator's own
                    // availability gate, as a click would.
                    if sole_target {
                        crate::selection::select_only(world, entity);
                    }
                    EntityParam::Named {
                        param,
                        name,
                        entity,
                        entity_name: name_of(world, entity),
                    }
                }
                None => EntityParam::NoSuchName { param, name },
            },
            None if OPTIONAL_ENTITY_PARAMS.contains(&(op.id.as_str(), param)) => {
                EntityParam::LeftOut { param }
            }
            None if !takes_selection => EntityParam::NeedsAName { param },
            None => match world
                .get_resource::<Selection>()
                .and_then(Selection::primary)
            {
                Some(entity) => EntityParam::FromSelection {
                    param,
                    entity,
                    entity_name: name_of(world, entity),
                },
                None => EntityParam::NothingSelected { param },
            },
        };

        let entity = match &outcome {
            EntityParam::Named { entity, .. } | EntityParam::FromSelection { entity, .. } => {
                Some(*entity)
            }
            _ => None,
        };
        if let Some(entity) = entity {
            match at {
                Some(index) => op.params[index].1 = PropertyValue::Entity(entity),
                None => op
                    .params
                    .push((param.to_string(), PropertyValue::Entity(entity))),
            }
        }
        outcomes.push(outcome);
    }

    for outcome in &outcomes {
        if let Some(line) = outcome.line(&op.id) {
            if outcome.is_refusal() {
                warn!("{ENV_RUN_OP}: {line}");
            } else {
                info!("{ENV_RUN_OP}: {line}");
            }
        }
    }
    outcomes
}

/// Resolve one clause's entity parameters and dispatch it.
///
/// The boot queue and the authoring tests both come through here, so a
/// scripted session and the harness run the same path.
pub fn run_boot_op(world: &mut World, op: &BootOp) -> Result<OperatorResult, CallOperatorError> {
    let mut op = op.clone();
    resolve_entity_params(world, &mut op);
    let id = op.id.clone();
    let mut call = world.operator(op.id);
    for (key, value) in op.params {
        call = call.param(key, value);
    }
    let result = call.call();
    // An operator refused by its availability gate reports `Cancelled` and
    // logs only at `debug!` (see `dispatch_operator`), so a scripted run
    // would otherwise say nothing about the clause not happening.
    if let Ok(OperatorResult::Cancelled) = &result {
        warn!("{ENV_RUN_OP}: {id} did not run: the operator refused or was unavailable");
    }
    result
}

/// Parse one `JACKDAW_RUN_OP` clause and run it.
pub fn run_op_clause(world: &mut World, clause: &str) -> Result<OperatorResult, CallOperatorError> {
    let Some(op) = parse_run_ops(clause).into_iter().next() else {
        return Err(CallOperatorError::UnknownId(clause.to_string().into()));
    };
    run_boot_op(world, &op)
}

/// Run one clause the way a toolbar button, a menu row or a keybind does.
///
/// The difference from [`run_op_clause`] is `creates_history_entry`. A
/// scripted clause is a chained call and opens no snapshot span; a press does.
/// An operator that records its own history entry and one that leaves the
/// entry to the snapshot look alike from a chained call and differ from a
/// press, so anything asserting how many entries a chord leaves behind has to
/// come through here.
pub fn run_op_clause_as_user(
    world: &mut World,
    clause: &str,
) -> Result<OperatorResult, CallOperatorError> {
    let Some(mut op) = parse_run_ops(clause).into_iter().next() else {
        return Err(CallOperatorError::UnknownId(clause.to_string().into()));
    };
    resolve_entity_params(world, &mut op);
    let mut call = world.operator(op.id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    for (key, value) in op.params {
        call = call.param(key, value);
    }
    call.call()
}

/// Count settle frames, then drain the queue one clause every gap.
fn drive_boot_ops(world: &mut World) {
    let Some(queue) = world.get_resource::<BootOpQueue>() else {
        return;
    };
    if queue.queue.is_empty() {
        return;
    }
    // A clause that drove the mouse or the keyboard is not finished when
    // its operator returned: the gesture is a list of beats spread over
    // frames (see `crate::test_input`). Holding the count still while
    // those play out keeps one clause meaning one gesture, however many
    // steps a drag was cut into.
    if !world
        .get_resource::<crate::test_input::SyntheticInput>()
        .is_none_or(crate::test_input::SyntheticInput::is_idle)
    {
        return;
    }
    {
        let mut queue = world.resource_mut::<BootOpQueue>();
        if queue.waiting > 0 {
            queue.waiting -= 1;
            return;
        }
        queue.waiting = queue.gap;
    }

    let op = world.resource_mut::<BootOpQueue>().queue.remove(0);
    match run_boot_op(world, &op) {
        Ok(result) => info!("{ENV_RUN_OP}: {} -> {result:?}", op.id),
        Err(err) => error!("{ENV_RUN_OP}: {} failed: {err}", op.id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn param(key: &str, value: PropertyValue) -> (String, PropertyValue) {
        (key.to_string(), value)
    }

    /// The gap is what makes a menu walk of thirty clauses bearable, so
    /// a script may name it; anything that is not a frame count leaves
    /// the default in place rather than stalling the run.
    #[test]
    fn the_environment_may_shorten_the_gap_between_clauses() {
        assert_eq!(gap_frames(None), GAP_FRAMES);
        assert_eq!(gap_frames(Some("6")), 6);
        assert_eq!(gap_frames(Some(" 12 ")), 12);
        assert_eq!(gap_frames(Some("0")), 0);
        assert_eq!(gap_frames(Some("999999")), MAX_GAP_FRAMES);
        assert_eq!(gap_frames(Some("soon")), GAP_FRAMES);
        assert_eq!(gap_frames(Some("-4")), GAP_FRAMES);
    }

    #[test]
    fn a_bare_operator_id_parses_with_no_parameters() {
        assert_eq!(
            parse_run_ops("terrain.scatter"),
            vec![BootOp {
                id: "terrain.scatter".to_string(),
                params: Vec::new(),
            }]
        );
    }

    #[test]
    fn parameter_values_are_typed_by_their_spelling() {
        let ops = parse_run_ops(
            "terrain.scatter seed=7 density=1.5 random_yaw=true align_to_normal=false \
             terrain=Ground accept=1,2",
        );
        assert_eq!(ops.len(), 1);
        assert_eq!(
            ops[0].params,
            vec![
                param("seed", PropertyValue::Int(7)),
                param("density", PropertyValue::Float(1.5)),
                param("random_yaw", PropertyValue::Bool(true)),
                param("align_to_normal", PropertyValue::Bool(false)),
                param("terrain", PropertyValue::String("Ground".into())),
                param("accept", PropertyValue::String("1,2".into())),
            ]
        );
    }

    #[test]
    fn semicolons_separate_a_sequence_of_runs() {
        let ops = parse_run_ops("terrain.scatter seed=3 ; viewport.screenshot path=/tmp/a.png");
        assert_eq!(ops.len(), 2);
        assert_eq!(ops[0].id, "terrain.scatter");
        assert_eq!(ops[0].params, vec![param("seed", PropertyValue::Int(3))]);
        assert_eq!(ops[1].id, "viewport.screenshot");
        assert_eq!(
            ops[1].params,
            vec![param("path", PropertyValue::String("/tmp/a.png".into()))]
        );
    }

    /// A malformed token is skipped rather than failing the whole boot.
    #[test]
    fn a_token_without_an_equals_is_dropped_and_the_rest_survives() {
        let ops = parse_run_ops("terrain.scatter nonsense seed=4");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].params, vec![param("seed", PropertyValue::Int(4))]);
    }

    #[test]
    fn an_empty_or_blank_spec_parses_to_nothing_runnable() {
        assert!(parse_run_ops("").is_empty());
        assert!(parse_run_ops("   ;  ").is_empty());
    }

    /// A negative number still types as an int, so `weight_channel=-1`
    /// reaches the operator as the "off" value it expects.
    #[test]
    fn negative_numbers_stay_numeric() {
        let ops = parse_run_ops("terrain.scatter weight_channel=-1 offset=-0.5");
        assert_eq!(
            ops[0].params,
            vec![
                param("weight_channel", PropertyValue::Int(-1)),
                param("offset", PropertyValue::Float(-0.5)),
            ]
        );
    }

    #[test]
    fn a_name_carried_by_one_entity_resolves_to_it() {
        let mut world = World::new();
        let wanted = world.spawn(Name::new("Target")).id();
        world.spawn(Name::new("Other"));
        let mut state = world.query::<(Entity, &Name)>();
        assert_eq!(
            unique_named_entity(state.iter(&world), "Target"),
            Some(wanted)
        );
    }

    /// A document can hold two nodes sharing a name, and neither is picked.
    #[test]
    fn a_name_two_entities_share_resolves_to_neither() {
        let mut world = World::new();
        world.spawn(Name::new("Row"));
        world.spawn(Name::new("Row"));
        let mut state = world.query::<(Entity, &Name)>();
        assert_eq!(unique_named_entity(state.iter(&world), "Row"), None);
        assert_eq!(unique_named_entity(state.iter(&world), "Missing"), None);
    }

    /// A resolved parameter's line names the operator, the parameter, the
    /// entity, its name, and where the answer came from.
    #[test]
    fn a_resolved_parameter_names_its_entity_and_its_source() {
        let mut world = World::new();
        let entity = world.spawn(Name::new("Panel")).id();

        let named = EntityParam::Named {
            param: "entity",
            name: "Panel".to_string(),
            entity,
            entity_name: name_of(&world, entity),
        };
        let line = named
            .line("component.add")
            .expect("a resolved parameter reports");
        assert!(!named.is_refusal());
        for expected in ["component.add", "`entity`", "Panel", "from name=Panel"] {
            assert!(
                line.contains(expected),
                "{expected:?} missing from {line:?}"
            );
        }
        assert!(
            line.contains(&entity.to_string()),
            "the entity id: {line:?}"
        );

        let from_selection = EntityParam::FromSelection {
            param: "entity",
            entity,
            entity_name: name_of(&world, entity),
        };
        let line = from_selection.line("physics.enable").expect("reports too");
        assert!(line.contains("from the selection"), "{line:?}");
    }

    /// An entity with no `Name` still produces a readable label.
    #[test]
    fn an_unnamed_entity_still_reads_as_something() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        assert_eq!(name_of(&world, entity), "unnamed");
    }

    /// A parameter the clause spelled itself produces no line; a refusal
    /// always does.
    #[test]
    fn only_the_parameters_the_resolver_touched_produce_a_line() {
        assert_eq!(
            EntityParam::Given { param: "entity" }.line("field.set"),
            None
        );
        for refusal in [
            EntityParam::NoSuchName {
                param: "entity",
                name: "Nope".to_string(),
            },
            EntityParam::NeedsAName {
                param: "instance_entity",
            },
            EntityParam::NothingSelected { param: "entity" },
            EntityParam::NotAName {
                param: "entity",
                value: "42".to_string(),
            },
        ] {
            assert!(refusal.is_refusal());
            assert!(
                refusal.line("prefab.apply_to_source").is_some(),
                "a refusal has to say why: {refusal:?}"
            );
        }
    }

    /// A path with an `=` in it keeps everything after the first one, so
    /// a query-string-ish value survives.
    #[test]
    fn only_the_first_equals_splits_a_parameter() {
        let ops = parse_run_ops("op.id key=a=b");
        assert_eq!(
            ops[0].params,
            vec![param("key", PropertyValue::String("a=b".into()))]
        );
    }
}
