//! Shared window header shell for the editor and project launcher.
//!
//! Has window controls, drag region, and an empty content slot for screen-specific header UI.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};
use jackdaw_feathers::tokens;

use crate::EditorEntity;

use super::chrome::WindowChromeStyle;
use super::controls;
use super::native_hit_test::NativeHitTestClient;

#[derive(Component)]
pub struct WindowHeaderRoot;

/// Empty flex region between leading chrome and caption controls; fill with screen content.
#[derive(Component)]
pub struct WindowShellHeaderSlot;

/// Backplate behind header chrome; receives window-drag presses on empty title-bar space.
#[derive(Component)]
pub struct WindowHeaderDragRegion;

/// Absolute header layers; `Node::left` updated when the window fills the work area (macOS).
#[derive(Component)]
pub struct MacosHeaderChromeInset;

pub struct WindowHeaderPlugin;

impl Plugin for WindowHeaderPlugin {
    fn build(&self, app: &mut App) {
        let chrome = WindowChromeStyle::current();
        if chrome.uses_app_drag_handler() {
            app.add_observer(on_drag_region_press);
        }
    }
}

/// Window header chrome with an empty [`WindowShellHeaderSlot`]. Returns the slot entity.
pub fn spawn_window_header(
    parent: &mut ChildSpawnerCommands,
    #[cfg(not(target_os = "windows"))] icon_font: Handle<Font>,
    #[cfg(target_os = "windows")] caption_font: Handle<Font>,
    chrome: WindowChromeStyle,
) -> Entity {
    let macos_traffic_light_inset = chrome.macos_traffic_light_inset();
    let show_custom_controls = chrome.shows_custom_window_controls();
    #[cfg(target_os = "windows")]
    let caption_controls = controls::window_controls_native(caption_font);
    #[cfg(not(target_os = "windows"))]
    let caption_controls = controls::window_controls_interactive(icon_font);

    let uses_macos_native_titlebar = chrome == WindowChromeStyle::MacNativeTitlebar;
    let mut header_slot = None::<Entity>;
    let header_root = (
        WindowHeaderRoot,
        EditorEntity,
        Node {
            position_type: PositionType::Relative,
            width: percent(100),
            height: px(tokens::WINDOW_HEADER_HEIGHT),
            ..default()
        },
    );
    let mut header_spawner = parent.spawn(header_root);
    if !uses_macos_native_titlebar {
        header_spawner.insert(BackgroundColor(tokens::WINDOW_BG));
    }
    header_spawner
        .with_children(|header| {
            header_slot = Some(spawn_foreground_row(
                header,
                macos_traffic_light_inset,
                uses_macos_native_titlebar,
                show_custom_controls,
                caption_controls,
            ));
        });
    return header_slot.expect("window header content slot spawned");
}

fn spawn_foreground_row(
    parent: &mut ChildSpawnerCommands,
    macos_traffic_light_inset: f32,
    uses_macos_native_titlebar: bool,
    show_custom_controls: bool,
    caption_controls: impl Bundle,
) -> Entity {
    let mut header_slot = None::<Entity>;
    parent
        .spawn((
            EditorEntity,
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
                row.spawn(header_chrome_background(macos_traffic_light_inset));
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

fn header_chrome_background(macos_traffic_light_inset: f32) -> impl Bundle {
    (
        MacosHeaderChromeInset,
        EditorEntity,
        Pickable::IGNORE,
        Node {
            position_type: PositionType::Absolute,
            top: px(0.0),
            left: px(macos_traffic_light_inset),
            right: px(0.0),
            height: percent(100),
            ..default()
        },
        BackgroundColor(tokens::WINDOW_BG),
    )
}

fn header_drag_backplate(macos_traffic_light_inset: f32) -> impl Bundle {
    (
        MacosHeaderChromeInset,
        WindowHeaderDragRegion,
        EditorEntity,
        Node {
            position_type: PositionType::Absolute,
            top: px(0.0),
            left: px(macos_traffic_light_inset),
            right: px(0.0),
            height: percent(100),
            ..default()
        },
    )
}

fn caption_controls_slot(show_custom_controls: bool, caption_controls: impl Bundle) -> impl Bundle {
    (
        EditorEntity,
        Pickable::IGNORE,
        Node {
            flex_shrink: 0.0,
            height: percent(100),
            #[cfg(not(target_os = "windows"))]
            padding: UiRect::right(px(tokens::SPACING_MD)),
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
    )
}

fn header_content_slot() -> impl Bundle {
    (
        WindowShellHeaderSlot,
        EditorEntity,
        Pickable::IGNORE,
        Node {
            flex_grow: 1.0,
            min_width: px(0.0),
            height: percent(100),
            overflow: Overflow::clip(),
            ..default()
        },
    )
}

fn on_drag_region_press(
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
pub fn native_hit_test_client(content: impl Bundle) -> impl Bundle {
    (NativeHitTestClient, content)
}
