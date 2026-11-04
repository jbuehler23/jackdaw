//! The Graph window: the open animation graph drawn on the node canvas.
//!
//! The canvas itself comes from [`jackdaw_node_graph`]; this module hosts it,
//! puts the graph's parameters and its selected state beside it, and lights
//! whichever state the rig is actually in.

use bevy::feathers::controls::FeathersCheckbox;
use bevy::feathers::theme::ThemedText;
use bevy::prelude::*;
use bevy::ui::Checked;
use bevy::ui_widgets::{SliderValue, ValueChange};
use jackdaw_animation_runtime::{
    AnimationGraphPlayback, AnimationGraphRef, AnimationParameterKind,
};
use jackdaw_api::op::OperatorCommandsExt as _;
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{ButtonOperatorCall, ButtonProps, ButtonSize, ButtonVariant, button},
    slider_row::{SliderRowProps, spawn_slider_row},
    text_edit::{TextEditCommitEvent, TextEditProps, text_edit},
    tokens,
};
use jackdaw_node_graph::{
    Connection, GraphCanvasWorld, GraphNode, GraphNodeBody, GraphNodeView, GraphSelection,
};

use super::graph_doc::{
    AnimationGraphDoc, GraphAnyStateNode, GraphStateNode, motion_summary, transition_summary,
};
use super::graph_ops::{
    AnimationGraphAddParamOp, AnimationGraphAddStateOp, AnimationGraphRemoveStateOp,
    AnimationGraphSaveOp, AnimationGraphSetEntryOp, AnimationGraphSetParamOp,
    AnimationGraphSetStateOp,
};

/// Catalog id of the dock window this module builds.
pub const GRAPH_WINDOW_ID: &str = "jackdaw.animation_graph";

/// How far a wire's annotation sits from the ends it joins.
const NODE_CENTRE: Vec2 = Vec2::new(80.0, 40.0);

/// Colour of the state the rig is in.
const RUNNING_STATE_BG: Color = Color::srgb(0.16, 0.34, 0.24);

/// Colour of a state standing still.
const IDLE_STATE_BG: Color = Color::srgb(0.14, 0.15, 0.17);

/// Colour of the annotation on a transition that is running.
const RUNNING_WIRE_BG: Color = Color::srgba(0.30, 0.65, 0.45, 0.85);

/// Colour of the annotation on a transition standing still.
const IDLE_WIRE_BG: Color = Color::srgba(0.0, 0.0, 0.0, 0.55);

/// Marker for the window's toolbar row.
#[derive(Component)]
struct GraphToolbar;

/// Marker for the window's parameter strip.
#[derive(Component)]
struct GraphParameterStrip;

/// Marker for the strip showing whichever state is selected.
#[derive(Component)]
struct GraphStateStrip;

/// Marker for the container the canvas is built into.
#[derive(Component)]
struct GraphCanvasHost;

/// Marker on everything this window spawns, so a part left with no parent when
/// the dock rebuilt the window is dropped rather than drawn over the editor.
#[derive(Component)]
struct GraphWindowPart;

/// The state name a rename field is editing.
#[derive(Component)]
struct StateNameField(String);

/// The parameter a control writes.
#[derive(Component)]
struct ParameterControl {
    name: String,
    kind: AnimationParameterKind,
}

/// The label drawn beside one wire.
#[derive(Component)]
struct TransitionLabel(Entity);

/// What one node's body was last filled with.
#[derive(Component)]
struct StateBodySummary(String);

/// What the graph is doing, as the canvas draws it.
#[derive(Debug, Default, PartialEq)]
pub struct GraphHighlight {
    /// The state driving the rig.
    pub state: String,
    /// The state being faded out, while a transition is still running.
    pub fading_from: Option<String>,
}

/// What a playback says the canvas should light.
pub fn highlight_of(playback: &AnimationGraphPlayback) -> GraphHighlight {
    GraphHighlight {
        state: playback.state.clone(),
        fading_from: playback
            .transition
            .as_ref()
            .filter(|transition| transition.remaining_secs > 0.0)
            .map(|transition| transition.from.clone()),
    }
}

/// The entity the open graph previews on.
///
/// The selection wins when it plays this graph, so a scene holding several
/// rigs follows the one being worked on; otherwise the first entity playing it
/// stands in, which is what a scene with one rig wants.
pub fn graph_preview_target(world: &mut World) -> Option<Entity> {
    let path = world.resource::<AnimationGraphDoc>().path.clone()?;
    let selected = world.resource::<crate::selection::Selection>().primary();
    let mut playing = world.query::<(Entity, &AnimationGraphRef)>();
    let mut first = None;
    for (entity, reference) in playing.iter(world) {
        if reference.path != path {
            continue;
        }
        if Some(entity) == selected {
            return Some(entity);
        }
        first.get_or_insert(entity);
    }
    first
}

/// Builds the Graph window: a toolbar over a parameter strip, a state strip
/// and the canvas. The systems in this module fill all four.
pub fn animation_graph_window_content() -> impl Bundle {
    (
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            (GraphToolbar, strip_node()),
            (GraphParameterStrip, strip_node()),
            (GraphStateStrip, strip_node()),
            (
                GraphCanvasHost,
                Node {
                    width: percent(100),
                    flex_grow: 1.0,
                    min_height: px(0),
                    ..default()
                },
            ),
        ],
    )
}

fn strip_node() -> Node {
    Node {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: px(tokens::SPACING_SM),
        padding: UiRect::axes(px(tokens::SPACING_SM), px(tokens::SPACING_XS)),
        width: percent(100),
        flex_shrink: 0.0,
        flex_wrap: FlexWrap::Wrap,
        row_gap: px(tokens::SPACING_XS),
        ..default()
    }
}

fn despawn_children(commands: &mut Commands, children: Option<&Children>) {
    for child in children.into_iter().flatten() {
        commands.entity(*child).despawn();
    }
}

fn hint(commands: &mut Commands, parent: Entity, text: &str) {
    commands.spawn((
        GraphWindowPart,
        Text::new(text.to_string()),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(parent),
    ));
}

/// Build the canvas into the window body once a graph is open.
fn build_graph_canvas(
    mut commands: Commands,
    doc: Res<AnimationGraphDoc>,
    hosts: Query<(Entity, Option<&Children>), With<GraphCanvasHost>>,
    worlds: Query<&GraphCanvasWorld>,
) {
    let showing = worlds.iter().any(|world| Some(world.graph) == doc.graph);
    for (host, children) in &hosts {
        match doc.graph {
            Some(graph) if !showing => {
                despawn_children(&mut commands, children);
                let canvas = commands
                    .spawn((
                        GraphWindowPart,
                        jackdaw_node_graph::canvas(graph),
                        ChildOf(host),
                    ))
                    .id();
                commands.spawn((
                    GraphWindowPart,
                    jackdaw_node_graph::canvas_world(graph),
                    ChildOf(canvas),
                ));
            }
            None if children.is_some_and(|kids| !kids.is_empty()) => {
                despawn_children(&mut commands, children);
                hint(
                    &mut commands,
                    host,
                    "No graph is open. Pick one in the Animation panel's Graph tab.",
                );
            }
            _ => {}
        }
    }
}

/// What the toolbar was last drawn for.
#[derive(PartialEq)]
struct ToolbarSignature {
    path: Option<String>,
    entry: String,
    dirty: bool,
    states: usize,
}

fn update_graph_toolbar(
    mut commands: Commands,
    doc: Res<AnimationGraphDoc>,
    bars: Query<(Entity, Option<&Children>), With<GraphToolbar>>,
    mut last: Local<Option<ToolbarSignature>>,
) {
    let signature = ToolbarSignature {
        path: doc.path.clone(),
        entry: doc.def.entry.clone(),
        dirty: doc.dirty,
        states: doc.def.states.len(),
    };
    if last.as_ref() == Some(&signature) && bars.iter().all(|(_, kids)| filled(kids)) {
        return;
    }
    *last = Some(signature);

    for (bar, children) in &bars {
        despawn_children(&mut commands, children);
        let title = match &doc.path {
            Some(path) if doc.dirty => format!("{path} (unsaved)"),
            Some(path) => path.clone(),
            None => "No graph open".to_string(),
        };
        commands.spawn((
            GraphWindowPart,
            Text::new(title),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(bar),
        ));
        if doc.path.is_none() {
            continue;
        }
        commands.spawn((
            GraphWindowPart,
            button(ButtonProps::new("Add state").with_size(ButtonSize::MD)),
            ButtonOperatorCall::new(AnimationGraphAddStateOp::ID),
            ChildOf(bar),
        ));
        commands.spawn((
            GraphWindowPart,
            button(
                ButtonProps::new("Save")
                    .with_size(ButtonSize::MD)
                    .with_variant(ButtonVariant::Default),
            ),
            ButtonOperatorCall::new(AnimationGraphSaveOp::ID),
            ChildOf(bar),
        ));
        commands.spawn((
            GraphWindowPart,
            button(ButtonProps::new("Add parameter").with_size(ButtonSize::MD)),
            ButtonOperatorCall::new(AnimationGraphAddParamOp::ID),
            ChildOf(bar),
        ));
        let entry = if doc.def.entry.is_empty() {
            "no entry state".to_string()
        } else {
            format!("entry: {}", doc.def.entry)
        };
        commands.spawn((
            GraphWindowPart,
            Text::new(entry),
            TextFont {
                font_size: tokens::TEXT_SIZE_XS,
                ..default()
            },
            TextColor(tokens::TEXT_MUTED_COLOR.into()),
            ChildOf(bar),
        ));
    }
}

fn filled(children: Option<&Children>) -> bool {
    children.is_some_and(|kids| !kids.is_empty())
}

/// What the parameter strip was last drawn for.
#[derive(PartialEq)]
struct ParameterSignature {
    path: Option<String>,
    parameters: Vec<(String, AnimationParameterKind)>,
}

fn update_parameter_strip(
    mut commands: Commands,
    doc: Res<AnimationGraphDoc>,
    strips: Query<(Entity, Option<&Children>), With<GraphParameterStrip>>,
    mut last: Local<Option<ParameterSignature>>,
) {
    let signature = ParameterSignature {
        path: doc.path.clone(),
        parameters: doc
            .def
            .parameters
            .iter()
            .map(|parameter| (parameter.name.clone(), parameter.kind))
            .collect(),
    };
    if last.as_ref() == Some(&signature) && strips.iter().all(|(_, kids)| filled(kids)) {
        return;
    }
    *last = Some(signature);

    for (strip, children) in &strips {
        despawn_children(&mut commands, children);
        if doc.path.is_none() {
            continue;
        }
        if doc.def.parameters.is_empty() {
            hint(
                &mut commands,
                strip,
                "This graph declares no parameters yet.",
            );
            continue;
        }
        for parameter in &doc.def.parameters {
            let control = ParameterControl {
                name: parameter.name.clone(),
                kind: parameter.kind,
            };
            match parameter.kind {
                AnimationParameterKind::Trigger => {
                    commands.spawn((
                        GraphWindowPart,
                        button(ButtonProps::new(parameter.name.clone()).with_size(ButtonSize::MD)),
                        ButtonOperatorCall::new(AnimationGraphSetParamOp::ID)
                            .with_param("name", parameter.name.clone())
                            .with_param("value", 1.0),
                        ChildOf(strip),
                    ));
                }
                AnimationParameterKind::Bool => {
                    let caption = parameter.name.clone();
                    let mut checkbox = commands.spawn_scene(bsn! {
                        @FeathersCheckbox {
                            @caption: bsn! { Text(caption) ThemedText }
                        }
                    });
                    checkbox.insert((GraphWindowPart, control, ChildOf(strip)));
                    if parameter.default != 0.0 {
                        checkbox.insert(Checked);
                    }
                }
                AnimationParameterKind::Float => {
                    let row = commands
                        .spawn((
                            GraphWindowPart,
                            Node {
                                width: px(220.0),
                                ..default()
                            },
                            ChildOf(strip),
                        ))
                        .id();
                    spawn_slider_row(
                        &mut commands,
                        row,
                        SliderRowProps::new(
                            parameter.name.clone(),
                            parameter.default,
                            0.0..parameter.default.abs().max(1.0) * 10.0,
                        ),
                        control,
                    );
                }
            }
        }
    }
}

/// Write a bool parameter as its checkbox is clicked.
fn on_parameter_toggle(
    change: On<ValueChange<bool>>,
    controls: Query<&ParameterControl>,
    mut commands: Commands,
) {
    let Ok(control) = controls.get(change.source) else {
        return;
    };
    jackdaw_feathers::utils::set_marker_if_alive::<Checked>(
        &mut commands,
        change.source,
        change.value,
    );
    commands
        .operator(AnimationGraphSetParamOp::ID)
        .param("name", control.name.clone())
        .param("value", if change.value { 1.0 } else { 0.0 })
        .call();
}

/// Write a float parameter as its slider moves.
fn on_parameter_slide(
    change: On<ValueChange<f32>>,
    controls: Query<&ParameterControl>,
    mut commands: Commands,
) {
    let Ok(control) = controls.get(change.source) else {
        return;
    };
    if control.kind != AnimationParameterKind::Float {
        return;
    }
    commands
        .entity(change.source)
        .insert(SliderValue(change.value));
    commands
        .operator(AnimationGraphSetParamOp::ID)
        .param("name", control.name.clone())
        .param("value", f64::from(change.value))
        .call();
}

/// The state the canvas selection names, if it names one.
fn selected_state(selection: &GraphSelection, nodes: &Query<&GraphStateNode>) -> Option<String> {
    selection
        .entities
        .iter()
        .find_map(|entity| nodes.get(*entity).ok())
        .map(|state| state.0.clone())
}

fn update_state_strip(
    mut commands: Commands,
    doc: Res<AnimationGraphDoc>,
    panel: Res<super::AnimationPanelState>,
    selection: Res<GraphSelection>,
    nodes: Query<&GraphStateNode>,
    strips: Query<(Entity, Option<&Children>), With<GraphStateStrip>>,
    mut last: Local<Option<(Option<String>, Option<String>)>>,
) {
    let chosen = selected_state(&selection, &nodes);
    let shown = chosen
        .as_ref()
        .and_then(|name| doc.state(name))
        .map(|state| {
            (
                state.name.clone(),
                motion_summary(&state.motion),
                state.looped,
                state.speed,
            )
        });
    let library_clip = match (&panel.file, &panel.clip) {
        (Some(file), Some(clip)) => Some(format!("{file}#{clip}")),
        _ => None,
    };
    let signature = (
        shown.as_ref().map(|(name, ..)| name.clone()),
        library_clip.clone(),
    );
    if last.as_ref() == Some(&signature) && strips.iter().all(|(_, kids)| filled(kids)) {
        return;
    }
    *last = Some(signature);

    for (strip, children) in &strips {
        despawn_children(&mut commands, children);
        let Some((name, clip, looped, speed)) = shown.clone() else {
            hint(&mut commands, strip, "Select a state to edit it.");
            continue;
        };
        commands.spawn((
            GraphWindowPart,
            StateNameField(name.clone()),
            text_edit(TextEditProps::default().with_default_value(name.clone())),
            ChildOf(strip),
        ));
        hint(&mut commands, strip, &format!("{clip}, x{speed:.2}"));
        commands.spawn((
            GraphWindowPart,
            button(
                ButtonProps::new(if looped { "Looping" } else { "Once" }).with_size(ButtonSize::MD),
            ),
            ButtonOperatorCall::new(AnimationGraphSetStateOp::ID)
                .with_param("name", name.clone())
                .with_param("loop", !looped),
            ChildOf(strip),
        ));
        if let Some(spec) = library_clip.clone() {
            let clip = spec
                .rsplit_once('#')
                .map_or(spec.clone(), |(_, clip)| clip.to_string());
            commands.spawn((
                GraphWindowPart,
                button(ButtonProps::new(format!("Play {clip}")).with_size(ButtonSize::MD)),
                ButtonOperatorCall::new(AnimationGraphSetStateOp::ID)
                    .with_param("name", name.clone())
                    .with_param("clip", spec),
                ChildOf(strip),
            ));
        }
        commands.spawn((
            GraphWindowPart,
            button(ButtonProps::new("Make entry").with_size(ButtonSize::MD)),
            ButtonOperatorCall::new(AnimationGraphSetEntryOp::ID).with_param("name", name.clone()),
            ChildOf(strip),
        ));
        commands.spawn((
            GraphWindowPart,
            button(
                ButtonProps::new("Remove")
                    .with_size(ButtonSize::MD)
                    .with_variant(ButtonVariant::Ghost),
            ),
            ButtonOperatorCall::new(AnimationGraphRemoveStateOp::ID).with_param("name", name),
            ChildOf(strip),
        ));
    }
}

/// Rename a state once its field is committed, rather than as it is typed:
/// a rename per keystroke would leave one undo entry per letter.
fn on_state_name_commit(
    event: On<TextEditCommitEvent>,
    fields: Query<&StateNameField>,
    mut commands: Commands,
) {
    let Ok(field) = fields.get(event.entity) else {
        return;
    };
    if event.text.is_empty() || event.text == field.0 {
        return;
    }
    commands
        .operator(AnimationGraphSetStateOp::ID)
        .param("name", field.0.clone())
        .param("rename", event.text.clone())
        .call();
}

/// Say on each node what its state plays.
fn fill_state_node_bodies(
    mut commands: Commands,
    doc: Res<AnimationGraphDoc>,
    bodies: Query<(
        Entity,
        &GraphNodeBody,
        Option<&Children>,
        Option<&StateBodySummary>,
    )>,
    states: Query<&GraphStateNode>,
    any_state: Query<(), With<GraphAnyStateNode>>,
) {
    for (body, owner, children, summary) in &bodies {
        let wanted = if any_state.contains(owner.node) {
            "every state".to_string()
        } else {
            let Ok(state) = states.get(owner.node) else {
                continue;
            };
            let Some(held) = doc.state(&state.0) else {
                continue;
            };
            let entry = if doc.def.entry == held.name {
                "entry, "
            } else {
                ""
            };
            format!(
                "{}\n{entry}{}{}, x{:.2}",
                held.name,
                motion_summary(&held.motion),
                if held.looped { ", loop" } else { "" },
                held.speed
            )
        };
        if summary.is_some_and(|summary| summary.0 == wanted) {
            continue;
        }
        despawn_children(&mut commands, children);
        commands
            .entity(body)
            .insert(StateBodySummary(wanted.clone()));
        commands.spawn((
            GraphWindowPart,
            jackdaw_node_graph::body_label(wanted),
            ChildOf(body),
        ));
    }
}

/// Draw each wire's conditions and crossfade beside it.
fn update_transition_labels(
    mut commands: Commands,
    doc: Res<AnimationGraphDoc>,
    wires: Query<(Entity, &Connection)>,
    nodes: Query<&GraphNode>,
    worlds: Query<(Entity, &GraphCanvasWorld)>,
    mut labels: Query<(Entity, &TransitionLabel, &mut Node, &mut Text)>,
) {
    let Some(graph) = doc.graph else {
        return;
    };
    let Some((canvas, _)) = worlds.iter().find(|(_, world)| world.graph == graph) else {
        return;
    };
    for (label, owner, _, _) in &labels {
        if !wires.contains(owner.0) {
            commands.entity(label).despawn();
        }
    }
    for (at, transition) in doc.def.transitions.iter().enumerate() {
        let Some(wire) = doc.wire(at) else {
            continue;
        };
        let Ok((_, connection)) = wires.get(wire) else {
            continue;
        };
        let (Ok(source), Ok(target)) = (
            nodes.get(connection.source_node),
            nodes.get(connection.target_node),
        ) else {
            continue;
        };
        let at_position = (source.position + target.position) / 2.0 + NODE_CENTRE;
        let wanted = transition_summary(transition);
        match labels.iter_mut().find(|(_, owner, _, _)| owner.0 == wire) {
            Some((_, _, mut node, mut text)) => {
                node.left = px(at_position.x);
                node.top = px(at_position.y);
                if text.0 != wanted {
                    text.0 = wanted;
                }
            }
            None => {
                commands.spawn((
                    GraphWindowPart,
                    TransitionLabel(wire),
                    Text::new(wanted),
                    TextFont {
                        font_size: tokens::TEXT_SIZE_XS,
                        ..default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                    BackgroundColor(IDLE_WIRE_BG),
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(at_position.x),
                        top: px(at_position.y),
                        padding: UiRect::axes(px(4.0), px(1.0)),
                        ..default()
                    },
                    Pickable::IGNORE,
                    ChildOf(canvas),
                ));
            }
        }
    }
}

/// Light the state the rig is in and the transition still running.
fn light_running_state(
    doc: Res<AnimationGraphDoc>,
    playbacks: Query<(&AnimationGraphRef, &AnimationGraphPlayback)>,
    states: Query<&GraphStateNode>,
    mut views: Query<(&GraphNodeView, &mut BackgroundColor)>,
    wires: Query<&Connection>,
    mut labels: Query<(&TransitionLabel, &mut BackgroundColor), Without<GraphNodeView>>,
) {
    let Some(path) = doc.path.clone() else {
        return;
    };
    let running = playbacks
        .iter()
        .find(|(reference, _)| reference.path == path)
        .map(|(_, playback)| highlight_of(playback))
        .unwrap_or_default();
    for (view, mut background) in &mut views {
        let lit = states
            .get(view.0)
            .is_ok_and(|state| state.0 == running.state);
        let wanted = if lit { RUNNING_STATE_BG } else { IDLE_STATE_BG };
        if background.0 != wanted {
            background.0 = wanted;
        }
    }
    for (label, mut background) in &mut labels {
        let lit = wires.get(label.0).is_ok_and(|wire| {
            let from = states
                .get(wire.source_node)
                .ok()
                .map(|state| state.0.clone());
            let to = states
                .get(wire.target_node)
                .ok()
                .map(|state| state.0.clone());
            to.is_some_and(|to| to == running.state)
                && running.fading_from.is_some()
                && running.fading_from == from
        });
        let wanted = if lit { RUNNING_WIRE_BG } else { IDLE_WIRE_BG };
        if background.0 != wanted {
            background.0 = wanted;
        }
    }
}

/// Drop anything this window spawned that ended up with no parent.
fn drop_orphaned_window_parts(
    orphans: Query<Entity, (With<GraphWindowPart>, Without<ChildOf>)>,
    mut commands: Commands,
) {
    for orphan in &orphans {
        commands.entity(orphan).despawn();
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<AnimationGraphDoc>()
        .add_systems(Startup, super::graph_doc::register_graph_node_types)
        .add_observer(on_parameter_toggle)
        .add_observer(on_parameter_slide)
        .add_observer(on_state_name_commit)
        .add_systems(
            Update,
            (
                super::graph_doc::read_canvas_back,
                super::graph_doc::reconcile_canvas,
                build_graph_canvas,
                update_graph_toolbar,
                update_parameter_strip,
                update_state_strip,
                fill_state_node_bodies,
                update_transition_labels,
                light_running_state,
                drop_orphaned_window_parts,
            )
                .chain()
                .run_if(in_state(crate::AppState::Editor)),
        );
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_animation_runtime::AnimationGraphTransition;

    #[test]
    fn the_highlight_names_the_state_the_rig_is_in() {
        let playback = AnimationGraphPlayback {
            state: "run".into(),
            since_secs: 0.2,
            transition: None,
        };
        assert_eq!(
            highlight_of(&playback),
            GraphHighlight {
                state: "run".into(),
                fading_from: None,
            }
        );
    }

    #[test]
    fn a_crossfade_still_running_lights_the_state_it_leaves() {
        let playback = AnimationGraphPlayback {
            state: "run".into(),
            since_secs: 0.05,
            transition: Some(AnimationGraphTransition {
                from: "idle".into(),
                remaining_secs: 0.1,
            }),
        };
        assert_eq!(highlight_of(&playback).fading_from.as_deref(), Some("idle"));
    }

    #[test]
    fn a_crossfade_that_has_run_out_lights_nothing_behind_it() {
        let playback = AnimationGraphPlayback {
            state: "run".into(),
            since_secs: 0.3,
            transition: Some(AnimationGraphTransition {
                from: "idle".into(),
                remaining_secs: 0.0,
            }),
        };
        assert!(highlight_of(&playback).fading_from.is_none());
    }
}
