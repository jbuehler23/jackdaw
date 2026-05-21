//! Generic node graph editor for the Jackdaw editor.
//!
//! Provides a pannable/zoomable canvas hosting typed nodes connected by cubic
//! Bezier wires. The crate is domain-agnostic: consumer crates register their
//! own node types via [`NodeTypeRegistry`] to build animation graphs, shader
//! graphs, material graphs, etc.
//!
//! # Architecture
//! - Graph data lives as ECS entities with reflected components so it
//!   round-trips through the editor's JSN AST serializer.
//! - Canvas rendering uses a `UiMaterial` + WGSL shader for GPU-accelerated
//!   anti-aliased Bezier wires (Inigo Quilez quadratic SDF decomposition).
//! - Interaction is driven by a [`GraphGesture`] state machine that consumes
//!   pointer events via observers and pushes `EditorCommand`s onto the shared
//!   `CommandHistory` for undo/redo.

pub mod add_node_popover;
pub mod canvas;
pub mod commands;
pub mod connection;
pub mod gesture;
pub mod graph;
pub mod interaction;
pub mod materials;
pub mod node_widget;
pub mod registry;
pub mod selection;
pub mod sync;

use bevy_ui_render::UiMaterialPlugin;
pub use canvas::{GraphCanvasViewport, GraphCanvasWorld, canvas, canvas_world};
pub use commands::{
    AddGraphNodeCmd, CreateConnectionCmd, MoveGraphNodesCmd, RemoveConnectionCmd,
    RemoveGraphNodesCmd,
};
pub use connection::{GhostConnection, PendingRemove, TerminalAnchor, update_connection_endpoints};
pub use gesture::{ConnectionAnchor, GraphGesture};
pub use graph::{
    Connection, GraphCanvasView, GraphNode, GraphNodeSelected, NodeGraph, Terminal,
    TerminalDirection,
};
pub use materials::ConnectionMaterial;
pub use node_widget::{GraphNodeBody, GraphNodeView, body_label, node};
pub use registry::{NodeTypeDescriptor, NodeTypeRegistry, TerminalDescriptor};
pub use selection::GraphSelection;
pub use sync::CanvasWorldIndex;

use bevy_app::prelude::*;
use bevy_asset::embedded_asset;
use bevy_ecs::prelude::*;
use jackdaw_commands::CommandHistory;

/// Registers all node-graph types, resources, systems, and assets.
pub struct NodeGraphPlugin;

impl Plugin for NodeGraphPlugin {
    fn build(&self, app: &mut App) {
        // Embed the connection SDF shader.
        embedded_asset!(app, "shaders/connection.wgsl");

        // Reflect types so they round-trip through the JSN AST.
        app.register_type::<NodeGraph>()
            .register_type::<GraphCanvasView>()
            .register_type::<GraphNode>()
            .register_type::<Terminal>()
            .register_type::<TerminalDirection>()
            .register_type::<Connection>();

        // Resources.
        app.init_resource::<NodeTypeRegistry>()
            .init_resource::<GraphSelection>()
            .init_resource::<GraphGesture>()
            .init_resource::<CanvasWorldIndex>()
            .init_resource::<CommandHistory>();

        // Connection rendering pipeline (UiMaterial + SDF shader).
        app.add_plugins(UiMaterialPlugin::<ConnectionMaterial>::default());

        // Per-frame systems.
        app.add_systems(
            Update,
            (
                canvas::apply_canvas_view,
                canvas::handle_canvas_pan_zoom,
                node_widget::apply_node_position,
                node_widget::apply_selection_highlight,
                connection::update_pending_remove_markers,
                // Keyboard.
                interaction::handle_delete_key,
                // Note: undo/redo is handled globally by the main editor's
                // `handle_undo_redo_keys` system against the same shared
                // `CommandHistory`. Having a duplicate handler here caused
                // Ctrl+Z to pop two commands at once (issue seen when drawing
                // brushes + applying material: undo removed the material AND
                // the last brush).
                add_node_popover::handle_tab_quick_add,
                add_node_popover::handle_popover_escape,
            ),
        );
        // UI lifecycle sync systems must run in order so the canvas index
        // is populated before nodes/connections try to consult it.
        app.add_systems(
            Update,
            (
                sync::index_canvas_worlds,
                sync::prune_canvas_world_index,
                (
                    sync::spawn_node_ui_for_new_graph_nodes,
                    sync::spawn_connection_ui_for_new,
                ),
                (
                    sync::despawn_node_ui_for_removed,
                    sync::despawn_connection_ui_for_removed,
                ),
            )
                .chain(),
        );
        app.add_systems(
            PostUpdate,
            (
                connection::update_connection_endpoints,
                connection::update_ghost_wire,
            )
                .after(bevy_ui::UiSystems::Layout),
        );

        // Pointer observers for gesture handling. Terminal observers fire
        // before node observers so propagation can be stopped on the
        // handled path before the event bubbles to the node root.
        app.add_observer(interaction::on_terminal_click)
            .add_observer(interaction::on_terminal_right_click)
            .add_observer(interaction::on_terminal_drag_start)
            .add_observer(interaction::on_terminal_drag)
            .add_observer(interaction::on_terminal_drag_end)
            .add_observer(interaction::on_node_click)
            .add_observer(interaction::on_node_drag_start)
            .add_observer(interaction::on_node_drag)
            .add_observer(interaction::on_node_drag_end)
            .add_observer(add_node_popover::on_canvas_right_click)
            .add_observer(add_node_popover::on_entry_click)
            .add_observer(add_node_popover::on_entry_over)
            .add_observer(add_node_popover::on_entry_out)
            .add_observer(add_node_popover::on_backdrop_click);
    }
}
