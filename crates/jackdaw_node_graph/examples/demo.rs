//! Minimal stand-alone demo for the node graph crate.
//!
//! Spawns a single graph with editable `Constant` nodes feeding an `Add`
//! node and computes the sum in real time so every value change ripples
//! through the graph. This exercises the full interaction + evaluation
//! stack end-to-end.
//!
//! The `Constant` nodes use `jackdaw_feathers::text_edit` so the numeric
//! inputs match Jackdaw's editor styling (pre-focus prefix drag, enter/
//! escape to commit, tabular-figure font, muted border, etc.) and the
//! `Add` node's read-only sum display uses the same feathers typography
//! tokens via [`body_label`].
//!
//! Run with: `cargo run -p jackdaw_node_graph --example demo`

use bevy::prelude::*;

use jackdaw_feathers::{
    EditorFeathersPlugin,
    text_edit::{self, TextEditProps, TextEditValue},
};
use jackdaw_node_graph::{
    Connection, GraphCanvasView, GraphNode, GraphNodeBody, NodeGraph, NodeGraphPlugin,
    NodeTypeDescriptor, NodeTypeRegistry, TerminalDescriptor, body_label, canvas, canvas_world,
};

fn main() -> AppExit {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(EditorFeathersPlugin)
        .add_plugins(NodeGraphPlugin)
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (
                populate_demo_node_bodies,
                sync_constant_inputs,
                evaluate_demo_graph,
                update_demo_body_labels.after(evaluate_demo_graph),
            ),
        )
        .run()
}

// --------------------------------------------------------------------------
// Demo components
// --------------------------------------------------------------------------

/// Per-entity numeric value for `demo.constant` nodes. Edited through the
/// feathers `text_edit` widget via [`sync_constant_inputs`].
#[derive(Component, Debug, Clone, Copy)]
struct ConstantValue(f32);

/// Cached computed result for any node; `ConstantValue` for constants,
/// sum of resolved inputs for `demo.add` nodes. Written by
/// [`evaluate_demo_graph`] each frame.
#[derive(Component, Debug, Clone, Copy, Default)]
struct NodeResult(f32);

/// Back-pointer on the `Add` node's body label text entity so
/// [`update_demo_body_labels`] can find its source data.
#[derive(Component, Debug, Clone, Copy)]
struct DemoBodyLabel {
    node: Entity,
}

/// Marker on a feathers `text_edit` wrapper that should write its parsed
/// value into `ConstantValue` on the referenced `GraphNode` data entity.
#[derive(Component, Debug, Clone, Copy)]
struct ConstantInputTarget {
    node: Entity,
}

// --------------------------------------------------------------------------
// Setup
// --------------------------------------------------------------------------

fn setup(mut commands: Commands, mut registry: ResMut<NodeTypeRegistry>) {
    commands.spawn(Camera2d);
    register_demo_node_types(&mut registry);

    let graph = commands
        .spawn((
            NodeGraph {
                title: "Demo Graph".into(),
            },
            GraphCanvasView::default(),
        ))
        .id();

    let const_a = commands
        .spawn((
            GraphNode {
                node_type: "demo.constant".into(),
                position: Vec2::new(60.0, 60.0),
            },
            ConstantValue(1.5),
            NodeResult::default(),
            ChildOf(graph),
        ))
        .id();
    let const_b = commands
        .spawn((
            GraphNode {
                node_type: "demo.constant".into(),
                position: Vec2::new(60.0, 260.0),
            },
            ConstantValue(2.75),
            NodeResult::default(),
            ChildOf(graph),
        ))
        .id();
    let add = commands
        .spawn((
            GraphNode {
                node_type: "demo.add".into(),
                position: Vec2::new(380.0, 160.0),
            },
            NodeResult::default(),
            ChildOf(graph),
        ))
        .id();

    commands.spawn((
        Connection {
            source_node: const_a,
            source_terminal: 0,
            target_node: add,
            target_terminal: 0,
        },
        ChildOf(graph),
    ));
    commands.spawn((
        Connection {
            source_node: const_b,
            source_terminal: 0,
            target_node: add,
            target_terminal: 1,
        },
        ChildOf(graph),
    ));

    let canvas_root = commands.spawn(canvas(graph)).id();
    commands
        .spawn(canvas_world(graph))
        .insert(ChildOf(canvas_root));
}

// --------------------------------------------------------------------------
// Body population
// --------------------------------------------------------------------------

/// Runs once per newly-spawned `GraphNodeBody`. Constants get a feathers
/// `text_edit` field configured as `numeric_f32` with its default value
/// seeded from `ConstantValue`. Adds get a read-only result label using
/// `body_label` (which itself resolves to feathers typography tokens).
fn populate_demo_node_bodies(
    added_bodies: Query<(Entity, &GraphNodeBody), Added<GraphNodeBody>>,
    nodes: Query<(&GraphNode, Option<&ConstantValue>)>,
    mut commands: Commands,
) {
    for (body_entity, body) in added_bodies.iter() {
        let Ok((graph_node, constant)) = nodes.get(body.node) else {
            continue;
        };
        match graph_node.node_type.as_str() {
            "demo.constant" => {
                let value = constant.map(|c| c.0).unwrap_or(0.0);
                commands.spawn((
                    text_edit::text_edit(
                        TextEditProps::default()
                            .numeric_f32()
                            .with_default_value(format!("{value:.2}"))
                            .with_min(-100.0)
                            .with_max(100.0),
                    ),
                    ConstantInputTarget { node: body.node },
                    ChildOf(body_entity),
                ));
            }
            "demo.add" => {
                commands.spawn((
                    body_label("sum = 0.00"),
                    DemoBodyLabel { node: body.node },
                    Pickable::IGNORE,
                    ChildOf(body_entity),
                ));
            }
            _ => {}
        }
    }
}

/// Watch feathers `TextEditValue` changes on any `ConstantInputTarget`
/// wrapper and push the parsed value into the bound `GraphNode` data
/// entity's `ConstantValue`. This runs while the user types, while they
/// drag-scrub the `↔` prefix, and when they commit via Enter/unfocus ;
/// all three paths update the value live.
fn sync_constant_inputs(
    changed: Query<(&ConstantInputTarget, &TextEditValue), Changed<TextEditValue>>,
    mut values: Query<&mut ConstantValue>,
) {
    for (target, value) in changed.iter() {
        // `text_edit::numeric_f32` stores the number with optional decimals,
        // possibly wrapped in a suffix. The feathers input filter already
        // restricts the buffer to `Decimal`, so a plain parse is safe.
        let Ok(parsed) = value.0.parse::<f32>() else {
            continue;
        };
        if let Ok(mut val) = values.get_mut(target.node) {
            val.0 = parsed;
        }
    }
}

/// Compute every node's result each frame.
fn evaluate_demo_graph(
    nodes: Query<(Entity, &GraphNode)>,
    constants: Query<&ConstantValue>,
    connections: Query<&Connection>,
    mut results: Query<&mut NodeResult>,
) {
    // Pass 1: constants (leaves of the dataflow).
    for (entity, node) in nodes.iter() {
        if node.node_type == "demo.constant"
            && let Ok(value) = constants.get(entity)
            && let Ok(mut result) = results.get_mut(entity)
        {
            result.0 = value.0;
        }
    }
    // Pass 2: adds read from the just-written constant results.
    for (entity, node) in nodes.iter() {
        if node.node_type != "demo.add" {
            continue;
        }
        let a = resolve_input(entity, 0, &connections, &results);
        let b = resolve_input(entity, 1, &connections, &results);
        if let Ok(mut result) = results.get_mut(entity) {
            result.0 = a + b;
        }
    }
}

/// Walk back from `(node, terminal)` through the graph's connections
/// and return the resolved value of whatever's wired into it, or 0 if
/// the input is unconnected.
fn resolve_input(
    node: Entity,
    terminal: u32,
    connections: &Query<&Connection>,
    results: &Query<&mut NodeResult>,
) -> f32 {
    for conn in connections.iter() {
        if conn.target_node == node
            && conn.target_terminal == terminal
            && let Ok(result) = results.get(conn.source_node)
        {
            return result.0;
        }
    }
    0.0
}

/// Rewrite the Add node's "sum = ..." label every frame. The Constant
/// nodes update themselves via the `text_edit` widget so they aren't
/// touched here.
fn update_demo_body_labels(
    mut labels: Query<(&DemoBodyLabel, &mut Text)>,
    nodes: Query<&GraphNode>,
    results: Query<&NodeResult>,
) {
    for (label, mut text) in labels.iter_mut() {
        let Ok(node) = nodes.get(label.node) else {
            continue;
        };
        if node.node_type != "demo.add" {
            continue;
        }
        let sum = results.get(label.node).map(|r| r.0).unwrap_or(0.0);
        let new_text = format!("sum = {sum:.2}");
        if text.0 != new_text {
            text.0 = new_text;
        }
    }
}

// --------------------------------------------------------------------------
// Node-type registration
// --------------------------------------------------------------------------

fn register_demo_node_types(registry: &mut NodeTypeRegistry) {
    registry.register(NodeTypeDescriptor {
        id: "demo.constant".into(),
        display_name: "Constant".into(),
        category: "Math".into(),
        accent_color: Color::srgb(0.4, 0.7, 1.0),
        inputs: vec![],
        outputs: vec![TerminalDescriptor {
            label: "value".into(),
            data_type: "f32".into(),
            color: Color::srgb(0.4, 0.7, 1.0),
        }],
        body_components: vec![],
    });
    registry.register(NodeTypeDescriptor {
        id: "demo.add".into(),
        display_name: "Add".into(),
        category: "Math".into(),
        accent_color: Color::srgb(1.0, 0.8, 0.3),
        inputs: vec![
            TerminalDescriptor {
                label: "a".into(),
                data_type: "f32".into(),
                color: Color::srgb(0.4, 0.7, 1.0),
            },
            TerminalDescriptor {
                label: "b".into(),
                data_type: "f32".into(),
                color: Color::srgb(0.4, 0.7, 1.0),
            },
        ],
        outputs: vec![TerminalDescriptor {
            label: "sum".into(),
            data_type: "f32".into(),
            color: Color::srgb(0.4, 0.7, 1.0),
        }],
        body_components: vec![],
    });
}
