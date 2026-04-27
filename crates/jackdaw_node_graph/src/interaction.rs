//! Pointer-driven interaction: selection, node drag, connection drag.
//!
//! Observers are registered in [`crate::NodeGraphPlugin::build`]. Each
//! observer either mutates in-memory gesture state ([`GraphGesture`]) or
//! queues a deferred world mutation that pushes an
//! [`EditorCommand`](jackdaw_commands::EditorCommand) onto the shared
//! [`CommandHistory`] so the change is undoable.
//!
//! The gesture lifecycle for a node drag is:
//!
//! 1. `Pointer<DragStart>` → snapshot positions, transition to
//!    [`GraphGesture::MoveNodes`].
//! 2. `Pointer<Drag>` → write live positions using `Drag::distance`.
//! 3. `Pointer<DragEnd>` → compute `(old, new)` diffs, queue a
//!    [`MoveGraphNodesCmd`] on `CommandHistory`, reset gesture to `Idle`.

use bevy::ecs::relationship::Relationship;
use bevy::picking::events::{Click, Drag, DragEnd, DragStart, Pointer};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use jackdaw_commands::CommandHistory;
use std::collections::HashMap;

use crate::commands::{
    CreateConnectionCmd, MoveGraphNodesCmd, RemoveConnectionCmd, RemoveGraphNodesCmd,
};
use crate::gesture::{ConnectionAnchor, GraphGesture};
use crate::graph::{Connection, GraphCanvasView, GraphNode, TerminalDirection};
use crate::node_widget::{GraphNodeView, GraphTerminalView};
use crate::selection::GraphSelection;

// ==========================================================================
// Node click + drag
// ==========================================================================

/// Select a node on plain click; shift extends, ctrl toggles.
///
/// **Order-of-events note**: `bevy_picking` fires `Click` *before* `DragEnd`
/// in the same release handler. That means after a drag-to-move, this
/// observer would naively fire with the released-on node as its target and
/// clobber the group selection with a single-node select. We guard against
/// that by bailing whenever `GraphGesture` is not `Idle`; during a drag
/// the gesture is `MoveNodes`/`ConnectDrag`/etc., and the guard sees it
/// before `on_node_drag_end` resets it to `Idle`. A plain click (no drag)
/// keeps the gesture at `Idle` throughout, so it still selects normally.
///
/// `event.event_target()` is propagation-aware: the event bubbles up from
/// the click target through `ChildOf` until it hits a `GraphNodeView`
/// ancestor, which is the node root. Widgets inside the body that want
/// to absorb their own clicks (like feathers `text_edit`) stop
/// propagation on their own wrapper, so those clicks never reach here.
pub fn on_node_click(
    event: On<Pointer<Click>>,
    views: Query<&GraphNodeView>,
    gesture: Res<GraphGesture>,
    keys: Res<ButtonInput<KeyCode>>,
    mut selection: ResMut<GraphSelection>,
    mut commands: Commands,
) {
    if !matches!(*gesture, GraphGesture::Idle) {
        return;
    }
    let Ok(view) = views.get(event.event_target()) else {
        return;
    };
    if keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight) {
        selection.extend(&mut commands, view.0);
    } else if keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight) {
        selection.toggle(&mut commands, view.0);
    } else {
        selection.select_single(&mut commands, view.0);
    }
}

/// Begin a `MoveNodes` gesture and ensure the dragged node is selected.
///
/// Relies on `Pointer<DragStart>` propagation walking up from the original
/// target until it hits a `GraphNodeView` ancestor; i.e. the node root.
/// Drags from widgets that want to own their own drag gesture (like
/// feathers `text_edit`'s scrub hitbox) stop propagation on their own
/// wrapper so they never reach this observer.
pub fn on_node_drag_start(
    event: On<Pointer<DragStart>>,
    views: Query<&GraphNodeView>,
    nodes: Query<&GraphNode>,
    mut selection: ResMut<GraphSelection>,
    mut gesture: ResMut<GraphGesture>,
    mut commands: Commands,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let Ok(view) = views.get(event.event_target()) else {
        return;
    };

    if !selection.entities.contains(&view.0) {
        selection.select_single(&mut commands, view.0);
    }

    let mut snapshot = HashMap::new();
    for &entity in &selection.entities {
        if let Ok(node) = nodes.get(entity) {
            snapshot.insert(entity, node.position);
        }
    }
    *gesture = GraphGesture::MoveNodes {
        start_positions: snapshot,
        anchor_cursor: Vec2::ZERO,
    };
}

/// Apply live drag deltas to every node in the move snapshot.
///
/// Uses `Drag::distance` (total movement from drag start) scaled by the
/// inverse of the canvas zoom so the node tracks the cursor at any zoom.
///
/// Gated purely on `GraphGesture::MoveNodes`; no query on the event
/// target; because `on_node_drag_start` is the only place that
/// transitions into `MoveNodes`, and that observer already enforces the
/// `NodeDragHandle` rule.
pub fn on_node_drag(
    event: On<Pointer<Drag>>,
    mut nodes: Query<&mut GraphNode>,
    gesture: Res<GraphGesture>,
    canvas_views: Query<&GraphCanvasView>,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let GraphGesture::MoveNodes {
        start_positions, ..
    } = &*gesture
    else {
        return;
    };

    // Phase 2 supports one canvas at a time; pick whichever we find.
    let zoom = canvas_views.iter().next().map(|v| v.zoom).unwrap_or(1.0);
    let world_delta = event.distance / zoom.max(f32::EPSILON);

    for (&entity, &start_pos) in start_positions {
        if let Ok(mut node) = nodes.get_mut(entity) {
            node.position = start_pos + world_delta;
        }
    }
}

/// Commit the move as an undoable command.
///
/// Same gating as `on_node_drag`: only acts when the gesture is
/// `MoveNodes`, which is only set by `on_node_drag_start` after passing
/// the `NodeDragHandle` walk-up.
pub fn on_node_drag_end(
    event: On<Pointer<DragEnd>>,
    nodes: Query<&GraphNode>,
    mut gesture: ResMut<GraphGesture>,
    mut commands: Commands,
) {
    if event.button != PointerButton::Primary {
        return;
    }

    let previous = std::mem::replace(&mut *gesture, GraphGesture::Idle);
    let GraphGesture::MoveNodes {
        start_positions, ..
    } = previous
    else {
        return;
    };

    // Compute (old, new) pairs now; the query gives us the current (new)
    // positions. Skip entries that didn't actually move (click-then-release).
    let mut moves: Vec<(Entity, Vec2, Vec2)> = Vec::with_capacity(start_positions.len());
    for (entity, start_pos) in start_positions {
        let Ok(node) = nodes.get(entity) else {
            continue;
        };
        if (node.position - start_pos).length_squared() < 0.01 {
            continue;
        }
        moves.push((entity, start_pos, node.position));
    }
    if moves.is_empty() {
        return;
    }

    // Defer into a world-scoped closure so we can call
    // CommandHistory::execute with `&mut World`. Execute is idempotent for
    // this command (positions are already written), but tracks the new state
    // for redo.
    commands.queue(move |world: &mut World| {
        let cmd = Box::new(MoveGraphNodesCmd { moves });
        let mut history = world
            .remove_resource::<CommandHistory>()
            .unwrap_or_default();
        history.execute(cmd, world);
        world.insert_resource(history);
    });
}

// ==========================================================================
// Terminal drag → connection creation
// ==========================================================================

/// Start a `ConnectDrag` from an output terminal.
///
/// Pointer events auto-propagate up the `ChildOf` chain (see
/// `bevy_picking::events::Pointer`; `#[entity_event(auto_propagate)]`), so if
/// we don't stop propagation here, the same `DragStart` bubbles up to the
/// node root and `on_node_drag_start` kicks in, overwriting our `ConnectDrag`
/// with a `MoveNodes` gesture and the node ends up sliding around instead of
/// spawning a wire.
///
/// **Coordinate note**: `event.pointer_location.position` is in **logical**
/// pixels while `UiGlobalTransform::translation` and `ComputedNode::size`
/// are in **physical** pixels. We convert here so everything downstream
/// (snap hit-testing, ghost rendering, `on_terminal_drag_end`) works in
/// one unit system regardless of DPI scale.
pub fn on_terminal_drag_start(
    mut event: On<Pointer<DragStart>>,
    terminals: Query<(&GraphTerminalView, &ComputedNode)>,
    mut gesture: ResMut<GraphGesture>,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let Ok((view, computed)) = terminals.get(event.event_target()) else {
        return;
    };

    // Phase 2 only supports dragging out of an output (the common case).
    // Dragging the endpoint of an existing connection is Phase 6 polish.
    if view.direction != TerminalDirection::Output {
        return;
    }

    let cursor_physical = event.pointer_location.position / computed.inverse_scale_factor;

    *gesture = GraphGesture::ConnectDrag {
        source: ConnectionAnchor::FromOutput {
            node: view.node,
            terminal: view.index,
        },
        cursor_pos: cursor_physical,
        snap_target: None,
    };
    event.propagate(false);
}

/// Track cursor while dragging a connection ghost.
pub fn on_terminal_drag(
    mut event: On<Pointer<Drag>>,
    terminals: Query<(&GraphTerminalView, &ComputedNode)>,
    mut gesture: ResMut<GraphGesture>,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    // Only match drag events whose original target is a terminal; this
    // early-return also prevents us from eating the bubble of a node drag.
    let Ok((_view, computed)) = terminals.get(event.event_target()) else {
        return;
    };
    let cursor_physical = event.pointer_location.position / computed.inverse_scale_factor;
    if let GraphGesture::ConnectDrag { cursor_pos, .. } = &mut *gesture {
        *cursor_pos = cursor_physical;
        event.propagate(false);
    }
}

/// Commit or cancel the connection at the end of a terminal drag.
///
/// Snap-target hit-testing lives in `update_ghost_wire` (runs every frame
/// while the gesture is `ConnectDrag`, in `PostUpdate` after layout so
/// terminal transforms are fresh). This observer just reads the
/// last-computed `snap_target` off the gesture and commits it.
pub fn on_terminal_drag_end(
    mut event: On<Pointer<DragEnd>>,
    terminal_views: Query<&GraphTerminalView>,
    nodes: Query<Option<&ChildOf>, With<GraphNode>>,
    mut gesture: ResMut<GraphGesture>,
    mut commands: Commands,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    // Only act on drag-ends that started on a terminal. Node drags bubble
    // through the same observer and would otherwise clobber the gesture
    // state machine on release.
    if terminal_views.get(event.event_target()).is_err() {
        return;
    }
    let previous = std::mem::replace(&mut *gesture, GraphGesture::Idle);
    let GraphGesture::ConnectDrag {
        source,
        snap_target,
        ..
    } = previous
    else {
        return;
    };
    event.propagate(false);

    let ConnectionAnchor::FromOutput {
        node: source_node,
        terminal: source_terminal_idx,
    } = source
    else {
        return;
    };
    let Some(target) = snap_target else {
        // No compatible input was under the cursor at release; drop
        // silently. The ghost wire has already been despawned by
        // update_ghost_wire on the next frame.
        return;
    };

    let Ok(Some(parent)) = nodes.get(source_node) else {
        return;
    };
    let graph = parent.get();
    let target_node = target.node;
    let target_terminal_idx = target.terminal;

    commands.queue(move |world: &mut World| {
        let cmd = Box::new(CreateConnectionCmd::new(
            graph,
            source_node,
            source_terminal_idx,
            target_node,
            target_terminal_idx,
        ));
        let mut history = world
            .remove_resource::<CommandHistory>()
            .unwrap_or_default();
        history.execute(cmd, world);
        world.insert_resource(history);
    });
}

/// Swallow plain clicks on terminals so they don't bubble up to
/// `on_node_click` (which would select the owning node when the user meant
/// to interact with the terminal).
pub fn on_terminal_click(mut event: On<Pointer<Click>>, terminals: Query<&GraphTerminalView>) {
    if event.button != PointerButton::Primary {
        return;
    }
    if terminals.get(event.event_target()).is_err() {
        return;
    }
    event.propagate(false);
}

/// Right click on a terminal removes every connection touching it.
///
/// Simpler than hit-testing the wire's Bezier curve on the CPU; good enough
/// for Phase 3. Each removal is pushed as an `EditorCommand` so undo/redo
/// works automatically.
pub fn on_terminal_right_click(
    mut event: On<Pointer<Click>>,
    terminals: Query<&GraphTerminalView>,
    connections: Query<(Entity, &Connection, Option<&ChildOf>)>,
    mut commands: Commands,
) {
    if event.button != PointerButton::Secondary {
        return;
    }
    let Ok(term) = terminals.get(event.event_target()) else {
        return;
    };

    let node = term.node;
    let idx = term.index;
    let direction = term.direction;

    let targets: Vec<(Entity, Entity)> = connections
        .iter()
        .filter_map(|(entity, conn, parent)| {
            let graph = parent.map(Relationship::get)?;
            let matches = match direction {
                TerminalDirection::Input => conn.target_node == node && conn.target_terminal == idx,
                TerminalDirection::Output => {
                    conn.source_node == node && conn.source_terminal == idx
                }
            };
            matches.then_some((entity, graph))
        })
        .collect();

    if targets.is_empty() {
        return;
    }
    event.propagate(false);

    commands.queue(move |world: &mut World| {
        let mut history = world
            .remove_resource::<jackdaw_commands::CommandHistory>()
            .unwrap_or_default();
        for (conn_entity, graph) in targets {
            let cmd = Box::new(RemoveConnectionCmd::new(graph, conn_entity));
            history.execute(cmd, world);
        }
        world.insert_resource(history);
    });
}

// ==========================================================================
// Keyboard shortcuts
// ==========================================================================

/// Delete or Backspace removes the current graph selection (nodes +
/// incident connections) via `RemoveGraphNodesCmd`.
pub fn handle_delete_key(
    keys: Res<ButtonInput<KeyCode>>,
    selection: Res<GraphSelection>,
    nodes: Query<Option<&ChildOf>, With<GraphNode>>,
    mut commands: Commands,
) {
    if !keys.just_pressed(KeyCode::Delete) && !keys.just_pressed(KeyCode::Backspace) {
        return;
    }
    if selection.entities.is_empty() {
        return;
    }

    // Resolve the owning graph from the first selected node.
    let graph = selection
        .entities
        .iter()
        .find_map(|&e| nodes.get(e).ok().and_then(|p| p.map(ChildOf::get)));
    let Some(graph) = graph else {
        return;
    };
    let entities = selection.entities.clone();

    commands.queue(move |world: &mut World| {
        let cmd = Box::new(RemoveGraphNodesCmd::new(graph, entities));
        let mut history = world
            .remove_resource::<CommandHistory>()
            .unwrap_or_default();
        history.execute(cmd, world);
        world.insert_resource(history);

        // Clear the selection after the removal; the entities no longer exist.
        if let Some(mut sel) = world.get_resource_mut::<GraphSelection>() {
            sel.entities.clear();
        }
    });
}

/// Ctrl+Z / Ctrl+Y (or Ctrl+Shift+Z) drive undo/redo against the shared
/// `CommandHistory`.
pub fn handle_undo_redo_keys(keys: Res<ButtonInput<KeyCode>>, mut commands: Commands) {
    let ctrl = keys.pressed(KeyCode::ControlLeft) || keys.pressed(KeyCode::ControlRight);
    if !ctrl {
        return;
    }
    let shift = keys.pressed(KeyCode::ShiftLeft) || keys.pressed(KeyCode::ShiftRight);

    let want_undo = keys.just_pressed(KeyCode::KeyZ) && !shift;
    let want_redo = (keys.just_pressed(KeyCode::KeyZ) && shift) || keys.just_pressed(KeyCode::KeyY);

    if want_undo {
        commands.queue(|world: &mut World| {
            let mut history = world
                .remove_resource::<CommandHistory>()
                .unwrap_or_default();
            history.undo(world);
            world.insert_resource(history);
        });
    } else if want_redo {
        commands.queue(|world: &mut World| {
            let mut history = world
                .remove_resource::<CommandHistory>()
                .unwrap_or_default();
            history.redo(world);
            world.insert_resource(history);
        });
    }
}
