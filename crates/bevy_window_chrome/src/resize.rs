//! Invisible edge strips for borderless window resize (client-side chrome only).

use bevy::feathers::cursor::EntityCursor;
use bevy::math::CompassOctant;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, SystemCursorIcon, Window};

use crate::WindowChromeEntity;
use crate::chrome::WindowChromeStyle;

const RESIZE_HANDLE_THICKNESS: f32 = 8.0;

#[derive(Component)]
pub(crate) struct WindowResizeRoot;

#[derive(Component, Copy, Clone)]
pub(crate) struct WindowResizeEdge(pub CompassOctant);

/// Spawns the resize edge overlay as a child if the chrome style uses client-side resize handles.
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn spawn_resize_edge_overlay_if_needed(
    parent: &mut ChildSpawnerCommands,
    style: WindowChromeStyle,
    header_height: f32,
) {
    if style.uses_resize_edge_overlay() {
        parent.spawn(resize_edge_overlay(header_height));
    }
}

/// Invisible edge strips for borderless window resize (client-side chrome only).
///
/// The top edge starts below the header band so it does not fight the title-bar drag region.
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn resize_edge_overlay(header_height: f32) -> impl Bundle {
    let thickness = px(RESIZE_HANDLE_THICKNESS);
    let header_band = px(header_height);
    return (
        WindowResizeRoot,
        WindowChromeEntity,
        Pickable::IGNORE,
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            ..default()
        },
        children![
            resize_edge(
                CompassOctant::North,
                Node {
                    position_type: PositionType::Absolute,
                    top: header_band,
                    left: px(0.0),
                    width: percent(100),
                    height: thickness,
                    ..default()
                },
            ),
            resize_edge(
                CompassOctant::South,
                Node {
                    position_type: PositionType::Absolute,
                    bottom: px(0.0),
                    left: px(0.0),
                    width: percent(100),
                    height: thickness,
                    ..default()
                },
            ),
            resize_edge(
                CompassOctant::West,
                Node {
                    position_type: PositionType::Absolute,
                    top: px(0.0),
                    left: px(0.0),
                    width: thickness,
                    height: percent(100),
                    ..default()
                },
            ),
            resize_edge(
                CompassOctant::East,
                Node {
                    position_type: PositionType::Absolute,
                    top: px(0.0),
                    right: px(0.0),
                    width: thickness,
                    height: percent(100),
                    ..default()
                },
            ),
            resize_edge(
                CompassOctant::NorthWest,
                Node {
                    position_type: PositionType::Absolute,
                    top: px(0.0),
                    left: px(0.0),
                    width: thickness,
                    height: thickness,
                    ..default()
                },
            ),
            resize_edge(
                CompassOctant::NorthEast,
                Node {
                    position_type: PositionType::Absolute,
                    top: px(0.0),
                    right: px(0.0),
                    width: thickness,
                    height: thickness,
                    ..default()
                },
            ),
            resize_edge(
                CompassOctant::SouthWest,
                Node {
                    position_type: PositionType::Absolute,
                    bottom: px(0.0),
                    left: px(0.0),
                    width: thickness,
                    height: thickness,
                    ..default()
                },
            ),
            resize_edge(
                CompassOctant::SouthEast,
                Node {
                    position_type: PositionType::Absolute,
                    bottom: px(0.0),
                    right: px(0.0),
                    width: thickness,
                    height: thickness,
                    ..default()
                },
            ),
        ],
    );
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
fn resize_edge(direction: CompassOctant, node: Node) -> impl Bundle {
    return (
        WindowResizeEdge(direction),
        WindowChromeEntity,
        node,
        Hovered::default(),
        EntityCursor::System(resize_cursor_icon(direction)),
    );
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
fn resize_cursor_icon(direction: CompassOctant) -> SystemCursorIcon {
    return match direction {
        CompassOctant::North => SystemCursorIcon::NResize,
        CompassOctant::South => SystemCursorIcon::SResize,
        CompassOctant::East => SystemCursorIcon::EResize,
        CompassOctant::West => SystemCursorIcon::WResize,
        CompassOctant::NorthEast => SystemCursorIcon::NeResize,
        CompassOctant::NorthWest => SystemCursorIcon::NwResize,
        CompassOctant::SouthEast => SystemCursorIcon::SeResize,
        CompassOctant::SouthWest => SystemCursorIcon::SwResize,
    };
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub(crate) fn on_resize_edge_press(
    press: On<Pointer<Press>>,
    edges: Query<&WindowResizeEdge>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    let Ok(edge) = edges.get(press.original_event_target()) else {
        return;
    };
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    window.start_drag_resize(edge.0);
}
