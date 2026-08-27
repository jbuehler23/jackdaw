use bevy::ecs::change_detection::Tick;
use bevy::ecs::lifecycle::RemovedComponents;
use bevy::ecs::reflect::AppFunctionRegistry;
use bevy::ecs::system::SystemState;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bevy::reflect::func::{ArgList, DynamicFunction, FunctionRegistry, Return};
use bevy::ui::Checked;
use bevy::ui_widgets::SliderValue;

use crate::resolve::{
    BindValue, ResolvedSource, ResolvedWrite, WriteValue, is_self_cycle, read_resolved,
    resolve_context, resolve_source, resolve_write, write_resolved,
};
use crate::{BindContext, BindError, BindFailures, BindPath, Binding, Bindings, ValueTextTarget};

/// Functions register under their full path (`my_game::ui::ratio`), so a
/// binding may name either that or the trailing segment alone. A short name
/// shared by several registered functions is an error, not a coin flip.
fn lookup_function<'a>(
    registry: &'a FunctionRegistry,
    name: &str,
) -> Result<&'a DynamicFunction<'static>, BindError> {
    if let Some(function) = registry.get(name) {
        return Ok(function);
    }
    let mut matches: Vec<&DynamicFunction<'static>> = registry
        .iter()
        .filter(|f| {
            f.name()
                .is_some_and(|n| n.rsplit("::").next() == Some(name))
        })
        .collect();
    match matches.len() {
        0 => Err(BindError::UnknownFunction {
            name: name.to_string(),
        }),
        1 => Ok(matches.remove(0)),
        _ => {
            let mut paths: Vec<&str> = matches
                .iter()
                .filter_map(|f| f.name().map(AsRef::as_ref))
                .collect();
            paths.sort_unstable();
            Err(BindError::AmbiguousFunction {
                name: name.to_string(),
                candidates: paths.into_iter().map(str::to_string).collect(),
            })
        }
    }
}

/// A `via` function with its lookup already done. Resolving a short name walks
/// every registered function, and the answer does not change between frames.
pub(crate) struct ResolvedVia {
    /// The name the binding gave, for the messages a failed call raises.
    name: String,
    /// The function itself, cloned out of the registry once. A
    /// `DynamicFunction` is a handle, so keeping one is cheap.
    function: DynamicFunction<'static>,
}

/// Looks a `via` function up once, when the binding it belongs to is resolved.
/// A name that answers to nothing fails here rather than on every frame.
fn resolve_via(world: &World, name: &str) -> Result<ResolvedVia, BindError> {
    let registry_arc = world
        .get_resource::<AppFunctionRegistry>()
        .ok_or(BindError::NoFunctionRegistry)?
        .clone();
    let registry = registry_arc.read();
    Ok(ResolvedVia {
        name: name.to_string(),
        function: lookup_function(&registry, name)?.clone(),
    })
}

/// Runs a binding's reads through a registered function and takes the result.
/// The function has to hand back a value the binding can keep: a borrow does
/// not outlive the call.
pub fn apply_via(world: &World, name: &str, args: Vec<BindValue>) -> Result<BindValue, BindError> {
    call_via(&resolve_via(world, name)?, args)
}

/// The call itself, once the function has been found.
pub(crate) fn call_via(via: &ResolvedVia, args: Vec<BindValue>) -> Result<BindValue, BindError> {
    let ResolvedVia { name, function } = via;
    let mut list = ArgList::new();
    for arg in args {
        match arg {
            BindValue::F32(v) => list = list.with_owned(v),
            BindValue::Bool(v) => list = list.with_owned(v),
            BindValue::Str(v) => list = list.with_owned(v),
        }
    }
    let ret = function.call(list).map_err(|e| BindError::FunctionCall {
        name: name.clone(),
        message: format!("{e:?}"),
    })?;
    let Return::Owned(owned) = ret else {
        return Err(BindError::NonOwnedReturn { name: name.clone() });
    };
    crate::resolve::extract(owned.as_partial_reflect(), name)
}

/// Warn once per (widget, binding index). Shared with the action observer,
/// which fails on every click of the same broken button.
pub(crate) fn report(failures: &mut BindFailures, entity: Entity, index: usize, err: BindError) {
    if failures.0.insert((entity, index)) {
        warn!("binding {index} on {entity}: {err}");
    }
}

/// Takes a binding out of the warn-once ledger. The entry records that this
/// binding's failure has already been logged, so dropping it once the binding
/// succeeds lets a later failure be logged again.
fn clear_failure(world: &mut World, entity: Entity, index: usize) {
    if let Some(mut failures) = world.get_resource_mut::<BindFailures>() {
        failures.0.remove(&(entity, index));
    }
}

/// How many evaluator runs pass before a widget whose bindings failed is
/// looked up again. Re-resolving clones the authored list and takes the type
/// registry lock, and what a failed lookup waits for (a type registered, a
/// resource inserted, a target named) does not arrive any sooner for being
/// asked about more often. Roughly twice a second at 60fps.
const RESOLVE_RETRY_RUNS: u32 = 30;

/// What a binding does with the values it read, and where the result lands,
/// worked out once instead of every frame.
pub(crate) enum ResolvedTarget {
    /// Drives a field of one of the widget's own components.
    Field {
        /// The registered function the reads pass through, if the binding
        /// names one, looked up once.
        via: Option<ResolvedVia>,
        /// Whether the result is a fraction, written as a percentage.
        as_percent: bool,
        /// The widget field the result lands in.
        write: ResolvedWrite,
    },
    /// Fills in the widget's own `Text`.
    Text {
        /// The sentence, with `{}` where each read goes.
        format: String,
    },
    /// Shows or hides the widget.
    Visible {
        /// The registered function the read passes through, if any, looked up
        /// once.
        via: Option<ResolvedVia>,
    },
    /// Keeps the widget's slider, checkbox or text in step with its source.
    Value {
        /// The widget's text field, looked up once from
        /// [`crate::ValueTextTarget`]. `None` when nothing
        /// named one, or when what it named is not a write this world can
        /// make.
        text: Option<ResolvedWrite>,
    },
    /// Nothing to do each frame: an action sends its event from an observer
    /// when the widget is activated.
    Action,
    /// The binding's paths could not be looked up. The reason is reported
    /// every frame and the lookup is tried again, so a binding that names a
    /// type registered later starts working on its own.
    Unresolved(BindError),
}

/// One authored binding with its lookups already done.
pub(crate) struct ResolvedBinding {
    /// Where this binding sits in the authored list, which is how a failure is
    /// reported back to the row the author wrote.
    pub(crate) binding_index: usize,
    /// The reads, in authored order.
    pub(crate) sources: Vec<ResolvedSource>,
    /// What the binding does with them.
    pub(crate) target: ResolvedTarget,
    /// When the lookup was done. A binding resolved since the evaluator last
    /// ran is evaluated whether or not its sources have moved. A binding that
    /// keeps failing is looked up again on the retry cadence, so its whole
    /// widget carries a fresh stamp each time that comes round.
    pub(crate) resolved_at: Tick,
    /// Whether the last attempt to evaluate this binding failed. A failing
    /// binding is due every frame: nothing about its sources will move to
    /// bring it back, and what it is waiting for, such as a widget component
    /// hydrated a frame late, costs a read to notice.
    pub(crate) failing: bool,
}

/// The lookups behind a widget's [`Bindings`], kept beside them.
///
/// This is derived state, not authored state: the resolver maintains it as the
/// scene changes and nothing else writes it. It holds entity ids, component ids
/// and reflect handles, none of which mean anything outside the world that
/// produced them, so it is not a reflected type and a scene writer, which works
/// from the type registry, cannot see it.
#[derive(Component)]
pub struct ResolvedBindings(Vec<ResolvedBinding>);

impl ResolvedBindings {
    /// How many of the widget's bindings have been looked up.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the widget's binding list resolved to nothing at all.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// How many bindings the evaluator has read sources for since the app started.
///
/// A frame in which no source moved leaves this standing still, which is what
/// makes the change-tick gate observable from outside the crate.
#[derive(Resource, Default)]
pub struct BindReads(
    /// The running count.
    pub u64,
);

/// Reads every binding's sources and writes its target only when the value it
/// computed differs from the value already there, so a widget nothing moved
/// stays clean for the rest of the frame. A binding that fails warns once and
/// the others carry on.
///
/// Before any of that it brings the lookups up to date: this is the only place
/// [`ResolvedBindings`] is built, so an app that registers the evaluator alone,
/// as the editor does behind its preview toggle, needs nothing else.
///
/// A binding whose sources have not moved since the last run is skipped: it
/// would have computed the value already stored and written nothing either way,
/// so an idle frame touches no game state. `Value` bindings are the exception
/// and run every frame, because a click that moves the widget without moving
/// the value has to be put back.
///
/// What is due is decided before anything is written, so two bindings in a
/// chain, one reading what the other writes, advance a link per frame.
pub fn evaluate_bindings(
    world: &mut World,
    mut dirty: Local<SystemState<DirtyBindings<'static, 'static>>>,
    mut targets: Local<SystemState<Query<'static, 'static, (Entity, &'static ResolvedBindings)>>>,
    mut last_run: Local<Tick>,
    mut retry: Local<HashSet<Entity>>,
    mut runs_since_retry: Local<u32>,
) {
    *runs_since_retry += 1;
    let retrying: Vec<Entity> = if *runs_since_retry >= RESOLVE_RETRY_RUNS {
        *runs_since_retry = 0;
        retry.drain().collect()
    } else {
        Vec::new()
    };
    refresh_lookups(world, &mut dirty, retrying);
    let this_run = world.change_tick();
    // Tick zero is what a `Local` holds before the system has ever run, so the
    // first pass evaluates everything rather than comparing against a tick
    // nothing in the world has reached. The same branch covers a tick that has
    // saturated: everything is due, which is the safe answer.
    let first_run = *last_run == Tick::new(0);
    let mut work: Vec<(Entity, Vec<usize>)> = Vec::new();
    {
        let Ok(bound) = targets.get(world) else {
            return;
        };
        for (entity, resolved) in bound.iter() {
            let due: Vec<usize> = resolved
                .0
                .iter()
                .enumerate()
                .filter(|(_, binding)| is_due(world, binding, *last_run, this_run, first_run))
                .map(|(index, _)| index)
                .collect();
            if !due.is_empty() {
                work.push((entity, due));
            }
        }
    }
    *last_run = this_run;
    // The writes below must land on a later tick than the gate just read, or a
    // binding reading what another one writes would never see it move.
    world.increment_change_tick();
    for (entity, due) in work {
        let Some(mut held) = world.get_mut::<ResolvedBindings>(entity) else {
            continue;
        };
        // Evaluation needs the whole world, so the list moves out for the
        // duration and back afterwards.
        let mut list = std::mem::take(&mut held.bypass_change_detection().0);
        for index in due {
            let Some(binding) = list.get(index) else {
                continue;
            };
            let binding_index = binding.binding_index;
            if matches!(binding.target, ResolvedTarget::Unresolved(_)) {
                retry.insert(entity);
            } else if let Some(mut reads) = world.get_resource_mut::<BindReads>() {
                reads.0 += 1;
            }
            let outcome = evaluate_resolved(world, entity, binding);
            let was_failing = binding.failing;
            let failed = outcome.is_err();
            match outcome {
                // A binding that succeeds leaves the ledger, so a later failure
                // is warned about again.
                Ok(()) => {
                    if was_failing {
                        clear_failure(world, entity, binding_index);
                    }
                }
                Err(err) => {
                    // The failure may be about the world rather than the
                    // binding, such as a widget whose target component has not
                    // arrived yet. Nothing about its sources will move to bring
                    // it back, so the lookup is what has to happen again.
                    retry.insert(entity);
                    if let Some(mut failures) = world.get_resource_mut::<BindFailures>() {
                        report(&mut failures, entity, binding_index, err);
                    }
                }
            }
            if let Some(binding) = list.get_mut(index) {
                binding.failing = failed;
            }
        }
        if let Some(mut held) = world.get_mut::<ResolvedBindings>(entity)
            && held.0.is_empty()
        {
            held.bypass_change_detection().0 = list;
        }
    }
}

/// Whether a binding has anything to do this frame: it was looked up since the
/// evaluator last ran, or one of the values it reads has moved.
///
/// Three kinds answer without asking. An action does nothing here, since it
/// sends its event from an observer. A `Value` binding keeps the widget
/// following the value, so skipping it would leave a click that moved the
/// widget and not the value standing; a checkbox's state is a component being
/// there or not, which no tick reports either way. `Text` is exempt for the
/// same reason: a keystroke moves the box without moving the source, so a gated
/// binding would leave the two disagreeing until something else touched the
/// source. The write is equality-guarded either way, so a widget already
/// showing its value is left alone. A binding that failed is due for the same
/// reason its widget is looked up again: nothing about its sources will move to
/// bring it back.
fn is_due(
    world: &World,
    binding: &ResolvedBinding,
    last_run: Tick,
    this_run: Tick,
    first_run: bool,
) -> bool {
    match &binding.target {
        ResolvedTarget::Action => false,
        ResolvedTarget::Value { .. }
        | ResolvedTarget::Text { .. }
        | ResolvedTarget::Unresolved(_) => true,
        _ => {
            first_run
                || binding.failing
                || binding.resolved_at.is_newer_than(last_run, this_run)
                || binding
                    .sources
                    .iter()
                    .any(|source| source.changed(world, last_run, this_run))
        }
    }
}

/// Every world change that can make a lookup stale. Insertions and edits show
/// up as `Changed`; a component going away does not, so removals are read from
/// their own queues.
type DirtyBindings<'w, 's> = (
    Query<'w, 's, Entity, Changed<Bindings>>,
    Query<
        'w,
        's,
        Entity,
        (
            Or<(Changed<ChildOf>, Changed<BindContext>)>,
            Or<(With<Bindings>, With<Children>)>,
        ),
    >,
    Query<'w, 's, Entity, (With<ResolvedBindings>, Without<Bindings>)>,
    RemovedComponents<'w, 's, ChildOf>,
    RemovedComponents<'w, 's, BindContext>,
);

/// Whether removals went by that this reader never saw.
///
/// A host may park the evaluator; the editor runs it only while preview is on.
/// Bevy clears these queues every frame, so a removal that happened while it
/// was parked is gone before it runs again, and the cursor is the only thing
/// that still knows: it is left behind the oldest message still held.
fn missed_removals<T: Component>(removals: &RemovedComponents<'_, '_, T>) -> bool {
    removals
        .messages()
        .is_some_and(|messages| removals.reader().missed_messages(messages) > 0)
}

fn refresh_lookups(
    world: &mut World,
    dirty: &mut SystemState<DirtyBindings<'static, 'static>>,
    retry: Vec<Entity>,
) {
    let (mut entities, subtrees, mut stale, missed) = {
        let Ok((changed, moved, orphaned, mut unparented, mut uncontexted)) = dirty.get_mut(world)
        else {
            return;
        };
        // Asked before the reads below, which move the cursor forward.
        let missed = missed_removals(&unparented) || missed_removals(&uncontexted);
        let mut subtrees = moved.iter().collect::<Vec<_>>();
        subtrees.extend(unparented.read());
        subtrees.extend(uncontexted.read());
        (
            changed.iter().collect::<Vec<_>>(),
            subtrees,
            orphaned.iter().collect::<Vec<_>>(),
            missed,
        )
    };
    entities.extend(retry);
    for entity in stale.drain(..) {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<ResolvedBindings>();
        }
    }
    // A removal nobody read may have been the one that moved a widget out of
    // its context, and the widget itself is no longer anywhere a walk from the
    // roots that changed would reach it. Which one it was cannot be recovered,
    // so every lookup is made again, once, on the frame the gap is noticed.
    if missed {
        let mut bound = world.query_filtered::<Entity, With<ResolvedBindings>>();
        entities.extend(bound.iter(world).collect::<Vec<_>>());
    }
    if entities.is_empty() && subtrees.is_empty() {
        return;
    }
    let mut visited: HashSet<Entity> = HashSet::new();
    for root in subtrees {
        let mut stack = vec![root];
        while let Some(entity) = stack.pop() {
            if !visited.insert(entity) {
                continue;
            }
            if let Some(children) = world.get::<Children>(entity) {
                stack.extend(children.iter());
            }
            // A reparented subtree is mostly entities with no bindings at all.
            // Walking through them is the point; looking them up is not.
            if is_bound(world, entity) {
                resolve_entity(world, entity);
            }
        }
    }
    for entity in entities {
        if visited.insert(entity) {
            resolve_entity(world, entity);
        }
    }
}

/// Whether an entity is one the resolver has anything to say about: it carries
/// bindings, or it carries lookups that may be stale.
fn is_bound(world: &World, entity: Entity) -> bool {
    world
        .get_entity(entity)
        .is_ok_and(|e| e.contains::<Bindings>() || e.contains::<ResolvedBindings>())
}

/// Looks a widget's bindings up again, or drops the lookups if it has none
/// left. Every path into the resolver ends here.
fn resolve_entity(world: &mut World, entity: Entity) {
    let Some(bindings) = world.get::<Bindings>(entity) else {
        if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
            entity_mut.remove::<ResolvedBindings>();
        }
        return;
    };
    let bindings = bindings.0.clone();
    let context = resolve_context(world, entity);
    let at = world.change_tick();
    let resolved: Vec<ResolvedBinding> = bindings
        .iter()
        .enumerate()
        .map(|(index, binding)| {
            let (sources, target) = match resolve_one(world, entity, context, binding) {
                Ok(pair) => pair,
                Err(err) => (Vec::new(), ResolvedTarget::Unresolved(err)),
            };
            ResolvedBinding {
                binding_index: index,
                sources,
                target,
                resolved_at: at,
                failing: false,
            }
        })
        .collect();
    // A lookup that succeeds clears the failure the log carries for it, and a
    // binding no longer on the list takes its entry with it.
    if let Some(mut failures) = world.get_resource_mut::<BindFailures>() {
        failures.0.retain(|(failed, index)| {
            *failed != entity
                || resolved
                    .get(*index)
                    .is_some_and(|b| matches!(b.target, ResolvedTarget::Unresolved(_)))
        });
    }
    if let Ok(mut entity_mut) = world.get_entity_mut(entity) {
        entity_mut.insert(ResolvedBindings(resolved));
    }
}

/// Looks one binding up. The widget is passed as well as its context because a
/// binding that reads exactly what it writes is refused here: see
/// [`crate::BindError::SelfCycle`].
fn resolve_one(
    world: &mut World,
    entity: Entity,
    context: Option<Entity>,
    binding: &Binding,
) -> Result<(Vec<ResolvedSource>, ResolvedTarget), BindError> {
    let reads: &[BindPath] = match binding {
        Binding::Field { read, .. } => read,
        Binding::Text { args, .. } => args,
        Binding::Visible { read, .. } => std::slice::from_ref(read),
        Binding::Value { with, .. } => std::slice::from_ref(with),
        Binding::Action { .. } => &[],
    };
    let sources = reads
        .iter()
        .map(|path| resolve_source(world, context, path))
        .collect::<Result<Vec<_>, _>>()?;
    let target = match binding {
        Binding::Field {
            via,
            write,
            as_percent,
            ..
        } => {
            let resolved = resolve_write(world, write)?;
            if sources
                .iter()
                .any(|source| is_self_cycle(entity, source, &resolved))
            {
                return Err(BindError::SelfCycle {
                    path: write.raw.clone(),
                });
            }
            ResolvedTarget::Field {
                via: resolve_maybe_via(world, via.as_deref())?,
                as_percent: *as_percent,
                write: resolved,
            }
        }
        Binding::Text { format, .. } => ResolvedTarget::Text {
            format: format.clone(),
        },
        Binding::Visible { via, .. } => ResolvedTarget::Visible {
            via: resolve_maybe_via(world, via.as_deref())?,
        },
        // A failed lookup is not the binding's failure: most `Value` bindings
        // drive a slider or a checkbox and never look at this. A string one
        // finds it missing and says so.
        Binding::Value { .. } => ResolvedTarget::Value {
            text: resolve_text_target(world),
        },
        Binding::Action { event, fields } => {
            crate::actions::check_event(world, event, fields)?;
            ResolvedTarget::Action
        }
    };
    Ok((sources, target))
}

fn resolve_maybe_via(world: &World, name: Option<&str>) -> Result<Option<ResolvedVia>, BindError> {
    name.map(|name| resolve_via(world, name)).transpose()
}

/// Where a widget's text lives, if anything has said. Answered once per
/// resolve, so a target installed after a binding was looked up is picked up
/// the next time that binding is, which the write failing in the meantime asks
/// for on its own.
fn resolve_text_target(world: &mut World) -> Option<ResolvedWrite> {
    let path = world.get_resource::<ValueTextTarget>()?.0.clone();
    resolve_write(world, &path).ok()
}

fn read_all(world: &World, sources: &[ResolvedSource]) -> Result<Vec<BindValue>, BindError> {
    sources.iter().map(|s| read_resolved(world, s)).collect()
}

/// Substitutes `{}` placeholders in order. Whole floats lose their fraction
/// (`87`, not `87.0`); the rest round to one decimal. Surplus values are
/// ignored, but a placeholder with no value left is an error.
fn render(format: &str, values: &[BindValue]) -> Result<String, BindError> {
    let mut out = String::with_capacity(format.len() + 8);
    let mut pieces = format.split("{}");
    if let Some(first) = pieces.next() {
        out.push_str(first);
    }
    let mut values = values.iter();
    for piece in pieces {
        match values.next() {
            Some(BindValue::F32(v)) if v.fract() == 0.0 => out.push_str(&(*v as i64).to_string()),
            Some(BindValue::F32(v)) => out.push_str(&format!("{v:.1}")),
            Some(BindValue::Bool(v)) => out.push_str(if *v { "true" } else { "false" }),
            Some(BindValue::Str(v)) => out.push_str(v),
            None => return Err(BindError::TooManyPlaceholders),
        }
        out.push_str(piece);
    }
    Ok(out)
}

fn evaluate_resolved(
    world: &mut World,
    entity: Entity,
    resolved: &ResolvedBinding,
) -> Result<(), BindError> {
    match &resolved.target {
        ResolvedTarget::Unresolved(err) => Err(err.clone()),
        ResolvedTarget::Action => Ok(()),
        ResolvedTarget::Field {
            via,
            as_percent,
            write,
        } => {
            let mut values = read_all(world, &resolved.sources)?;
            let value = match via {
                Some(via) => call_via(via, values)?,
                // Silently keeping the last read would make the others look
                // wired up when nothing consumes them.
                None if values.len() > 1 => {
                    return Err(BindError::MultipleReadsNoVia {
                        count: values.len(),
                    });
                }
                None => values.pop().ok_or(BindError::NoReads)?,
            };
            let out = match (value, *as_percent) {
                (BindValue::F32(v), true) => WriteValue::Percent(v),
                (BindValue::F32(v), false) => WriteValue::F32(v),
                (BindValue::Bool(v), _) => WriteValue::Bool(v),
                (BindValue::Str(v), _) => WriteValue::Str(v),
            };
            write_resolved(world, entity, write, &out).map(|_| ())
        }
        ResolvedTarget::Visible { via } => {
            let mut values = read_all(world, &resolved.sources)?;
            let value = values.pop().ok_or(BindError::NoReads)?;
            let value = match via {
                Some(via) => call_via(via, vec![value])?,
                None => value,
            };
            let BindValue::Bool(v) = value else {
                return Err(BindError::VisibleNotBool);
            };
            let mut visibility =
                world
                    .get_mut::<Visibility>(entity)
                    .ok_or(BindError::MissingWidgetComponent {
                        entity,
                        component: "Visibility",
                    })?;
            let new = if v {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
            if *visibility != new {
                *visibility = new;
            }
            Ok(())
        }
        ResolvedTarget::Text { format } => {
            let values = read_all(world, &resolved.sources)?;
            let text = render(format, &values)?;
            let Some(mut target) = world.get_mut::<Text>(entity) else {
                return Err(BindError::MissingWidgetComponent {
                    entity,
                    component: "Text",
                });
            };
            if target.0 != text {
                target.0 = text;
            }
            Ok(())
        }
        ResolvedTarget::Value { text } => {
            let mut values = read_all(world, &resolved.sources)?;
            let value = values.pop().ok_or(BindError::NoReads)?;
            // Which widget component takes the value is decided by the value's
            // own shape, so a text widget has to be recognised before that
            // guess is made: otherwise a number aimed at a name field reports
            // a missing slider.
            let text = text.as_ref().filter(|write| {
                world
                    .get_entity(entity)
                    .is_ok_and(|widget| widget.contains_id(write.component))
            });
            match (value, text) {
                (BindValue::Str(v), Some(write)) => {
                    write_resolved(world, entity, write, &WriteValue::Str(v)).map(|_| ())
                }
                (BindValue::Str(_), None) => Err(BindError::StringValueNoTarget),
                (BindValue::F32(_), Some(_)) => Err(BindError::ValueNotAString { kind: "number" }),
                (BindValue::Bool(_), Some(_)) => Err(BindError::ValueNotAString { kind: "bool" }),
                // SliderValue is immutable, so a sync means re-inserting it.
                // The equality guard keeps an unchanged binding from doing that
                // every frame.
                (BindValue::F32(v), None) => {
                    let current = world
                        .get::<SliderValue>(entity)
                        .ok_or(BindError::MissingWidgetComponent {
                            entity,
                            component: "SliderValue",
                        })?
                        .0;
                    if current != v {
                        world.entity_mut(entity).insert(SliderValue(v));
                    }
                    Ok(())
                }
                (BindValue::Bool(v), None) => {
                    let has = world.get::<Checked>(entity).is_some();
                    if v && !has {
                        world.entity_mut(entity).insert(Checked);
                    } else if !v && has {
                        world.entity_mut(entity).remove::<Checked>();
                    }
                    Ok(())
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_whole_floats_without_a_fraction() {
        let values = [BindValue::F32(87.0), BindValue::F32(2.5)];
        assert_eq!(render("{} / {}", &values).unwrap(), "87 / 2.5");
    }

    #[test]
    fn renders_bools_and_strings() {
        let values = [BindValue::Bool(true), BindValue::Str("ok".into())];
        assert_eq!(render("[{}] {}", &values).unwrap(), "[true] ok");
    }

    #[test]
    fn more_placeholders_than_args_is_an_error() {
        let err = render("{} / {}", &[BindValue::F32(1.0)]).unwrap_err();
        assert_eq!(err, BindError::TooManyPlaceholders);
        assert_eq!(err.to_string(), "more {} placeholders than args");
    }
}
