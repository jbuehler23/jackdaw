//! Shared window header shell: caption controls, drag region, and an empty content slot.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};

use crate::native_hit_test::NativeHitTestClient;
use crate::{WindowChromeEntity, WindowChromeStyle, WindowChromeTheme};

#[derive(Component)]
pub struct WindowHeaderRoot;

/// Empty flex region between leading chrome and caption controls; fill with screen content.
#[derive(Component)]
pub struct WindowShellHeaderSlot;

/// Backplate behind header chrome; receives window-drag presses on empty title-bar space.
#[derive(Component)]
pub struct WindowHeaderDragRegion;

/// Absolute header background layer; `Node::left` updated when the window fills the work area (macOS).
#[derive(Component)]
pub struct MacosHeaderChromeInset;

/// Leading header widget slot; `Node::margin.left` updated when the window fills the work area (macOS).
#[derive(Component)]
pub struct MacosHeaderLeadingInset;

fn macos_traffic_light_inset(style: WindowChromeStyle, theme: &WindowChromeTheme) -> f32 {
    return if style == WindowChromeStyle::MacNativeTitlebar {
        theme.macos_traffic_light_inset
    } else {
        0.0
    };
}

/// Window header chrome with an empty [`WindowShellHeaderSlot`]. Returns the slot entity.
///
/// `caption_controls` is the minimize/maximize/close cluster bundle. On Windows, use
/// [`crate::window_controls_native`]; on other platforms supply your own button widgets carrying
/// the [`crate::WindowControlsMinimize`] / [`crate::WindowControlsMaximize`] /
/// [`crate::WindowControlsClose`] markers.
pub fn spawn_window_header(
    parent: &mut ChildSpawnerCommands,
    theme: &WindowChromeTheme,
    style: WindowChromeStyle,
    caption_controls: impl Bundle,
) -> Entity {
    let inset = macos_traffic_light_inset(style, theme);
    let show_custom_controls = style.shows_custom_window_controls();
    let uses_macos_native_titlebar = style == WindowChromeStyle::MacNativeTitlebar;

    let mut header_slot = None::<Entity>;
    let header_root = (
        WindowHeaderRoot,
        WindowChromeEntity,
        Node {
            position_type: PositionType::Relative,
            width: percent(100),
            height: px(theme.header_height),
            ..default()
        },
    );
    let mut header_spawner = parent.spawn(header_root);
    if !uses_macos_native_titlebar {
        header_spawner.insert(BackgroundColor(theme.window_background));
    }
    header_spawner.with_children(|header| {
        header_slot = Some(spawn_foreground_row(
            header,
            theme,
            inset,
            uses_macos_native_titlebar,
            show_custom_controls,
            caption_controls,
        ));
    });
    return header_slot.expect("window header content slot spawned");
}

fn spawn_foreground_row(
    parent: &mut ChildSpawnerCommands,
    theme: &WindowChromeTheme,
    macos_traffic_light_inset: f32,
    uses_macos_native_titlebar: bool,
    show_custom_controls: bool,
    caption_controls: impl Bundle,
) -> Entity {
    let mut header_slot = None::<Entity>;
    parent
        .spawn((
            WindowChromeEntity,
            Pickable::IGNORE,
            Node {
                position_type: PositionType::Absolute,
                top: px(0.0),
                left: px(0.0),
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Stretch,
                ..default()
            },
        ))
        .with_children(|row| {
            if uses_macos_native_titlebar {
                row.spawn(header_chrome_background(theme, macos_traffic_light_inset));
            }
            row.spawn(header_drag_backplate(macos_traffic_light_inset));
            header_slot = Some(row.spawn(header_content_slot()).id());
            row.spawn(caption_controls_slot(
                show_custom_controls,
                caption_controls,
            ));
        });
    return header_slot.expect("window header content slot spawned");
}

fn header_chrome_background(
    theme: &WindowChromeTheme,
    macos_traffic_light_inset: f32,
) -> impl Bundle {
    return (
        MacosHeaderChromeInset,
        WindowChromeEntity,
        Pickable::IGNORE,
        Node {
            position_type: PositionType::Absolute,
            top: px(0.0),
            left: px(macos_traffic_light_inset),
            right: px(0.0),
            height: percent(100),
            ..default()
        },
        BackgroundColor(theme.window_background),
    );
}

fn header_drag_backplate(macos_traffic_light_inset: f32) -> impl Bundle {
    return (
        MacosHeaderChromeInset,
        WindowHeaderDragRegion,
        WindowChromeEntity,
        Node {
            position_type: PositionType::Absolute,
            top: px(0.0),
            left: px(macos_traffic_light_inset),
            right: px(0.0),
            height: percent(100),
            ..default()
        },
    );
}

fn caption_controls_slot(show_custom_controls: bool, caption_controls: impl Bundle) -> impl Bundle {
    return (
        WindowChromeEntity,
        Pickable::IGNORE,
        Node {
            flex_shrink: 0.0,
            height: percent(100),
            #[cfg(target_os = "windows")]
            align_items: AlignItems::Stretch,
            #[cfg(not(target_os = "windows"))]
            align_items: AlignItems::Center,
            display: if show_custom_controls {
                Display::Flex
            } else {
                Display::None
            },
            ..default()
        },
        children![caption_controls],
    );
}

fn header_content_slot() -> impl Bundle {
    return (
        WindowShellHeaderSlot,
        WindowChromeEntity,
        Pickable::IGNORE,
        Node {
            flex_grow: 1.0,
            min_width: px(0.0),
            height: percent(100),
            overflow: Overflow::clip(),
            ..default()
        },
    );
}

pub(crate) fn on_drag_region_press(
    press: On<Pointer<Press>>,
    drag_regions: Query<Entity, With<WindowHeaderDragRegion>>,
    mut windows: Query<&mut Window, With<PrimaryWindow>>,
) {
    if press.button != PointerButton::Primary {
        return;
    }
    if drag_regions.get(press.event_target()).is_err() {
        return;
    }
    let Ok(mut window) = windows.single_mut() else {
        return;
    };
    window.start_drag_move();
}

/// Marks a header widget bundle as client-area for native non-client hit testing (Windows).
///
/// Any interactive widget placed in the title-bar drag region (menus, tabs, buttons) must be
/// tagged so Win32 hit testing treats it as client area instead of a window-drag surface.
pub fn native_hit_test_client(content: impl Bundle) -> impl Bundle {
    return (NativeHitTestClient, content);
}
