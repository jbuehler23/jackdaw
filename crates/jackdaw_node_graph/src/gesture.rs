//! Gesture state machine for canvas interaction.
//!
//! Phase 1 exposes the enum and a default-`Idle` resource; Phase 2 wires in
//! pointer observers to drive transitions (pan, node move, rect-select,
//! connection drag).

use bevy::prelude::*;
use std::collections::HashMap;

/// Where a drag-to-connect gesture started.
#[derive(Clone, Copy, Debug)]
pub enum ConnectionAnchor {
    /// Dragging out of an output terminal to create a new connection.
    FromOutput { node: Entity, terminal: u32 },
    /// Dragging the endpoint of an existing connection to re-route it.
    FromInput { node: Entity, terminal: u32 },
}

/// The terminal a connect-drag is currently snapped to (closest compatible
/// input terminal within the snap radius). Populated continuously by
/// `update_ghost_wire` so `on_terminal_drag_end` can commit without
/// re-running hit-testing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SnapHit {
    pub node: Entity,
    pub terminal: u32,
}

/// Active gesture in the graph canvas.
///
/// Exactly one gesture is active at a time. Transitions happen in observers
/// attached to the canvas and node UI entities in Phase 2.
#[derive(Resource, Default, Debug)]
pub enum GraphGesture {
    #[default]
    Idle,
    PanCanvas {
        start_offset: Vec2,
        anchor_cursor: Vec2,
    },
    MoveNodes {
        start_positions: HashMap<Entity, Vec2>,
        anchor_cursor: Vec2,
    },
    RectSelect {
        anchor: Vec2,
        current: Vec2,
    },
    /// Drag-to-connect. `cursor_pos` is in **physical** pixels (same unit
    /// as `UiGlobalTransform::translation`). `snap_target` is updated each
    /// frame by `update_ghost_wire`; `on_terminal_drag_end` reads it to
    /// decide whether to commit the connection.
    ConnectDrag {
        source: ConnectionAnchor,
        cursor_pos: Vec2,
        snap_target: Option<SnapHit>,
    },
}
