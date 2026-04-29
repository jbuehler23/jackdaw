//! Connection wire rendering.
//!
//! Each `Connection` component has a sibling UI node carrying a
//! [`ConnectionMaterial`]. The material node is sized to 100% of the
//! viewport and parented directly under it; **not** under the canvas
//! world; so it never needs to resize in response to node drags. This
//! avoids a one-frame lag between writing `Node::width/height` in
//! `PostUpdate` (after Layout) and seeing the new layout applied (only on
//! the next frame).
//!
//! Every frame (after `UiSystems::Layout`, so all UI transforms are fresh)
//! we look up the source and target terminal positions via their
//! [`UiGlobalTransform`], subtract the viewport origin to get
//! viewport-local pixels, and push those into the material uniforms. The
//! shader renders the cubic Bezier in viewport-local pixel space via
//! `in.uv * in.size`; its natural coordinate system when the node covers
//! the whole viewport.
//!
//! Stroke width is in screen pixels and does not scale with canvas zoom.

use bevy::ecs::relationship::Relationship;
use bevy::prelude::*;
use bevy::ui::UiGlobalTransform;
use std::collections::HashSet;

use crate::canvas::GraphCanvasViewport;
use crate::gesture::{ConnectionAnchor, GraphGesture, SnapHit};
use crate::graph::{Connection, GraphNode, TerminalDirection};
use crate::materials::ConnectionMaterial;
use crate::node_widget::GraphTerminalView;
use crate::registry::NodeTypeRegistry;
use crate::sync::CanvasWorldIndex;

/// Sibling marker for the UI node that renders a [`Connection`].
#[derive(Component, Debug, Clone, Copy)]
pub struct ConnectionView {
    pub connection: Entity,
}

/// Marker on the temporary ghost wire spawned while the user is dragging a
/// new connection out of an output terminal. Separate from [`ConnectionView`]
/// so it never collides with a real wire and can be despawned as a unit
/// when the drag ends.
#[derive(Component, Debug, Clone, Copy)]
pub struct GhostConnection;

/// Marker added to every `Connection` data entity whose source or target
/// terminal is currently being hovered with Right Click held down. The
/// connection renderer fades wires carrying this marker to signal they'll
/// be removed on click.
#[derive(Component, Debug, Clone, Copy)]
pub struct PendingRemove;

/// Anchor a UI node on top of a canvas terminal dot; helper return type
/// used by Phase 6's connection-endpoint editing. Not used in Phase 1/2.
#[derive(Debug, Clone, Copy)]
pub struct TerminalAnchor {
    pub world_pos: Vec2,
}

/// Snap radius for the drag-to-connect gesture, in physical pixels.
///
/// The terminal hit zone is 16 logical pixels wide; at 1x DPI this gives
/// the user a ~half-hit-zone extra slack around each input, and the value
/// scales naturally with DPI because the terminal positions are in
/// physical pixels too.
const SNAP_RADIUS_PX: f32 = 40.0;

/// Full-opacity wire color.
const WIRE_COLOR: Vec4 = Vec4::new(0.6, 0.6, 0.7, 1.0);
/// Dimmed wire color applied when a connection has a [`PendingRemove`]
/// marker (right+hover on one of its endpoints).
const WIRE_COLOR_PENDING_REMOVE: Vec4 = Vec4::new(0.95, 0.35, 0.35, 0.45);
/// Ghost color when the drag is free (no snap target yet). Warm
/// translucent gray signals "free-floating end".
const GHOST_COLOR_FREE: Vec4 = Vec4::new(0.85, 0.85, 0.9, 0.7);
/// Ghost color once snapped to a compatible input. Bright cyan-green that
/// reads as "yes, releasing here will create a connection".
const GHOST_COLOR_SNAPPED: Vec4 = Vec4::new(0.3, 0.95, 0.7, 1.0);

/// Recompute every `ConnectionView`'s material uniforms.
///
/// Runs in `PostUpdate` after `UiSystems::Layout`, so `UiGlobalTransform`
/// values are fresh for the current frame. Because the wire's UI node is
/// sized to the viewport and never resized, writes to `material.*` uniforms
/// take effect for the same frame's render; no lag.
///
/// Coordinate-system notes (these tripped me up and are easy to get wrong):
/// * `UiGlobalTransform::translation` is the **center** of the node in
///   **physical** pixels (`ComputedNode::size` is also physical).
/// * The shader's `in.uv * in.size` is in **top-left-origin** physical
///   pixels, so we must convert terminal centers to `terminal_center -
///   viewport_top_left` (where `top_left = center - size / 2`).
/// * The wire material node sits at the same rect as the viewport (100%
///   width/height, position 0,0), so its local frame matches the viewport
///   frame exactly.
pub fn update_connection_endpoints(
    mut materials: ResMut<Assets<ConnectionMaterial>>,
    connection_nodes: Query<(&ConnectionView, &MaterialNode<ConnectionMaterial>)>,
    connections: Query<&Connection>,
    pending_remove: Query<(), With<PendingRemove>>,
    terminals: Query<(&GraphTerminalView, &UiGlobalTransform)>,
    viewports: Query<(Entity, &ComputedNode, &UiGlobalTransform), With<GraphCanvasViewport>>,
) {
    let Some((_viewport_entity, viewport_computed, viewport_transform)) = viewports.iter().next()
    else {
        return;
    };
    let (_scale, _angle, viewport_center) = viewport_transform.to_scale_angle_translation();
    let viewport_size = viewport_computed.size();
    let viewport_top_left = viewport_center - viewport_size * 0.5;

    for (view, material_handle) in connection_nodes.iter() {
        let Ok(connection) = connections.get(view.connection) else {
            continue;
        };

        let Some(source_screen) = terminal_screen_pos(
            &terminals,
            connection.source_node,
            TerminalDirection::Output,
            connection.source_terminal,
        ) else {
            continue;
        };
        let Some(target_screen) = terminal_screen_pos(
            &terminals,
            connection.target_node,
            TerminalDirection::Input,
            connection.target_terminal,
        ) else {
            continue;
        };

        // Terminal centers → wire-local pixels (origin at viewport top-left).
        let source_local = source_screen - viewport_top_left;
        let target_local = target_screen - viewport_top_left;

        // Cubic Bezier control points: horizontal handles whose length is
        // half the x-distance. Gives the familiar Unreal/Blender S-wire.
        let dx = (target_local.x - source_local.x).abs().max(40.0);
        let p1 = source_local + Vec2::new(dx * 0.5, 0.0);
        let p2 = target_local - Vec2::new(dx * 0.5, 0.0);

        let Some(material) = materials.get_mut(&material_handle.0) else {
            continue;
        };
        material.p0 = source_local;
        material.p1 = p1;
        material.p2 = p2;
        material.p3 = target_local;
        material.color = if pending_remove.contains(view.connection) {
            WIRE_COLOR_PENDING_REMOVE
        } else {
            WIRE_COLOR
        };
    }
}

/// Spawn/update/despawn the ghost wire during a connect-drag, and keep the
/// gesture's `snap_target` in sync with whichever compatible input
/// terminal is closest to the cursor.
///
/// Runs in `PostUpdate` after `UiSystems::Layout`; same constraints as
/// [`update_connection_endpoints`].
pub fn update_ghost_wire(
    mut gesture: ResMut<GraphGesture>,
    mut materials: ResMut<Assets<ConnectionMaterial>>,
    terminal_transforms: Query<(&GraphTerminalView, &UiGlobalTransform)>,
    graph_nodes: Query<&GraphNode>,
    registry: Res<NodeTypeRegistry>,
    viewports: Query<(&ComputedNode, &UiGlobalTransform), With<GraphCanvasViewport>>,
    index: Res<CanvasWorldIndex>,
    node_parents: Query<&ChildOf, With<GraphNode>>,
    ghost_query: Query<(Entity, &MaterialNode<ConnectionMaterial>), With<GhostConnection>>,
    mut commands: Commands,
) {
    // If we're not in a connect-drag, make sure no ghost wire is lying
    // around and bail.
    let is_connecting = matches!(*gesture, GraphGesture::ConnectDrag { .. });
    if !is_connecting {
        for (entity, _) in ghost_query.iter() {
            if let Ok(mut ec) = commands.get_entity(entity) {
                ec.despawn();
            }
        }
        return;
    }

    // Destructure via mutable match so we can write back `snap_target`.
    let (source_anchor, cursor_pos) = match &*gesture {
        GraphGesture::ConnectDrag {
            source, cursor_pos, ..
        } => (*source, *cursor_pos),
        _ => return,
    };
    let (source_node, source_terminal_idx) = match source_anchor {
        ConnectionAnchor::FromOutput { node, terminal }
        | ConnectionAnchor::FromInput { node, terminal } => (node, terminal),
    };

    // The source terminal's screen-space center (physical pixels). Needed
    // to draw the ghost, and the stable "left" endpoint for the wire.
    let Some(source_screen) = terminal_screen_pos(
        &terminal_transforms,
        source_node,
        TerminalDirection::Output,
        source_terminal_idx,
    ) else {
        return;
    };

    // Look up the source terminal's data type via the registry descriptor.
    // The `GraphNode::node_type` keys into `NodeTypeRegistry` whose
    // descriptor owns the canonical `TerminalDescriptor::data_type`.
    let src_type: Option<String> = graph_nodes
        .get(source_node)
        .ok()
        .and_then(|node| registry.get(&node.node_type))
        .and_then(|desc| desc.outputs.get(source_terminal_idx as usize))
        .map(|term| term.data_type.clone());
    let Some(src_type) = src_type else {
        return;
    };

    // Closest compatible input within snap radius.
    let mut best: Option<(f32, SnapHit, Vec2)> = None;
    for (view, gt) in terminal_transforms.iter() {
        if view.direction != TerminalDirection::Input {
            continue;
        }
        // Resolve this terminal's data type via the same descriptor path.
        let Ok(target_node) = graph_nodes.get(view.node) else {
            continue;
        };
        let Some(descriptor) = registry.get(&target_node.node_type) else {
            continue;
        };
        let Some(term_desc) = descriptor.inputs.get(view.index as usize) else {
            continue;
        };
        if term_desc.data_type != src_type {
            continue;
        }
        let (_, _, translation) = gt.to_scale_angle_translation();
        let dist = translation.distance(cursor_pos);
        if dist > SNAP_RADIUS_PX {
            continue;
        }
        if best.as_ref().map(|b| dist < b.0).unwrap_or(true) {
            best = Some((
                dist,
                SnapHit {
                    node: view.node,
                    terminal: view.index,
                },
                translation,
            ));
        }
    }

    // Write back the snap target.
    if let GraphGesture::ConnectDrag { snap_target, .. } = &mut *gesture {
        *snap_target = best.as_ref().map(|b| b.1);
    }

    // Target screen pos: snap target if we have one, else the raw cursor.
    let target_screen = best.as_ref().map(|b| b.2).unwrap_or(cursor_pos);

    // Convert to viewport-local coords, same as update_connection_endpoints.
    let Some((viewport_computed, viewport_transform)) = viewports.iter().next() else {
        return;
    };
    let (_, _, viewport_center) = viewport_transform.to_scale_angle_translation();
    let viewport_size = viewport_computed.size();
    let viewport_top_left = viewport_center - viewport_size * 0.5;

    let source_local = source_screen - viewport_top_left;
    let target_local = target_screen - viewport_top_left;

    let dx = (target_local.x - source_local.x).abs().max(40.0);
    let p1 = source_local + Vec2::new(dx * 0.5, 0.0);
    let p2 = target_local - Vec2::new(dx * 0.5, 0.0);

    let color = if best.is_some() {
        GHOST_COLOR_SNAPPED
    } else {
        GHOST_COLOR_FREE
    };

    if let Some((_entity, material_handle)) = ghost_query.iter().next() {
        // Ghost already exists; just update its material.
        if let Some(material) = materials.get_mut(&material_handle.0) {
            material.p0 = source_local;
            material.p1 = p1;
            material.p2 = p2;
            material.p3 = target_local;
            material.color = color;
        }
    } else {
        // Ghost doesn't exist yet; spawn it parented to the viewport.
        // Look up the owning graph from the source node's ChildOf, then
        // find the viewport in the CanvasWorldIndex.
        let Ok(parent) = node_parents.get(source_node) else {
            return;
        };
        let graph = parent.get();
        let Some(&viewport_entity) = index.graph_to_viewport.get(&graph) else {
            return;
        };

        let material = materials.add(ConnectionMaterial {
            p0: source_local,
            p1,
            p2,
            p3: target_local,
            color,
            width: 2.5,
            feather: 1.0,
        });

        commands
            .spawn((
                GhostConnection,
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
            ))
            .insert(ChildOf(viewport_entity));
    }
}

/// Add [`PendingRemove`] to every connection touching a terminal the user
/// is currently hovering while holding Right. Removes the marker from
/// connections that no longer qualify.
///
/// Runs in `Update`. Combined with [`update_connection_endpoints`]'s
/// alpha-switching logic this fades wires the user is about to delete.
pub fn update_pending_remove_markers(
    mouse: Res<ButtonInput<MouseButton>>,
    hover_map: Res<bevy::picking::hover::HoverMap>,
    terminal_views: Query<&GraphTerminalView>,
    connections: Query<(Entity, &Connection)>,
    marked: Query<Entity, With<PendingRemove>>,
    mut commands: Commands,
) {
    let right_held = mouse.pressed(MouseButton::Right);

    let mut target_connections: HashSet<Entity> = HashSet::new();
    if right_held {
        // Collect every terminal the pointer is currently over.
        let mut hovered: Vec<(Entity, TerminalDirection, u32)> = Vec::new();
        for pointer_map in hover_map.values() {
            for &entity in pointer_map.keys() {
                if let Ok(view) = terminal_views.get(entity) {
                    hovered.push((view.node, view.direction, view.index));
                }
            }
        }
        if !hovered.is_empty() {
            for (entity, conn) in connections.iter() {
                let touches = hovered.iter().any(|(node, dir, idx)| match dir {
                    TerminalDirection::Output => {
                        conn.source_node == *node && conn.source_terminal == *idx
                    }
                    TerminalDirection::Input => {
                        conn.target_node == *node && conn.target_terminal == *idx
                    }
                });
                if touches {
                    target_connections.insert(entity);
                }
            }
        }
    }

    // Remove markers from connections that no longer qualify.
    for entity in marked.iter() {
        if !target_connections.contains(&entity)
            && let Ok(mut ec) = commands.get_entity(entity)
        {
            ec.remove::<PendingRemove>();
        }
    }
    // Add markers to new targets.
    for entity in target_connections {
        if marked.get(entity).is_err()
            && let Ok(mut ec) = commands.get_entity(entity)
        {
            ec.insert(PendingRemove);
        }
    }
}

fn terminal_screen_pos(
    terminals: &Query<(&GraphTerminalView, &UiGlobalTransform)>,
    node: Entity,
    direction: TerminalDirection,
    index: u32,
) -> Option<Vec2> {
    for (view, gt) in terminals.iter() {
        if view.node == node && view.direction == direction && view.index == index {
            // `UiGlobalTransform::translation` is the center of the node in
            // screen-space pixels, which is exactly what we want for the
            // wire endpoint (the dot's center).
            let (_scale, _angle, translation) = gt.to_scale_angle_translation();
            return Some(translation);
        }
    }
    None
}
