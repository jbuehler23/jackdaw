//! Operators for everything the Graph window offers.
//!
//! Every control in the window dispatches one of these, so a graph can be
//! authored from a script, from the remote or from the canvas by the same
//! path.

use bevy::prelude::*;
use jackdaw_animation_runtime::{
    AnimationCondition, AnimationGraphRef, AnimationGraphState, AnimationMotion,
    AnimationParameterDef, AnimationParameterKind, AnimationParams, AnimationTransitionDef,
};
use jackdaw_api::prelude::*;

use super::graph_doc::{
    AnimationGraphDoc, commit_graph_edit, condition_op_from_name, graph_path_for_name, new_graph,
    open_graph, parameter, parameter_kind_from_name, parse_clip_ref, write_graph_file,
};
use super::graph_window::graph_preview_target;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<AnimationGraphNewOp>()
        .register_operator::<AnimationGraphOpenOp>()
        .register_operator::<AnimationGraphSaveOp>()
        .register_operator::<AnimationGraphAddStateOp>()
        .register_operator::<AnimationGraphSetStateOp>()
        .register_operator::<AnimationGraphRemoveStateOp>()
        .register_operator::<AnimationGraphAddTransitionOp>()
        .register_operator::<AnimationGraphRemoveTransitionOp>()
        .register_operator::<AnimationGraphSetEntryOp>()
        .register_operator::<AnimationGraphAddParamOp>()
        .register_operator::<AnimationGraphSetParamOp>()
        .register_operator::<AnimationGraphConvertSetOp>()
        .register_operator::<AnimationGraphAssignOp>();
}

/// Start a graph of this name and open it on the canvas.
#[operator(
    id = "animation.graph.new",
    label = "New Animation Graph",
    description = "Start an empty animation graph in assets/animation and open it in the Graph \
                   window.",
    allows_undo = false,
    params(name(String, doc = "What the graph is called, which is also its file stem."))
)]
pub(crate) fn animation_graph_new(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let named = params
        .as_str("name")
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    commands.queue(move |world: &mut World| {
        let name = named.clone().unwrap_or_else(|| {
            let typed = world
                .resource::<super::AnimationPanelState>()
                .graph_name
                .clone();
            if typed.is_empty() {
                "graph".to_string()
            } else {
                typed
            }
        });
        if new_graph(world, &name).is_none() {
            warn!("animation.graph.new: {name} could not be written");
            return;
        }
        crate::open_window_in_default_area_if_absent(world, super::graph_window::GRAPH_WINDOW_ID);
    });
    OperatorResult::Finished
}

/// Open a graph file on the canvas.
#[operator(
    id = "animation.graph.open",
    label = "Open Animation Graph",
    description = "Show a graph file the project holds in the Graph window.",
    allows_undo = false,
    params(path(String, doc = "Assets-relative path of the graph file to open."))
)]
pub(crate) fn animation_graph_open(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let path = params.as_str("path").map(str::to_string)?;
    commands.queue(move |world: &mut World| {
        if !open_graph(world, &path) {
            return;
        }
        crate::open_window_in_default_area_if_absent(world, super::graph_window::GRAPH_WINDOW_ID);
    });
    OperatorResult::Finished
}

/// Write the open graph back to its file.
#[operator(
    id = "animation.graph.save",
    label = "Save Animation Graph",
    description = "Write the open graph back to the file it came from.",
    is_available = a_graph_is_open,
    allows_undo = false
)]
pub(crate) fn animation_graph_save(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(|world: &mut World| {
        if let Some(file) = write_graph_file(world) {
            info!("Wrote {}", file.display());
        }
    });
    OperatorResult::Finished
}

/// Put a state on the open graph.
#[operator(
    id = "animation.graph.add_state",
    label = "Add State",
    description = "Add a state to the open graph, playing one clip. The first state added \
                   becomes the entry state.",
    is_available = a_graph_is_open,
    allows_undo = false,
    params(
        name(String, doc = "What the state is called. Defaults to the clip's name."),
        clip(
            String,
            doc = "The clip it plays, as \"<assets-relative file>#<clip name>\"."
        ),
        r#loop(bool, default = true, doc = "Whether the state runs forever or once."),
        speed(f64, default = 1.0, doc = "Playback rate, as a multiple of the clip's own."),
    ),
)]
pub(crate) fn animation_graph_add_state(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let clip = params
        .as_str("clip")
        .filter(|spec| !spec.is_empty())
        .map(str::to_string);
    let wanted = params.as_str("name").map(str::to_string);
    let looped = params.as_bool("loop").unwrap_or(true);
    let speed = params.as_float("speed").unwrap_or(1.0) as f32;
    commands.queue(move |world: &mut World| {
        let motion = match clip.as_deref().map(parse_clip_ref) {
            Some(Some(clip)) => AnimationMotion::Clip(clip),
            Some(None) => {
                warn!("animation.graph.add_state: `clip` reads <file>#<clip name>");
                return;
            }
            None => AnimationMotion::default(),
        };
        let named = wanted.clone().unwrap_or_else(|| match &motion {
            AnimationMotion::Clip(clip) => clip.clip.clone(),
            AnimationMotion::Blend1d { parameter, .. } => parameter.clone(),
        });
        let doc = world.resource::<AnimationGraphDoc>();
        let name = doc.unused_state_name(&named);
        let position = doc.free_position();
        commit_graph_edit(world, "Add state", |def| {
            def.states.push(AnimationGraphState {
                name: name.clone(),
                motion,
                looped,
                speed,
                position,
            });
            if def.entry.is_empty() {
                def.entry = name;
            }
        });
    });
    OperatorResult::Finished
}

/// Change what a state is called or what it plays.
#[operator(
    id = "animation.graph.set_state",
    label = "Set State",
    description = "Change a state of the open graph: what it is called, the clip it plays, \
                   whether it loops and how fast it runs.",
    is_available = a_graph_is_open,
    allows_undo = false,
    params(
        name(String, doc = "The state to change."),
        rename(String, doc = "What to call it instead. Transitions follow the new name."),
        clip(
            String,
            doc = "The clip it plays, as \"<assets-relative file>#<clip name>\"."
        ),
        r#loop(bool, doc = "Whether the state runs forever or once."),
        speed(f64, doc = "Playback rate, as a multiple of the clip's own."),
    ),
)]
pub(crate) fn animation_graph_set_state(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let name = params.as_str("name").map(str::to_string)?;
    let rename = params
        .as_str("rename")
        .filter(|wanted| !wanted.is_empty())
        .map(str::to_string);
    let clip = params
        .as_str("clip")
        .filter(|spec| !spec.is_empty())
        .map(str::to_string);
    let looped = params.as_bool("loop");
    let speed = params.as_float("speed").map(|speed| speed as f32);
    commands.queue(move |world: &mut World| {
        let doc = world.resource::<AnimationGraphDoc>();
        if doc.state(&name).is_none() {
            warn!("animation.graph.set_state: no state is called `{name}`");
            return;
        }
        let renamed = rename.as_ref().map(|wanted| doc.unused_state_name(wanted));
        let motion = match clip.as_deref().map(parse_clip_ref) {
            Some(Some(clip)) => Some(AnimationMotion::Clip(clip)),
            Some(None) => {
                warn!("animation.graph.set_state: `clip` reads <file>#<clip name>");
                return;
            }
            None => None,
        };
        commit_graph_edit(world, "Set state", |def| {
            let Some(state) = def.states.iter_mut().find(|state| state.name == name) else {
                return;
            };
            if let Some(motion) = motion {
                state.motion = motion;
            }
            if let Some(looped) = looped {
                state.looped = looped;
            }
            if let Some(speed) = speed {
                state.speed = speed;
            }
            let Some(renamed) = renamed else {
                return;
            };
            state.name = renamed.clone();
            for transition in &mut def.transitions {
                if transition.from == name {
                    transition.from = renamed.clone();
                }
                if transition.to == name {
                    transition.to = renamed.clone();
                }
            }
            if def.entry == name {
                def.entry = renamed;
            }
        });
    });
    OperatorResult::Finished
}

/// Take a state off the open graph.
#[operator(
    id = "animation.graph.remove_state",
    label = "Remove State",
    description = "Take a state and every transition touching it off the open graph.",
    is_available = a_graph_is_open,
    allows_undo = false,
    params(name(String, doc = "The state to take away.")),
)]
pub(crate) fn animation_graph_remove_state(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let name = params.as_str("name").map(str::to_string)?;
    commands.queue(move |world: &mut World| {
        commit_graph_edit(world, "Remove state", |def| {
            def.states.retain(|state| state.name != name);
            def.transitions
                .retain(|transition| transition.from != name && transition.to != name);
            if def.entry == name {
                def.entry = def
                    .states
                    .first()
                    .map(|state| state.name.clone())
                    .unwrap_or_default();
            }
        });
    });
    OperatorResult::Finished
}

/// Put a transition on the open graph.
#[operator(
    id = "animation.graph.add_transition",
    label = "Add Transition",
    description = "Move the graph from one state to another once its conditions hold.",
    is_available = a_graph_is_open,
    allows_undo = false,
    params(
        from(
            String,
            doc = "The state it leaves. Left out, it is taken from every state."
        ),
        to(String, doc = "The state it arrives at."),
        when(
            String,
            doc = "Conditions, comma separated, each \"<parameter> <op> <value>\" with an op of \
                   >, >=, <, <=, == or !=. A trigger parameter is named on its own."
        ),
        fade(f64, default = 0.15, doc = "Seconds the state it leaves fades out over."),
        exit_time(
            f64,
            doc = "How far through its clip the state it leaves must be, from zero to one."
        ),
    ),
)]
pub(crate) fn animation_graph_add_transition(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let from = params.as_str("from").unwrap_or_default().to_string();
    let to = params.as_str("to").map(str::to_string)?;
    let when = params.as_str("when").unwrap_or_default().to_string();
    let fade = params.as_float("fade").unwrap_or(0.15) as f32;
    let exit_time = params.as_float("exit_time").map(|time| time as f32);
    commands.queue(move |world: &mut World| {
        let doc = world.resource::<AnimationGraphDoc>();
        if doc.state(&to).is_none() {
            warn!("animation.graph.add_transition: no state is called `{to}`");
            return;
        }
        if !from.is_empty() && doc.state(&from).is_none() {
            warn!("animation.graph.add_transition: no state is called `{from}`");
            return;
        }
        let Some(conditions) = parse_conditions(&when) else {
            warn!("animation.graph.add_transition: `when` reads <parameter> <op> <value>");
            return;
        };
        commit_graph_edit(world, "Add transition", |def| {
            def.transitions.push(AnimationTransitionDef {
                from: from.clone(),
                to: to.clone(),
                conditions,
                exit_time,
                crossfade_secs: fade,
                ..AnimationTransitionDef::default()
            });
        });
    });
    OperatorResult::Finished
}

/// Take a transition off the open graph.
#[operator(
    id = "animation.graph.remove_transition",
    label = "Remove Transition",
    description = "Take the transition between two states off the open graph.",
    is_available = a_graph_is_open,
    allows_undo = false,
    params(
        from(String, doc = "The state it leaves. Left out, an any-state transition."),
        to(String, doc = "The state it arrives at."),
    ),
)]
pub(crate) fn animation_graph_remove_transition(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let from = params.as_str("from").unwrap_or_default().to_string();
    let to = params.as_str("to").map(str::to_string)?;
    commands.queue(move |world: &mut World| {
        commit_graph_edit(world, "Remove transition", |def| {
            if let Some(at) = def
                .transitions
                .iter()
                .position(|held| held.from == from && held.to == to)
            {
                def.transitions.remove(at);
            }
        });
    });
    OperatorResult::Finished
}

/// Say which state the graph starts in.
#[operator(
    id = "animation.graph.set_entry",
    label = "Set Entry State",
    description = "Say which state the graph starts in.",
    is_available = a_graph_is_open,
    allows_undo = false,
    params(name(String, doc = "The state to start in.")),
)]
pub(crate) fn animation_graph_set_entry(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let name = params.as_str("name").map(str::to_string)?;
    commands.queue(move |world: &mut World| {
        if world.resource::<AnimationGraphDoc>().state(&name).is_none() {
            warn!("animation.graph.set_entry: no state is called `{name}`");
            return;
        }
        commit_graph_edit(world, "Set entry state", |def| def.entry = name.clone());
    });
    OperatorResult::Finished
}

/// Declare a parameter the game writes.
#[operator(
    id = "animation.graph.add_param",
    label = "Add Parameter",
    description = "Declare a parameter the game writes to steer the graph.",
    is_available = a_graph_is_open,
    allows_undo = false,
    params(
        name(
            String,
            doc = "What the parameter is called. Left out, the next unused \"Parameter\" name."
        ),
        kind(
            String,
            default = "float",
            doc = "What it holds: \"float\", \"bool\" or \"trigger\"."
        ),
        default(f64, doc = "The value it starts at."),
    ),
)]
pub(crate) fn animation_graph_add_param(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let wanted = params
        .as_str("name")
        .filter(|name| !name.is_empty())
        .map(str::to_string);
    let kind = params.as_str("kind").unwrap_or("float").to_string();
    let start = params.as_float("default").unwrap_or_default() as f32;
    commands.queue(move |world: &mut World| {
        let Some(kind) = parameter_kind_from_name(&kind) else {
            warn!("animation.graph.add_param: `kind` is float, bool or trigger");
            return;
        };
        let def = &world.resource::<AnimationGraphDoc>().def;
        let name = match wanted.clone() {
            Some(name) if parameter(def, &name).is_some() => {
                warn!("animation.graph.add_param: a parameter is already called `{name}`");
                return;
            }
            Some(name) => name,
            None => (1..)
                .map(|n| format!("Parameter {n}"))
                .find(|name| parameter(def, name).is_none())
                .unwrap_or_else(|| "Parameter".to_string()),
        };
        commit_graph_edit(world, "Add parameter", |def| {
            def.parameters.push(AnimationParameterDef {
                name: name.clone(),
                kind,
                default: start,
            });
        });
    });
    OperatorResult::Finished
}

/// Write a parameter on whatever the graph is previewing.
#[operator(
    id = "animation.graph.set_param",
    label = "Set Parameter",
    description = "Write a parameter on the entity the open graph is previewing, so the \
                   viewport shows what it steers.",
    is_available = a_graph_is_open,
    allows_undo = false,
    params(
        name(String, doc = "The parameter to write."),
        value(f64, default = 1.0, doc = "What to write. A bool reads zero as false."),
    ),
)]
pub(crate) fn animation_graph_set_param(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let name = params.as_str("name").map(str::to_string)?;
    let value = params.as_float("value").unwrap_or(1.0) as f32;
    commands.queue(move |world: &mut World| {
        let Some(kind) = parameter(&world.resource::<AnimationGraphDoc>().def, &name)
            .map(|parameter| parameter.kind)
        else {
            warn!("animation.graph.set_param: the graph declares no parameter `{name}`");
            return;
        };
        let Some(target) = graph_preview_target(world) else {
            warn!("animation.graph.set_param: nothing in the scene is playing this graph");
            return;
        };
        if world.get::<AnimationParams>(target).is_none() {
            world.entity_mut(target).insert(AnimationParams::default());
        }
        let Some(mut written) = world.get_mut::<AnimationParams>(target) else {
            return;
        };
        match kind {
            AnimationParameterKind::Float => written.set_float(&name, value),
            AnimationParameterKind::Bool => written.set_bool(&name, value != 0.0),
            AnimationParameterKind::Trigger if value != 0.0 => written.trigger(&name),
            AnimationParameterKind::Trigger => written.clear_trigger(&name),
        }
    });
    OperatorResult::Finished
}

/// Point the selected entity at a graph file.
#[operator(
    id = "animation.graph.assign",
    label = "Use Animation Graph",
    description = "Point the selected entity's graph reference at a graph file, and open that \
                   graph in the Graph window.",
    allows_undo = false,
    params(path(String, doc = "Assets-relative path of the graph file to play."))
)]
pub(crate) fn animation_graph_assign(
    params: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    let path = params.as_str("path").map(str::to_string)?;
    commands.queue(move |world: &mut World| {
        let Some(entity) = world.resource::<crate::selection::Selection>().primary() else {
            return;
        };
        if !crate::commands::field_edit_commit_on(
            world,
            entity,
            <AnimationGraphRef as bevy::reflect::TypePath>::type_path(),
            "path",
            &serde_json::json!(path),
        ) {
            warn!("animation.graph.assign: the entity refused the graph path");
            return;
        }
        if open_graph(world, &path) {
            crate::open_window_in_default_area_if_absent(
                world,
                super::graph_window::GRAPH_WINDOW_ID,
            );
        }
    });
    OperatorResult::Finished
}

/// Write a selected animation set out as a graph and open it.
#[operator(
    id = "animation.graph.convert_set",
    label = "Convert To Graph",
    description = "Write the selected entity's animation set out as a graph file, and point \
                   the entity at it.",
    is_available = a_set_without_a_graph_is_selected,
    allows_undo = false
)]
pub(crate) fn animation_graph_convert_set(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(convert_selected_set);
    OperatorResult::Finished
}

/// Turn the selected set into a graph file, and give the entity a reference to
/// it so the rig plays the graph rather than the set.
fn convert_selected_set(world: &mut World) {
    let Some(entity) = world.resource::<crate::selection::Selection>().primary() else {
        return;
    };
    let Some(set) = world
        .get::<jackdaw_animation_runtime::AnimationSet>(entity)
        .cloned()
    else {
        return;
    };
    let name = world
        .get::<Name>(entity)
        .map(|name| name.as_str().to_string())
        .unwrap_or_else(|| "graph".to_string());
    let path = graph_path_for_name(&name);
    super::graph_doc::install_graph(world, &path, set.to_graph(), true);
    if write_graph_file(world).is_none() {
        return;
    }
    let reference = AnimationGraphRef {
        path: path.clone(),
        skeleton_root: set.skeleton_root.clone(),
    };
    let registry = world.resource::<AppTypeRegistry>().clone();
    let value = {
        let registry = registry.read();
        crate::inspector::reflect_fields::reflect_to_json(&reference, &registry)
    };
    let Some(value) = value else {
        warn!("animation.graph.convert_set: the reference did not convert to a value to author");
        return;
    };
    if !crate::commands::field_edit_commit_on(
        world,
        entity,
        <AnimationGraphRef as bevy::reflect::TypePath>::type_path(),
        "",
        &value,
    ) {
        warn!("animation.graph.convert_set: the entity refused the graph reference");
    }
    crate::open_window_in_default_area_if_absent(world, super::graph_window::GRAPH_WINDOW_ID);
}

fn a_graph_is_open(doc: Res<AnimationGraphDoc>) -> bool {
    doc.path.is_some()
}

fn a_set_without_a_graph_is_selected(
    selection: Res<crate::selection::Selection>,
    sets: Query<
        (),
        (
            With<jackdaw_animation_runtime::AnimationSet>,
            Without<AnimationGraphRef>,
        ),
    >,
) -> bool {
    selection
        .primary()
        .is_some_and(|entity| sets.contains(entity))
}

/// Read the conditions a transition is authored with.
///
/// An empty text is no condition at all, which is a transition taken as soon
/// as its exit time allows.
fn parse_conditions(when: &str) -> Option<Vec<AnimationCondition>> {
    let mut conditions = Vec::new();
    for clause in when.split(',').map(str::trim).filter(|c| !c.is_empty()) {
        let words: Vec<&str> = clause.split_whitespace().collect();
        let condition = match words.as_slice() {
            [parameter] => AnimationCondition {
                parameter: (*parameter).to_string(),
                ..AnimationCondition::default()
            },
            [parameter, op, value] => AnimationCondition {
                parameter: (*parameter).to_string(),
                op: condition_op_from_name(op)?,
                value: value.parse().ok()?,
            },
            _ => return None,
        };
        conditions.push(condition);
    }
    Some(conditions)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_animation_runtime::AnimationConditionOp;

    #[test]
    fn a_condition_reads_its_parameter_operator_and_value() {
        let conditions = parse_conditions("speed >= 2.5").expect("the clause reads");
        assert_eq!(conditions.len(), 1);
        assert_eq!(conditions[0].parameter, "speed");
        assert_eq!(conditions[0].op, AnimationConditionOp::GreaterOrEqual);
        assert_eq!(conditions[0].value, 2.5);
    }

    #[test]
    fn a_trigger_condition_is_named_on_its_own() {
        let conditions = parse_conditions("attack, speed < 1").expect("the clauses read");
        assert_eq!(conditions.len(), 2);
        assert_eq!(conditions[0].parameter, "attack");
        assert_eq!(conditions[1].op, AnimationConditionOp::Less);
    }

    #[test]
    fn a_clause_with_no_operator_is_refused() {
        assert!(parse_conditions("speed 2.5").is_none());
        assert!(parse_conditions("speed ~ 2.5").is_none());
    }
}
