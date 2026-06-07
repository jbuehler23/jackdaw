//! Primary-window shell: chrome root, header, body slot, and resize overlay.

use bevy::prelude::*;

use crate::header::spawn_window_header;
use crate::resize::spawn_resize_edge_overlay_if_needed;
use crate::{WindowChromeTheme, WindowShellRoot};

/// Unstyled flex column that fills the area below the window header.
#[derive(Component)]
pub struct WindowShellContent;

/// Header and body entities returned by [`spawn_window_shell`].
#[derive(Clone, Copy, Debug)]
pub struct WindowShellSlots {
    pub header: Entity,
    pub body: Entity,
}

/// Spawns a UI camera, the window shell, and returns header/body slots for screen content.
///
/// `screen` is a caller marker copied onto the UI camera and shell root (useful for despawning
/// screen's chrome as a unit). `caption_font` is the icon font for client-side caption buttons
/// (Segoe on Windows; Lucide or compatible on Linux / FreeBSD).
pub fn spawn_window_shell<S: Component + Copy>(
    commands: &mut Commands,
    theme: &WindowChromeTheme,
    caption_font: Handle<Font>,
    screen: S,
) -> WindowShellSlots {
    commands.spawn((
        Camera2d,
        Camera {
            // note that this does not work on Windows
            clear_color: ClearColorConfig::Custom(Color::NONE),
            ..default()
        },
        screen,
    ));
    let mut header_slot = None::<Entity>;
    let mut body_slot = None::<Entity>;
    commands
        .spawn((
            screen,
            WindowShellRoot,
            BackgroundColor(theme.window_background),
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                overflow: Overflow::clip(),
                #[cfg(any(target_os = "linux", target_os = "freebsd"))]
                border_radius: BorderRadius::all(px(theme.linux_corner_radius)),
                ..default()
            },
        ))
        .with_children(|shell| {
            header_slot = Some(spawn_window_header(shell, theme, caption_font));
            body_slot = Some(
                shell
                    .spawn((
                        WindowShellContent,
                        Node {
                            width: percent(100),
                            height: percent(100),
                            flex_grow: 1.0,
                            min_height: px(0.0),
                            flex_direction: FlexDirection::Column,
                            overflow: Overflow::clip(),
                            ..default()
                        },
                    ))
                    .id(),
            );
            #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
            spawn_resize_edge_overlay_if_needed(shell);
        });
    return WindowShellSlots {
        header: header_slot.expect("window shell header slot spawned"),
        body: body_slot.expect("window shell body slot spawned"),
    };
}
