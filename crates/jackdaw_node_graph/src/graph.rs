//! Core data model for the node graph.
//!
//! All types here are reflected and (de)serializable so they round-trip
//! through the editor's JSN AST without special-casing. Connections carry
//! `Entity` fields which the JSN serializer rewrites to scene-local indices
//! on save and resolves back to live entities on load.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Root marker for a node graph. An entity with this component owns
/// `GraphNode` and `Connection` child entities.
#[derive(Component, Reflect, Clone, Debug, Serialize, Deserialize, Default)]
#[reflect(Component)]
pub struct NodeGraph {
    /// Human-readable title shown in the breadcrumb / graph list.
    pub title: String,
}

/// Pan/zoom state for a graph's canvas viewport.
///
/// Stored on the [`NodeGraph`] entity so per-graph view state persists
/// when switching graphs. Applied to the canvas world node each frame.
#[derive(Component, Reflect, Clone, Debug, Serialize, Deserialize)]
#[reflect(Component)]
pub struct GraphCanvasView {
    /// Pan offset in canvas pixels.
    pub offset: Vec2,
    /// Zoom multiplier; clamped to `MIN_ZOOM..=MAX_ZOOM`.
    pub zoom: f32,
}

impl Default for GraphCanvasView {
    fn default() -> Self {
        Self {
            offset: Vec2::ZERO,
            zoom: 1.0,
        }
    }
}

/// A node instance in a graph.
///
/// `node_type` keys into the [`NodeTypeRegistry`](crate::NodeTypeRegistry) for
/// the node's visual and terminal schema. Domain-specific parameters live on
/// sibling components spawned alongside the `GraphNode`; consumer crates add
/// whatever reflected components they need and edit them through the existing
/// inspector reflect-field UI.
#[derive(Component, Reflect, Clone, Debug, Serialize, Deserialize, Default)]
#[reflect(Component)]
pub struct GraphNode {
    /// Registry key identifying the node type (e.g. `"anim.state"`).
    pub node_type: String,
    /// Position in canvas-space pixels, top-left corner.
    pub position: Vec2,
}

/// Marker component added to a selected [`GraphNode`] by [`GraphSelection`](crate::GraphSelection).
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct GraphNodeSelected;

/// An input or output port on a [`GraphNode`].
///
/// Terminals are child entities of their owning node. Their UI is created by
/// the [`node()`](crate::node) bundle function according to the descriptor in
/// the registry.
#[derive(Component, Reflect, Clone, Debug, Serialize, Deserialize)]
#[reflect(Component)]
pub struct Terminal {
    pub direction: TerminalDirection,
    /// Data-type name used for connection compatibility checks.
    pub data_type: String,
    pub label: String,
    /// Stable ordering within the owning node.
    pub index: u32,
}

/// Whether a terminal accepts incoming or produces outgoing data.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TerminalDirection {
    #[default]
    Input,
    Output,
}

/// An edge between two terminals in a graph.
///
/// Stored as a sibling entity under the owning `NodeGraph` so it serializes
/// with the scene. The JSN serializer rewrites `Entity` fields to scene-local
/// indices (see `src/scene_io.rs` `JsnSerializerProcessor`).
#[derive(Component, Reflect, Clone, Debug, Serialize, Deserialize)]
#[reflect(Component)]
pub struct Connection {
    /// The `GraphNode` entity that owns the source terminal.
    pub source_node: Entity,
    /// Stable index of the source terminal within its node.
    pub source_terminal: u32,
    /// The `GraphNode` entity that owns the target terminal.
    pub target_node: Entity,
    /// Stable index of the target terminal within its node.
    pub target_terminal: u32,
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            source_node: Entity::PLACEHOLDER,
            source_terminal: 0,
            target_node: Entity::PLACEHOLDER,
            target_terminal: 0,
        }
    }
}

/// Minimum allowed canvas zoom.
pub const MIN_ZOOM: f32 = 0.25;

/// Maximum allowed canvas zoom.
pub const MAX_ZOOM: f32 = 4.0;
