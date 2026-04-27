//! Pannable/zoomable canvas container for a node graph.
//!
//! The canvas is a two-level hierarchy:
//! * Outer viewport; clipped, receives pointer/scroll events, holds a
//!   [`GraphCanvasViewport`] marker that points back at the graph entity.
//! * Inner "world"; an absolutely-positioned child with a [`UiTransform`]
//!   whose `translation` and `scale` come from the graph's
//!   [`GraphCanvasView`]. Node and connection UI are children of the world,
//!   laid out in canvas-space pixels via `Node::left`/`Node::top`.
//!
//! Pan uses middle-mouse drag; zoom uses the scroll wheel, both gated on
//! cursor hover over the viewport.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::picking::hover::HoverMap;
use bevy::prelude::*;

use crate::graph::{GraphCanvasView, MAX_ZOOM, MIN_ZOOM};

/// Outer viewport node for a graph canvas. Stores the graph entity it
/// displays so systems can look up the view state.
#[derive(Component, Debug, Clone, Copy)]
pub struct GraphCanvasViewport {
    pub graph: Entity,
}

/// Inner world container. Its `UiTransform` is driven by the owning graph's
/// [`GraphCanvasView`]. Node and connection UI entities should be children of
/// an entity with this marker.
#[derive(Component, Debug, Clone, Copy)]
pub struct GraphCanvasWorld {
    pub graph: Entity,
}

/// Spawn a canvas viewport bundle for a graph.
///
/// The viewport is clipped and receives pan/zoom input. Consumer code should
/// also spawn a [`canvas_world`] child under it; nodes and connections live
/// inside the world, which is pan/zoom-transformed.
pub fn canvas(graph: Entity) -> impl Bundle {
    (
        Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            overflow: Overflow::clip(),
            position_type: PositionType::Relative,
            ..default()
        },
        GraphCanvasViewport { graph },
        BackgroundColor(Color::srgb(0.09, 0.09, 0.10)),
        Pickable::default(),
    )
}

/// Spawn the world container bundle for a canvas.
///
/// Designed to be inserted as a child of a [`canvas`] entity. The world
/// receives [`UiTransform`] from [`apply_canvas_view`] each frame, and its
/// children (nodes, connection wires) are laid out in canvas-space pixels
/// via absolute positioning.
pub fn canvas_world(graph: Entity) -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            ..default()
        },
        UiTransform::IDENTITY,
        GraphCanvasWorld { graph },
    )
}

/// Copies the graph's [`GraphCanvasView`] into the world child's
/// [`UiTransform`] each frame.
///
/// We keep `GraphCanvasView` on the graph entity (not the UI node) so it
/// serializes with the scene and survives rebuilds of the UI tree.
pub fn apply_canvas_view(
    graphs: Query<&GraphCanvasView>,
    mut worlds: Query<(&GraphCanvasWorld, &mut UiTransform)>,
) {
    for (world, mut transform) in worlds.iter_mut() {
        let Ok(view) = graphs.get(world.graph) else {
            continue;
        };
        transform.translation = Val2::px(view.offset.x, view.offset.y);
        transform.scale = Vec2::splat(view.zoom);
    }
}

/// Handles pan + zoom input for whichever canvas the pointer is currently
/// hovering over.
///
/// * Middle-mouse drag pans by updating `GraphCanvasView::offset`.
/// * Scroll wheel zooms toward the cursor by updating
///   `GraphCanvasView::zoom` and adjusting `offset` so the point under the
///   cursor stays fixed in canvas space.
pub fn handle_canvas_pan_zoom(
    hover_map: Res<HoverMap>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mut motion: MessageReader<bevy::input::mouse::MouseMotion>,
    mut wheel: MessageReader<MouseWheel>,
    viewports: Query<&GraphCanvasViewport>,
    mut graphs: Query<&mut GraphCanvasView>,
) {
    // Find the hovered viewport (if any) and the cursor's position over it.
    let mut hovered_graph: Option<Entity> = None;
    for pointer_map in hover_map.values() {
        for &entity in pointer_map.keys() {
            if let Ok(viewport) = viewports.get(entity) {
                hovered_graph = Some(viewport.graph);
                break;
            }
        }
        if hovered_graph.is_some() {
            break;
        }
    }

    let Some(graph_entity) = hovered_graph else {
        // Drain to keep readers from backing up.
        motion.clear();
        wheel.clear();
        return;
    };

    let Ok(mut view) = graphs.get_mut(graph_entity) else {
        motion.clear();
        wheel.clear();
        return;
    };

    // Middle-mouse pan.
    if mouse_buttons.pressed(MouseButton::Middle) {
        let mut delta = Vec2::ZERO;
        for ev in motion.read() {
            delta += ev.delta;
        }
        if delta != Vec2::ZERO {
            view.offset += delta;
        }
    } else {
        motion.clear();
    }

    // Scroll wheel zoom.
    let mut zoom_delta = 0.0f32;
    for ev in wheel.read() {
        let step = match ev.unit {
            MouseScrollUnit::Line => ev.y * 0.1,
            MouseScrollUnit::Pixel => ev.y * 0.005,
        };
        zoom_delta += step;
    }
    if zoom_delta != 0.0 {
        let old_zoom = view.zoom;
        let new_zoom = (old_zoom * (1.0 + zoom_delta)).clamp(MIN_ZOOM, MAX_ZOOM);
        if (new_zoom - old_zoom).abs() > f32::EPSILON {
            view.zoom = new_zoom;
            // TODO(phase 2): zoom-toward-cursor. We need the cursor's local
            // coordinate in the viewport, which requires the
            // ComputedNode::normalize_point pattern used by color_picker
            // controls. For phase 1 we zoom around the origin; Phase 2's
            // gesture rework adds the cursor anchor.
        }
    }
}
