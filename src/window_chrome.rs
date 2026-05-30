//! Borderless primary window caption buttons and resize edge handles.

use bevy::feathers::cursor::EntityCursor;
use bevy::math::CompassOctant;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, SystemCursorIcon, Window, WindowCloseRequested, WindowMode};
use bevy::winit::WINIT_WINDOWS;
use jackdaw_feathers::{
    button::{ButtonClickEvent, ButtonSize, ButtonVariant, IconButtonProps, icon_button},
    icons::Icon,
};

use crate::EditorEntity;

const RESIZE_HANDLE_THICKNESS: f32 = 5.0;
const WINDOW_SHELL_CORNER_RADIUS_PX: f32 = 8.0;

/// Root shell node whose corners track windowed vs maximized state.
#[derive(Component)]
pub struct WindowShellRoot;

#[derive(Component)]
struct WindowChromeMinimize;

#[derive(Component)]
struct WindowChromeMaximize;

#[derive(Component)]
struct WindowChromeClose;

#[derive(Component)]
struct WindowChromeResizeRoot;

#[derive(Component, Copy, Clone)]
struct WindowChromeResizeEdge(pub CompassOctant);

pub struct WindowChromePlugin;

impl Plugin for WindowChromePlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_minimize_click)
            .add_observer(on_maximize_click)
            .add_observer(on_close_click);
        #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
        {
            app.add_observer(on_resize_edge_press)
                .add_systems(Update, sync_window_chrome_state);
        }
    }
}

/// Primary-window settings for hosts that own [`WindowPlugin`].
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn borderless_primary_window() -> Window {
    Window {
        decorations: false,
        ..default()
    }
}

#[cfg(any(target_arch = "wasm32", target_os = "ios", target_os = "android"))]
pub fn borderless_primary_window() -> Window {
    Window::default()
}

fn caption_button(
    icon: Icon,
    marker: impl Bundle,
    icon_font: Handle<Font>,
    variant: ButtonVariant,
) -> impl Bundle {
    (
        marker,
        EditorEntity,
        icon_button(
            IconButtonProps::new(icon)
                .variant(variant)
                .with_size(ButtonSize::Icon),
            &icon_font,
        ),
    )
}

/// Minimize / maximize / close cluster for the top chrome row.
pub fn window_controls(icon_font: Handle<Font>) -> impl Bundle {
    (
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(2.0),
            flex_shrink: 0.0,
            ..default()
        },
        Pickable::IGNORE,
        children![
            caption_button(
                Icon::Minus,
                WindowChromeMinimize,
                icon_font.clone(),
                ButtonVariant::Ghost,
            ),
            caption_button(
                Icon::Maximize2,
                WindowChromeMaximize,
                icon_font.clone(),
                ButtonVariant::Ghost,
            ),
            caption_button(Icon::X, WindowChromeClose, icon_font, ButtonVariant::Close),
        ],
    )
}

/// Invisible edge strips for borderless window resize (`start_drag_resize`).
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn resize_edge_overlay() -> impl Bundle {
    let thickness = px(RESIZE_HANDLE_THICKNESS);
    let corner = thickness;

    (
        WindowChromeResizeRoot,
        EditorEntity,
        // Full-window container for positioning edge strips only. Without
        // `Pickable::IGNORE` this node blocks every UI element beneath it.
        Pickable::IGNORE,
        GlobalZIndex(10_000),
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
                    width: corner,
                    height: corner,
                    ..default()
                },
            ),
            resize_edge(
                CompassOctant::NorthEast,
                Node {
                    position_type: PositionType::Absolute,
                    top: px(0.0),
                    right: px(0.0),
                    width: corner,
                    height: corner,
                    ..default()
                },
            ),
            resize_edge(
                CompassOctant::SouthWest,
                Node {
                    position_type: PositionType::Absolute,
                    bottom: px(0.0),
                    left: px(0.0),
                    width: corner,
                    height: corner,
                    ..default()
                },
            ),
            resize_edge(
                CompassOctant::SouthEast,
                Node {
                    position_type: PositionType::Absolute,
                    bottom: px(0.0),
                    right: px(0.0),
                    width: corner,
                    height: corner,
                    ..default()
                },
            ),
        ],
    )
}

#[cfg(any(target_arch = "wasm32", target_os = "ios", target_os = "android"))]
pub fn resize_edge_overlay() -> impl Bundle {
    (
        EditorEntity,
        Node {
            display: Display::None,
            ..default()
        },
    )
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
fn resize_edge(direction: CompassOctant, node: Node) -> impl Bundle {
    (
        WindowChromeResizeEdge(direction),
        EditorEntity,
        node,
        Hovered::default(),
        EntityCursor::System(resize_cursor_icon(direction)),
        // Transparent nodes still need a pick target for edge resize drags.
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.001)),
    )
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
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

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
fn on_resize_edge_press(
    press: On<Pointer<Press>>,
    edges: Query<&WindowChromeResizeEdge>,
    parents: Query<&ChildOf>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    let Some(edge) = find_ancestor_component(press.event_target(), &edges, &parents) else {
        return;
    };
    for mut window in windows.iter_mut() {
        window.start_drag_resize(edge.0);
    }
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
fn sync_window_chrome_state(
    _main_thread: bevy::ecs::system::NonSendMarker,
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    mut overlays: Query<&mut Node, (With<WindowChromeResizeRoot>, Without<WindowShellRoot>)>,
    mut shells: Query<&mut Node, (With<WindowShellRoot>, Without<WindowChromeResizeRoot>)>,
    maximize_buttons: Query<&Children, With<WindowChromeMaximize>>,
    mut texts: Query<&mut Text>,
) {
    let Ok((entity, window)) = windows.single() else {
        return;
    };

    let is_fullscreen = !matches!(window.mode, WindowMode::Windowed);
    let is_maximized = primary_window_is_maximized(entity);

    let windowed_shell = !is_fullscreen && !is_maximized;
    for mut node in overlays.iter_mut() {
        node.display = if windowed_shell {
            Display::Flex
        } else {
            Display::None
        };
    }

    let shell_border_radius = if windowed_shell {
        BorderRadius::all(Val::Px(WINDOW_SHELL_CORNER_RADIUS_PX))
    } else {
        BorderRadius::ZERO
    };
    let shell_overflow = if windowed_shell {
        Overflow::clip()
    } else {
        Overflow::default()
    };
    for mut node in shells.iter_mut() {
        node.border_radius = shell_border_radius;
        node.overflow = shell_overflow;
    }

    #[cfg(target_os = "windows")]
    apply_windows_corner_preference(entity, windowed_shell);

    let icon = if is_maximized {
        Icon::Minimize2
    } else {
        Icon::Maximize2
    };
    let glyph = icon.unicode().to_string();
    for children in &maximize_buttons {
        for child in children.iter() {
            let Ok(mut text) = texts.get_mut(child) else {
                continue;
            };
            if text.0 != glyph {
                text.0 = glyph.clone();
            }
        }
    }
}

#[cfg(all(
    not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")),
    target_os = "windows"
))]
fn apply_windows_corner_preference(window_entity: Entity, round: bool) {
    use winit::platform::windows::{CornerPreference, WindowExtWindows};

    WINIT_WINDOWS.with(|windows_cell| {
        let winit_windows = windows_cell.borrow();
        let Some(backend) = winit_windows.get_window(window_entity) else {
            return;
        };
        let preference = if round {
            CornerPreference::Round
        } else {
            CornerPreference::DoNotRound
        };
        backend.set_corner_preference(preference);
    });
}

fn on_minimize_click(
    click: On<ButtonClickEvent>,
    buttons: Query<Entity, With<WindowChromeMinimize>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if buttons.get(click.entity).is_err() {
        return;
    }
    for mut window in windows.iter_mut() {
        window.set_minimized(true);
    }
}

fn on_maximize_click(
    click: On<ButtonClickEvent>,
    buttons: Query<Entity, With<WindowChromeMaximize>>,
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
) {
    if buttons.get(click.entity).is_err() {
        return;
    }
    let Ok((window_entity, mut window)) = windows.single_mut() else {
        return;
    };
    #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
    let next_maximized = !primary_window_is_maximized(window_entity);
    #[cfg(any(target_arch = "wasm32", target_os = "ios", target_os = "android"))]
    let next_maximized = true;
    window.set_maximized(next_maximized);
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
fn primary_window_is_maximized(window_entity: Entity) -> bool {
    WINIT_WINDOWS.with(|windows_cell| {
        let winit_windows = windows_cell.borrow();
        winit_windows
            .get_window(window_entity)
            .is_some_and(|backend| backend.is_maximized())
    })
}

fn on_close_click(
    click: On<ButtonClickEvent>,
    buttons: Query<Entity, With<WindowChromeClose>>,
    windows: Query<Entity, With<PrimaryWindow>>,
    mut close_events: MessageWriter<WindowCloseRequested>,
) {
    if buttons.get(click.entity).is_err() {
        return;
    }
    let Ok(window) = windows.single() else {
        return;
    };
    close_events.write(WindowCloseRequested { window });
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
fn find_ancestor_component<C: Component + Copy>(
    mut entity: Entity,
    query: &Query<&C>,
    parents: &Query<&ChildOf>,
) -> Option<C> {
    loop {
        if let Ok(component) = query.get(entity) {
            return Some(*component);
        }
        let Ok(parent) = parents.get(entity) else {
            return None;
        };
        entity = parent.parent();
    }
}
