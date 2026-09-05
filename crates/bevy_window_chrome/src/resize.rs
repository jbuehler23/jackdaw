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

/// Whether the edge strips are live, as the last sync found the window.
///
/// The strips lie over the outer eight pixels of the window, which is the
/// menu bar and the outermost panel borders. A press there must reach
/// what it landed on unless the window can actually be resized by it, and
/// the press arrives a frame after the state the strips were painted for.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ResizeEdgesLive(pub bool);

/// Whether a drag on an edge strip would resize the window. A maximized
/// or fullscreen window has no edge to pull, and neither has one the
/// platform will not resize.
pub(crate) fn edges_resize(mode: WindowMode, maximized: bool, resizable: bool) -> bool {
    matches!(mode, WindowMode::Windowed) && !maximized && resizable
}

/// Disables resize-edge picking while the window cannot be resized.
pub(crate) fn sync_resize_overlay_pickability(
    _main_thread: bevy::ecs::system::NonSendMarker,
    primary_window: Query<(Entity, &Window), With<PrimaryWindow>>,
    mut resize_edges: Query<&mut Pickable, With<WindowResizeEdge>>,
    mut live: ResMut<ResizeEdgesLive>,
) {
    let Ok((window_entity, window)) = primary_window.single() else {
        return;
    };
    let resizes = edges_resize(
        window.mode,
        crate::primary_window_is_maximized(window_entity),
        window.resizable,
    );
    live.set_if_neq(ResizeEdgesLive(resizes));
    let pickable = if resizes {
        Pickable::default()
    } else {
        Pickable::IGNORE
    };
    for mut edge_pickable in resize_edges.iter_mut() {
        edge_pickable.set_if_neq(pickable);
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
    live: Res<ResizeEdgesLive>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    // The strips stop taking picks a frame after the window is
    // maximized, and a press in that frame would start a drag-resize a
    // maximized window cannot honor.
    if !live.0 {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The strips sit over the outermost eight pixels of the window,
    /// where a maximized window has its menu bar and panel edges rather
    /// than a border to pull.
    #[test]
    fn a_maximized_window_has_no_resizable_edge() {
        assert!(edges_resize(WindowMode::Windowed, false, true));
        assert!(!edges_resize(WindowMode::Windowed, true, true));
    }

    #[test]
    fn a_fullscreen_or_fixed_window_has_no_resizable_edge() {
        assert!(!edges_resize(
            WindowMode::BorderlessFullscreen(MonitorSelection::Primary),
            false,
            true
        ));
        assert!(!edges_resize(WindowMode::Windowed, false, false));
    }
}
