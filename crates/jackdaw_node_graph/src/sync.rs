//! Bridges data-layer lifecycle to UI-layer lifecycle.
//!
//! Commands from [`crate::commands`] only mutate data components
//! (`GraphNode`, `Connection`, `Terminal`). The systems in this module watch
//! for those lifecycle events and keep the UI tree in sync:
//!
//! * When a `GraphCanvasWorld` entity appears, register its owner graph in
//!   the `CanvasWorldIndex` so other sync systems know where to parent UI.
//! * When a `GraphNode` is added, spawn the node UI under the matching
//!   canvas world.
//! * When a `GraphNode` is removed, despawn the UI entity that views it.
//! * Same pattern for `Connection`.

use bevy::ecs::relationship::Relationship;
use bevy::prelude::*;
use std::collections::HashMap;

use crate::canvas::{GraphCanvasViewport, GraphCanvasWorld};
use crate::connection::ConnectionView;
use crate::graph::{Connection, GraphNode};
use crate::materials::ConnectionMaterial;
use crate::node_widget::{GraphNodeView, node};
use crate::registry::NodeTypeRegistry;

/// Maps a graph entity to its UI entities: the inner `GraphCanvasWorld`
/// (parent of node UI) and the outer `GraphCanvasViewport` (parent of
/// wire overlay UI).
#[derive(Resource, Default, Debug)]
pub struct CanvasWorldIndex {
    pub graph_to_world: HashMap<Entity, Entity>,
    pub graph_to_viewport: HashMap<Entity, Entity>,
}

/// Populate [`CanvasWorldIndex`] as canvas viewports and worlds are spawned.
pub fn index_canvas_worlds(
    mut index: ResMut<CanvasWorldIndex>,
    added_worlds: Query<(Entity, &GraphCanvasWorld), Added<GraphCanvasWorld>>,
    added_viewports: Query<(Entity, &GraphCanvasViewport), Added<GraphCanvasViewport>>,
) {
    for (ui_entity, world) in added_worlds.iter() {
        index.graph_to_world.insert(world.graph, ui_entity);
    }
    for (ui_entity, viewport) in added_viewports.iter() {
        index.graph_to_viewport.insert(viewport.graph, ui_entity);
    }
}

/// Clean up [`CanvasWorldIndex`] entries whose canvas UI was despawned.
pub fn prune_canvas_world_index(
    mut index: ResMut<CanvasWorldIndex>,
    mut removed_worlds: RemovedComponents<GraphCanvasWorld>,
    mut removed_viewports: RemovedComponents<GraphCanvasViewport>,
) {
    for entity in removed_worlds.read() {
        index
            .graph_to_world
            .retain(|_, &mut ui_entity| ui_entity != entity);
    }
    for entity in removed_viewports.read() {
        index
            .graph_to_viewport
            .retain(|_, &mut ui_entity| ui_entity != entity);
    }
}

/// Spawn a node UI subtree whenever a new `GraphNode` data entity appears.
///
/// We look up the owning graph from the `GraphNode`'s `ChildOf` relation,
/// then the canvas world from [`CanvasWorldIndex`]. If either is missing the
/// spawn is skipped; this handles the common case where the data entity is
/// created before the canvas UI.
///
/// This also backfills UI for pre-existing nodes when the canvas itself
/// just appeared; necessary for switching into a graph whose data entities
/// were spawned long ago (e.g. loaded from scene or selected from the
/// timeline dock). Without the backfill, reusing a canvas to view an
/// existing graph would leave it empty until you mutated a node.
pub fn spawn_node_ui_for_new_graph_nodes(
    added: Query<(Entity, &GraphNode, Option<&ChildOf>), Added<GraphNode>>,
    added_worlds: Query<&GraphCanvasWorld, Added<GraphCanvasWorld>>,
    all_nodes: Query<(Entity, &GraphNode, &ChildOf)>,
    views: Query<&GraphNodeView>,
    index: Res<CanvasWorldIndex>,
    registry: Res<NodeTypeRegistry>,
    mut commands: Commands,
) {
    // Path A: a new GraphNode appeared (common authoring case).
    for (data_entity, graph_node, parent) in added.iter() {
        if views.iter().any(|v| v.0 == data_entity) {
            continue;
        }
        let Some(parent) = parent else {
            continue;
        };
        let graph = parent.get();
        let Some(&world_entity) = index.graph_to_world.get(&graph) else {
            continue;
        };
        let Some(descriptor) = registry.get(&graph_node.node_type) else {
            warn!(
                "sync: no NodeTypeDescriptor for '{}', skipping UI spawn",
                graph_node.node_type
            );
            continue;
        };
        commands
            .spawn(node(data_entity, graph_node, descriptor))
            .insert(ChildOf(world_entity));
    }

    // Path B: a new canvas world just appeared; walk every pre-existing
    // GraphNode that belongs to its graph and spawn UI for any that don't
    // already have a view.
    for world in added_worlds.iter() {
        let graph = world.graph;
        let Some(&world_entity) = index.graph_to_world.get(&graph) else {
            continue;
        };
        for (data_entity, graph_node, child_of) in all_nodes.iter() {
            if child_of.get() != graph {
                continue;
            }
            if views.iter().any(|v| v.0 == data_entity) {
                continue;
            }
            let Some(descriptor) = registry.get(&graph_node.node_type) else {
                continue;
            };
            commands
                .spawn(node(data_entity, graph_node, descriptor))
                .insert(ChildOf(world_entity));
        }
    }
}

/// Despawn node UI for removed `GraphNode` data entities.
pub fn despawn_node_ui_for_removed(
    mut removed: RemovedComponents<GraphNode>,
    views: Query<(Entity, &GraphNodeView)>,
    mut commands: Commands,
) {
    if removed.is_empty() {
        return;
    }
    let removed: Vec<Entity> = removed.read().collect();
    for (ui_entity, view) in views.iter() {
        if removed.contains(&view.0)
            && let Ok(mut ec) = commands.get_entity(ui_entity)
        {
            ec.despawn();
        }
    }
}

/// Spawn connection wire UI for newly added `Connection` data entities.
///
/// Wires are parented to the canvas **viewport** (not the world) and sized
/// to 100% so they never need to resize in response to node drags. The
/// shader receives viewport-local pixel coordinates in its uniforms.
///
/// Also backfills wire UI for pre-existing connections when a canvas
/// viewport just appeared; matches the backfill in
/// [`spawn_node_ui_for_new_graph_nodes`].
pub fn spawn_connection_ui_for_new(
    added: Query<(Entity, Option<&ChildOf>), (With<Connection>, Added<Connection>)>,
    added_viewports: Query<&GraphCanvasViewport, Added<GraphCanvasViewport>>,
    all_connections: Query<(Entity, &ChildOf), With<Connection>>,
    existing: Query<&ConnectionView>,
    index: Res<CanvasWorldIndex>,
    mut materials: ResMut<Assets<ConnectionMaterial>>,
    mut commands: Commands,
) {
    let spawn_wire = |conn_entity: Entity,
                      viewport_entity: Entity,
                      materials: &mut Assets<ConnectionMaterial>,
                      commands: &mut Commands| {
        let material = materials.add(ConnectionMaterial::default());
        commands
            .spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(0.0),
                    top: Val::Px(0.0),
                    width: Val::Percent(100.0),
                    height: Val::Percent(100.0),
                    ..default()
                },
                MaterialNode(material),
                ConnectionView {
                    connection: conn_entity,
                },
                Pickable::IGNORE,
            ))
            .insert(ChildOf(viewport_entity));
    };

    // Path A: new Connection data entity.
    for (conn_entity, parent) in added.iter() {
        if existing.iter().any(|v| v.connection == conn_entity) {
            continue;
        }
        let Some(parent) = parent else {
            continue;
        };
        let graph = parent.get();
        let Some(&viewport_entity) = index.graph_to_viewport.get(&graph) else {
            continue;
        };
        spawn_wire(conn_entity, viewport_entity, &mut materials, &mut commands);
    }

    // Path B: a new canvas viewport just appeared; backfill wires for
    // every existing Connection under its graph.
    for viewport in added_viewports.iter() {
        let graph = viewport.graph;
        let Some(&viewport_entity) = index.graph_to_viewport.get(&graph) else {
            continue;
        };
        for (conn_entity, child_of) in all_connections.iter() {
            if child_of.get() != graph {
                continue;
            }
            if existing.iter().any(|v| v.connection == conn_entity) {
                continue;
            }
            spawn_wire(conn_entity, viewport_entity, &mut materials, &mut commands);
        }
    }
}

/// Despawn connection UI for removed `Connection` data entities.
pub fn despawn_connection_ui_for_removed(
    mut removed: RemovedComponents<Connection>,
    views: Query<(Entity, &ConnectionView)>,
    mut commands: Commands,
) {
    if removed.is_empty() {
        return;
    }
    let removed: Vec<Entity> = removed.read().collect();
    for (ui_entity, view) in views.iter() {
        if removed.contains(&view.connection)
            && let Ok(mut ec) = commands.get_entity(ui_entity)
        {
            ec.despawn();
        }
    }
}
