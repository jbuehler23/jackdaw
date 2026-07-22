//! Shared layered graph renderer for the debugger's graph-shaped panels
//! (System Graph first, Relationships later): a pure layout function plus
//! the UI-node + `UiMaterial` wire renderer that draws it.
//!
//! `layered_layout` assigns each node a `(layer, row)` slot from a longest
//! dependency path; `spawn_graph` turns that into absolutely-positioned node
//! boxes and `GraphEdgeMaterial` wires. `sync_graph_edges` keeps each wire's
//! endpoints glued to its two node boxes every frame, adapted from
//! `jackdaw_node_graph::connection::update_connection_endpoints`.

use std::collections::{HashMap, HashSet};

use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::*;
use bevy::shader::ShaderRef;
use bevy::ui::UiGlobalTransform;

use jackdaw_feathers::tokens;

// --------------------------- Layout ---------------------------

/// A node's position in the layered layout: `layer` is its column (the
/// longest dependency chain reaching it), `row` its position within that
/// column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct NodeSlot {
    pub layer: usize,
    pub row: usize,
}

/// Longest-path layering: `edge (from, to)` means `from` runs before `to`,
/// so `layer[to] >= layer[from] + 1`. Relaxes edges until stable, capped at
/// `node_count` passes so a cycle (which can never converge) still
/// terminates rather than looping forever. Nodes with no incoming edges land
/// in layer 0. `row` is assigned by ascending node index within each layer,
/// so it is stable across calls and unique within a layer. Pure, no Bevy.
pub fn layered_layout(node_count: usize, edges: &[(usize, usize)]) -> Vec<NodeSlot> {
    let mut layer = vec![0usize; node_count];
    for _ in 0..node_count.max(1) {
        let mut changed = false;
        for &(from, to) in edges {
            if from >= node_count || to >= node_count {
                continue;
            }
            let candidate = layer[from] + 1;
            if candidate > layer[to] {
                layer[to] = candidate;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut next_row: HashMap<usize, usize> = HashMap::new();
    layer
        .into_iter()
        .map(|l| {
            let row = next_row.entry(l).or_insert(0);
            let slot = NodeSlot {
                layer: l,
                row: *row,
            };
            *row += 1;
            slot
        })
        .collect()
}

// --------------------------- Edge material ---------------------------

const SHADER_GRAPH_EDGE_PATH: &str = "embedded://jackdaw/remote/debug/shaders/graph_edge.wgsl";

/// GPU material for one graph edge wire: a copy of
/// `jackdaw_node_graph::materials::ConnectionMaterial`. Not reused directly:
/// `embedded_asset!` derives its registered path from the crate the macro is
/// invoked in, so `ConnectionMaterial`'s hardcoded shader path only resolves
/// if `jackdaw_node_graph::NodeGraphPlugin` (with its own node-editing
/// systems/resources) is added; copying the material + shader here keeps
/// this debug plugin self-contained, matching `sparkline.rs`'s precedent for
/// a bespoke `UiMaterial` in this module.
#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
pub struct GraphEdgeMaterial {
    #[uniform(0)]
    pub p0: Vec2,
    #[uniform(0)]
    pub p1: Vec2,
    #[uniform(0)]
    pub p2: Vec2,
    #[uniform(0)]
    pub p3: Vec2,
    #[uniform(0)]
    pub color: Vec4,
    #[uniform(0)]
    pub width: f32,
    #[uniform(0)]
    pub feather: f32,
}

impl Default for GraphEdgeMaterial {
    fn default() -> Self {
        Self {
            p0: Vec2::ZERO,
            p1: Vec2::ZERO,
            p2: Vec2::ZERO,
            p3: Vec2::ZERO,
            color: Vec4::new(0.6, 0.6, 0.7, 0.7),
            width: 1.5,
            feather: 1.0,
        }
    }
}

impl UiMaterial for GraphEdgeMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_GRAPH_EDGE_PATH.into()
    }
}

fn color_to_vec4(color: Color) -> Vec4 {
    let c = color.to_linear();
    Vec4::new(c.red, c.green, c.blue, c.alpha)
}

// --------------------------- Spawn ---------------------------

/// One node to render: its display label. Ambiguity highlighting is derived
/// from the `ambiguities` array passed to [`spawn_graph`], not carried here.
pub struct GraphNodeSpec {
    pub label: String,
}

/// Tag on a spawned node box. `index` matches its position in the
/// `nodes`/`edges`/`ambiguities` arrays passed to [`spawn_graph`].
#[derive(Component, Debug, Clone, Copy)]
pub struct GraphNodeBox {
    pub index: usize,
}

/// Tag on a spawned edge wire's material node.
#[derive(Component, Debug, Clone, Copy)]
pub struct GraphEdgeLink {
    pub from: usize,
    pub to: usize,
    pub ambiguous: bool,
}

/// Tag on the graph's content root. Node boxes and edge wires are both
/// parented here; [`sync_graph_edges`] treats it as the "viewport" whose
/// top-left is subtracted from each box's screen position to get wire
/// endpoints in the material's own local pixel space, the same trick
/// `jackdaw_node_graph::connection` uses for `GraphCanvasViewport`.
#[derive(Component)]
pub struct GraphCanvasContent;

/// Hard cap on drawn edges (ordering + ambiguity combined). Each edge is a
/// full-content-sized `UiMaterial` draw call, and a schedule can have well
/// over this many dependency and ambiguity edges combined.
pub const MAX_EDGES: usize = 120;

const COL_W: f32 = 220.0;
const ROW_H: f32 = 48.0;
const NODE_W: f32 = 190.0;

/// Build a scrollable, layered node graph as a child of `parent`: one box
/// per `nodes` entry positioned by [`layered_layout`], plus one wire per
/// ordering edge and per ambiguity, ordering edges first, capped at
/// [`MAX_EDGES`] total. Returns the scroll container entity.
pub fn spawn_graph(
    commands: &mut Commands,
    materials: &mut Assets<GraphEdgeMaterial>,
    parent: Entity,
    nodes: &[GraphNodeSpec],
    edges: &[(usize, usize)],
    ambiguities: &[(usize, usize)],
) -> Entity {
    let slots = layered_layout(nodes.len(), edges);
    let max_layer = slots.iter().map(|s| s.layer).max().unwrap_or(0);
    let mut rows_in_layer: HashMap<usize, usize> = HashMap::new();
    for slot in &slots {
        let count = rows_in_layer.entry(slot.layer).or_insert(0);
        *count = (*count).max(slot.row + 1);
    }
    let max_rows = rows_in_layer.values().copied().max().unwrap_or(0).max(1);

    let content_width = (max_layer as f32 + 1.0) * COL_W + NODE_W;
    let content_height = (max_rows as f32 + 1.0) * ROW_H;

    let scroll_container = commands
        .spawn((
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                overflow: Overflow::scroll(),
                ..default()
            },
            ChildOf(parent),
        ))
        .id();

    let content = commands
        .spawn((
            GraphCanvasContent,
            Node {
                width: Val::Px(content_width),
                height: Val::Px(content_height),
                ..default()
            },
            ChildOf(scroll_container),
        ))
        .id();

    let ambiguous_nodes: HashSet<usize> = ambiguities.iter().flat_map(|&(a, b)| [a, b]).collect();

    for (index, spec) in nodes.iter().enumerate() {
        let slot = slots[index];
        let left = ROW_H * 0.5 + slot.layer as f32 * COL_W;
        let top = ROW_H * 0.5 + slot.row as f32 * ROW_H;
        let border = if ambiguous_nodes.contains(&index) {
            Color::Srgba(tokens::DESTRUCTIVE_RED)
        } else {
            tokens::BORDER_SUBTLE
        };
        commands.spawn((
            GraphNodeBox { index },
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(left),
                top: Val::Px(top),
                width: Val::Px(NODE_W),
                padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(tokens::SPACING_XS)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(tokens::CORNER_RADIUS),
                ..default()
            },
            BackgroundColor(tokens::COMPONENT_CARD_BG),
            BorderColor::all(border),
            children![(
                Text::new(spec.label.clone()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
            ChildOf(content),
        ));
    }

    let ordering_color = color_to_vec4(tokens::BORDER_STRONG.with_alpha(0.8));
    let ambiguity_color = color_to_vec4(Color::Srgba(tokens::DESTRUCTIVE_RED));

    let mut drawn = 0usize;
    for &(from, to) in edges {
        if drawn >= MAX_EDGES {
            break;
        }
        spawn_edge(
            commands,
            materials,
            content,
            from,
            to,
            false,
            ordering_color,
        );
        drawn += 1;
    }
    for &(a, b) in ambiguities {
        if drawn >= MAX_EDGES {
            break;
        }
        spawn_edge(commands, materials, content, a, b, true, ambiguity_color);
        drawn += 1;
    }

    scroll_container
}

fn spawn_edge(
    commands: &mut Commands,
    materials: &mut Assets<GraphEdgeMaterial>,
    parent: Entity,
    from: usize,
    to: usize,
    ambiguous: bool,
    color: Vec4,
) {
    let material = materials.add(GraphEdgeMaterial {
        color,
        width: if ambiguous { 2.5 } else { 1.5 },
        ..default()
    });
    commands.spawn((
        GraphEdgeLink {
            from,
            to,
            ambiguous,
        },
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        MaterialNode(material),
        Pickable::IGNORE,
        ChildOf(parent),
    ));
}

// --------------------------- Endpoint sync ---------------------------

/// Recompute every [`GraphEdgeLink`]'s material endpoints from its two node
/// boxes' current screen positions.
///
/// Runs in `PostUpdate` after `UiSystems::Layout`, so `UiGlobalTransform` is
/// fresh for the current frame; a no-op while no graph is spawned (no
/// `GraphCanvasContent` in the world). Adapted from
/// `jackdaw_node_graph::connection::update_connection_endpoints`: each edge's
/// material node is sized to 100% of `GraphCanvasContent` and parented
/// directly under it, so subtracting that content root's top-left from a
/// node box's `UiGlobalTransform` center gives coordinates in the material's
/// own local pixel space.
pub fn sync_graph_edges(
    mut materials: ResMut<Assets<GraphEdgeMaterial>>,
    edge_links: Query<(&GraphEdgeLink, &MaterialNode<GraphEdgeMaterial>)>,
    boxes: Query<(&GraphNodeBox, &UiGlobalTransform)>,
    content_roots: Query<(&ComputedNode, &UiGlobalTransform), With<GraphCanvasContent>>,
) {
    let Some((content_computed, content_transform)) = content_roots.iter().next() else {
        return;
    };
    let (_, _, content_center) = content_transform.to_scale_angle_translation();
    let content_top_left = content_center - content_computed.size() * 0.5;

    let mut centers: HashMap<usize, Vec2> = HashMap::new();
    for (node_box, transform) in boxes.iter() {
        let (_, _, translation) = transform.to_scale_angle_translation();
        centers.insert(node_box.index, translation);
    }

    for (link, material_handle) in edge_links.iter() {
        let (Some(&from_screen), Some(&to_screen)) =
            (centers.get(&link.from), centers.get(&link.to))
        else {
            continue;
        };

        let from_local = from_screen - content_top_left;
        let to_local = to_screen - content_top_left;

        let dx = (to_local.x - from_local.x).abs().max(40.0);
        let p1 = from_local + Vec2::new(dx * 0.5, 0.0);
        let p2 = to_local - Vec2::new(dx * 0.5, 0.0);

        let Some(mut material) = materials.get_mut(&material_handle.0) else {
            continue;
        };
        material.p0 = from_local;
        material.p1 = p1;
        material.p2 = p2;
        material.p3 = to_local;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diamond_layers_by_longest_path() {
        let slots = layered_layout(4, &[(0, 1), (0, 2), (1, 3), (2, 3)]);
        let layers: Vec<usize> = slots.iter().map(|s| s.layer).collect();
        assert_eq!(layers, vec![0, 1, 1, 2]);
    }

    #[test]
    fn linear_chain_layers_sequentially() {
        let slots = layered_layout(3, &[(0, 1), (1, 2)]);
        let layers: Vec<usize> = slots.iter().map(|s| s.layer).collect();
        assert_eq!(layers, vec![0, 1, 2]);
    }

    #[test]
    fn isolated_node_is_layer_zero() {
        let slots = layered_layout(1, &[]);
        assert_eq!(slots[0].layer, 0);
    }

    #[test]
    fn rows_are_unique_within_a_layer() {
        // Five nodes, all layer 0 (no edges): each must get a distinct row.
        let slots = layered_layout(5, &[]);
        let mut rows: Vec<usize> = slots.iter().map(|s| s.row).collect();
        rows.sort_unstable();
        assert_eq!(rows, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn cycle_terminates_instead_of_looping_forever() {
        // A self-referential cycle can never converge; the pass cap must
        // still return promptly with some (bounded) layer assignment.
        let slots = layered_layout(3, &[(0, 1), (1, 2), (2, 0)]);
        assert_eq!(slots.len(), 3);
    }

    #[test]
    fn out_of_range_edges_are_ignored() {
        let slots = layered_layout(2, &[(0, 1), (0, 5), (5, 1)]);
        let layers: Vec<usize> = slots.iter().map(|s| s.layer).collect();
        assert_eq!(layers, vec![0, 1]);
    }
}
