//! Invisible edge strips for borderless window resize.

use bevy::feathers::cursor::EntityCursor;
use bevy::math::CompassOctant;
use bevy::picking::Pickable;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, SystemCursorIcon, Window, WindowMode};

use crate::WindowChromeEntity;

const RESIZE_HANDLE_THICKNESS: f32 = 8.0;

#[derive(Component)]
pub(crate) struct WindowResizeRoot;

#[derive(Component, Copy, Clone)]
pub(crate) struct WindowResizeEdge(pub CompassOctant);

/// Invisible edge strips for borderless window resize (client-side chrome only).
///
/// Stacked above the title bar drag region and application content so edge picks always win.
pub fn resize_edge_overlay() -> impl Bundle {
    let thickness = px(RESIZE_HANDLE_THICKNESS);
    (
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
                    top: px(0.0),
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
    )
}

fn resize_edge(direction: CompassOctant, node: Node) -> impl Bundle {
    (
        WindowResizeEdge(direction),
        WindowChromeEntity,
        Pickable::default(),
        node,
        Hovered::default(),
        EntityCursor::System(resize_cursor_icon(direction)),
    )
}

/// Disables resize-edge picking while the window cannot be resized.
pub(crate) fn sync_resize_overlay_pickability(
    _main_thread: bevy::ecs::system::NonSendMarker,
    primary_window: Query<(Entity, &Window), With<PrimaryWindow>>,
    mut resize_edges: Query<&mut Pickable, With<WindowResizeEdge>>,
) {
    let Ok((window_entity, window)) = primary_window.single() else {
        return;
    };
    let resizing_disabled = !matches!(window.mode, WindowMode::Windowed)
        || crate::primary_window_is_maximized(window_entity);
    let pickable = if resizing_disabled {
        Pickable::IGNORE
    } else {
        Pickable::default()
    };
    for mut edge_pickable in resize_edges.iter_mut() {
        if *edge_pickable != pickable {
            *edge_pickable = pickable.clone();
        }
    }
}

fn resize_cursor_icon(direction: CompassOctant) -> SystemCursorIcon {
    match direction {
        CompassOctant::North => SystemCursorIcon::NResize,
        CompassOctant::South => SystemCursorIcon::SResize,
        CompassOctant::East => SystemCursorIcon::EResize,
        CompassOctant::West => SystemCursorIcon::WResize,
        CompassOctant::NorthEast => SystemCursorIcon::NeResize,
        CompassOctant::NorthWest => SystemCursorIcon::NwResize,
        CompassOctant::SouthEast => SystemCursorIcon::SeResize,
        CompassOctant::SouthWest => SystemCursorIcon::SwResize,
    }
}

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
