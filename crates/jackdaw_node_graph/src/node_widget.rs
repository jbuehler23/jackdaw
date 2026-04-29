//! Bundle function producing a visual node in the canvas.
//!
//! A node is rendered as an absolutely-positioned container with three
//! sections stacked vertically:
//! * Title bar (accent-colored strip + display name)
//! * Terminal row (input column on the left, spacer, output column on the right)
//! * Body area (reserved for inline params; populated by consumer code)
//!
//! The node's on-screen position is driven by
//! [`apply_node_position`] reading from
//! [`GraphNode::position`].

use bevy::prelude::*;

use crate::graph::{GraphNode, GraphNodeSelected, Terminal, TerminalDirection};
use crate::registry::NodeTypeDescriptor;

/// Marker component placed on a node's UI root so systems can query it.
///
/// Holds the `GraphNode` entity it visualizes. Used by the terminal-position
/// tracking system and by selection/drag observers in Phase 2.
#[derive(Component, Debug, Clone, Copy)]
pub struct GraphNodeView(pub Entity);

/// Marker on a terminal's UI dot. Stores the owning node + terminal index so
/// the connection renderer can look up the dot's transform.
#[derive(Component, Debug, Clone, Copy)]
pub struct GraphTerminalView {
    pub node: Entity,
    pub direction: TerminalDirection,
    pub index: u32,
}

/// Marker on the body area UI entity of a node.
///
/// Carries the `GraphNode` data entity so consumer code can populate the
/// body with widgets (labels, sliders, text inputs, etc.) specific to a
/// given node type. Consumers add an `Added<GraphNodeBody>` system that
/// looks up `GraphNode::node_type` for each freshly-spawned body and
/// spawns appropriate children.
///
/// # Example
///
/// ```
/// # use bevy::prelude::*;
/// # use jackdaw_node_graph::{GraphNodeBody, GraphNode};
///
/// fn populate_constant_body(
///     added: Query<(Entity, &GraphNodeBody), Added<GraphNodeBody>>,
///     nodes: Query<&GraphNode>,
///     mut commands: Commands,
/// ) {
///     for (body_entity, body) in added.iter() {
///         let Ok(graph_node) = nodes.get(body.node) else { continue };
///         if graph_node.node_type != "demo.constant" { continue }
///         commands
///             .spawn((Text::new("value: 0.0"), TextFont { font_size: 11.0, ..default() }))
///             .insert(ChildOf(body_entity));
///     }
/// }
/// ```
#[derive(Component, Debug, Clone, Copy)]
pub struct GraphNodeBody {
    pub node: Entity,
}

const NODE_MIN_WIDTH: f32 = 160.0;
const TERMINAL_DOT_SIZE: f32 = 10.0;
/// Invisible hit-area wrapper around the visual dot. Larger than the dot
/// itself so the user can drag connections without needing pixel-perfect
/// aim.
const TERMINAL_HIT_SIZE: f32 = 16.0;
const TITLE_HEIGHT: f32 = 24.0;
const PADDING: f32 = 6.0;
const ROW_HEIGHT: f32 = 18.0;

/// Accent border color applied to selected nodes by
/// [`apply_selection_highlight`].
const SELECTED_BORDER: Color = Color::srgb(0.25, 0.55, 0.95);
/// Default border color for unselected nodes.
const UNSELECTED_BORDER: Color = Color::srgba(1.0, 1.0, 1.0, 0.08);

/// Build a node UI bundle.
///
/// The returned bundle is meant to be spawned as a child of a
/// [`GraphCanvasWorld`](crate::canvas::GraphCanvasWorld) entity. The caller
/// is responsible for spawning child `Terminal` components in lockstep; this
/// function only builds UI.
pub fn node(
    node_entity: Entity,
    node_component: &GraphNode,
    descriptor: &NodeTypeDescriptor,
) -> impl Bundle {
    let accent = descriptor.accent_color;
    let title = descriptor.display_name.clone();

    (
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(node_component.position.x),
            top: Val::Px(node_component.position.y),
            min_width: Val::Px(NODE_MIN_WIDTH),
            flex_direction: FlexDirection::Column,
            border: UiRect::all(Val::Px(1.0)),
            border_radius: BorderRadius::all(Val::Px(6.0)),
            ..default()
        },
        BorderColor::all(UNSELECTED_BORDER),
        BackgroundColor(Color::srgb(0.14, 0.15, 0.17)),
        GraphNodeView(node_entity),
        Pickable::default(),
        children![
            title_bar(title, accent),
            terminals_row(node_entity, descriptor),
            body_area(node_entity),
        ],
    )
}

fn title_bar(title: String, accent: Color) -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(TITLE_HEIGHT),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::horizontal(Val::Px(PADDING)),
            column_gap: Val::Px(PADDING),
            border: UiRect::bottom(Val::Px(1.0)),
            border_radius: BorderRadius::top(Val::Px(5.0)),
            ..default()
        },
        BorderColor::all(Color::srgba(0.0, 0.0, 0.0, 0.3)),
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.15)),
        children![
            (
                // Accent strip (2px wide vertical bar keyed to accent color).
                Node {
                    width: Val::Px(3.0),
                    height: Val::Px(12.0),
                    ..default()
                },
                BackgroundColor(accent),
            ),
            (
                Text::new(title),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(Color::srgb(0.9, 0.9, 0.92)),
            ),
        ],
    )
}

fn terminals_row(node_entity: Entity, descriptor: &NodeTypeDescriptor) -> impl Bundle {
    let inputs = build_terminal_column(node_entity, descriptor, TerminalDirection::Input);
    let outputs = build_terminal_column(node_entity, descriptor, TerminalDirection::Output);

    (
        Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            padding: UiRect::all(Val::Px(PADDING)),
            column_gap: Val::Px(PADDING),
            ..default()
        },
        Children::spawn((Spawn(inputs), Spawn(outputs))),
    )
}

fn build_terminal_column(
    node_entity: Entity,
    descriptor: &NodeTypeDescriptor,
    direction: TerminalDirection,
) -> impl Bundle {
    let entries: Vec<_> = match direction {
        TerminalDirection::Input => descriptor.inputs.iter().enumerate().collect(),
        TerminalDirection::Output => descriptor.outputs.iter().enumerate().collect(),
    };

    let align = if direction == TerminalDirection::Input {
        AlignItems::FlexStart
    } else {
        AlignItems::FlexEnd
    };

    let rows: Vec<_> = entries
        .into_iter()
        .map(|(idx, term)| {
            terminal_row(
                node_entity,
                direction,
                idx as u32,
                term.label.clone(),
                term.color,
            )
        })
        .collect();

    (
        Node {
            flex_grow: 1.0,
            flex_direction: FlexDirection::Column,
            align_items: align,
            row_gap: Val::Px(2.0),
            ..default()
        },
        Children::spawn(SpawnIter(rows.into_iter())),
    )
}

fn terminal_row(
    node_entity: Entity,
    direction: TerminalDirection,
    index: u32,
    label: String,
    color: Color,
) -> impl Bundle {
    let reverse = direction == TerminalDirection::Output;
    let flex_direction = if reverse {
        FlexDirection::RowReverse
    } else {
        FlexDirection::Row
    };

    (
        Node {
            height: Val::Px(TERMINAL_HIT_SIZE.max(ROW_HEIGHT)),
            flex_direction,
            align_items: AlignItems::Center,
            column_gap: Val::Px(PADDING),
            ..default()
        },
        children![
            // Invisible hit-zone wrapper. This is what receives pointer
            // events; it's bigger than the visual dot so dragging
            // connections doesn't require pixel-perfect aim.
            (
                Node {
                    width: Val::Px(TERMINAL_HIT_SIZE),
                    height: Val::Px(TERMINAL_HIT_SIZE),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                GraphTerminalView {
                    node: node_entity,
                    direction,
                    index,
                },
                Pickable::default(),
                children![(
                    // Visual dot. No Pickable; the parent hit-zone owns
                    // events, so pointer events consistently target the
                    // wrapper regardless of whether the click landed on
                    // the dot itself or its small margin.
                    Node {
                        width: Val::Px(TERMINAL_DOT_SIZE),
                        height: Val::Px(TERMINAL_DOT_SIZE),
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::all(Val::Px(TERMINAL_DOT_SIZE * 0.5)),
                        ..default()
                    },
                    BorderColor::all(Color::srgba(0.0, 0.0, 0.0, 0.5)),
                    BackgroundColor(color),
                    Pickable::IGNORE,
                )],
            ),
            (
                Text::new(label),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(Color::srgb(0.75, 0.75, 0.78)),
            ),
        ],
    )
}

fn body_area(node_entity: Entity) -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            padding: UiRect::all(Val::Px(PADDING)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(4.0),
            ..default()
        },
        GraphNodeBody { node: node_entity },
    )
}

/// Convenience bundle for a small body label; consumer code can use this
/// to render read-only values inside a node without having to restate the
/// font/color tokens every time. Matches Jackdaw's typography via
/// `jackdaw_feathers::tokens` so it sits consistently next to
/// `text_edit()` fields spawned from the same crate.
pub fn body_label(text: impl Into<String>) -> impl Bundle {
    (
        Text::new(text.into()),
        TextFont {
            font_size: jackdaw_feathers::tokens::FONT_SM,
            ..default()
        },
        TextColor(jackdaw_feathers::tokens::TEXT_SECONDARY),
    )
}

/// Push the latest [`GraphNode::position`] into the node UI's
/// `Node::left`/`Node::top`.
///
/// Runs each frame under `Update` so moving a node updates on the next
/// render. Phase 2's drag system mutates `GraphNode::position` through
/// `SetJsnField` commands, which triggers this to repaint.
pub fn apply_node_position(
    nodes: Query<&GraphNode>,
    mut views: Query<(&GraphNodeView, &mut Node)>,
) {
    for (view, mut node) in views.iter_mut() {
        let Ok(graph_node) = nodes.get(view.0) else {
            continue;
        };
        node.left = Val::Px(graph_node.position.x);
        node.top = Val::Px(graph_node.position.y);
    }
}

/// Update each node UI's border color based on whether its data entity
/// carries a [`GraphNodeSelected`] marker.
///
/// Runs every frame in `Update`. Cheap even with thousands of nodes ;
/// Phase 6 polish can switch to `Added<GraphNodeSelected>` +
/// `RemovedComponents` change detection if profiling shows overhead.
pub fn apply_selection_highlight(
    selected: Query<(), With<GraphNodeSelected>>,
    mut views: Query<(&GraphNodeView, &mut BorderColor)>,
) {
    for (view, mut border) in views.iter_mut() {
        let target = if selected.contains(view.0) {
            SELECTED_BORDER
        } else {
            UNSELECTED_BORDER
        };
        *border = BorderColor::all(target);
    }
}

/// Spawn `Terminal` components on `node_entity` to match a descriptor.
///
/// Caller is responsible for calling this once at node creation time; it
/// writes one child `Terminal` entity per input and per output. This is a
/// helper used by `AddGraphNodeCmd` in Phase 2 and by the demo example.
pub fn spawn_terminal_components(
    commands: &mut Commands,
    node_entity: Entity,
    descriptor: &NodeTypeDescriptor,
) {
    for (idx, input) in descriptor.inputs.iter().enumerate() {
        commands.spawn((
            Terminal {
                direction: TerminalDirection::Input,
                data_type: input.data_type.clone(),
                label: input.label.clone(),
                index: idx as u32,
            },
            ChildOf(node_entity),
        ));
    }
    for (idx, output) in descriptor.outputs.iter().enumerate() {
        commands.spawn((
            Terminal {
                direction: TerminalDirection::Output,
                data_type: output.data_type.clone(),
                label: output.label.clone(),
                index: idx as u32,
            },
            ChildOf(node_entity),
        ));
    }
}
