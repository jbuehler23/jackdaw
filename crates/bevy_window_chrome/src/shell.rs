//! Primary-window shell: chrome root, header, body slot, and resize overlay.

use bevy::prelude::*;

use crate::chrome::WindowChromeStyle;
use crate::header::spawn_window_header;
use crate::resize::spawn_resize_edge_overlay_if_needed;
use crate::{WindowChromeTheme, WindowShellRoot};

/// Unstyled flex column that fills the area below the window header.
#[derive(Component)]
pub struct WindowShellContent;

/// Header and body entities returned by [`spawn_window_shell`].
pub type WindowShellSlots = (Entity, Entity);

/// Spawns a UI camera, the window shell, and returns `(header_slot, body_slot)` for screen content.
///
/// `screen` is a caller marker copied onto the UI camera and shell root (useful for despawning a
/// screen's chrome as a unit). `caption_controls` is the minimize/maximize/close cluster bundle
/// (see [`spawn_window_header`]).
pub fn spawn_window_shell<S: Component + Copy>(
    commands: &mut Commands,
    style: WindowChromeStyle,
    theme: &WindowChromeTheme,
    caption_controls: impl Bundle,
    screen: S,
) -> WindowShellSlots {
    commands.spawn((Camera2d, screen));
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
                ..default()
            },
        ))
        .with_children(|shell| {
            header_slot = Some(spawn_window_header(shell, theme, style, caption_controls));
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
            spawn_resize_edge_overlay_if_needed(shell, style, theme.header_height);
        });
    return (
        header_slot.expect("window shell header slot spawned"),
        body_slot.expect("window shell body slot spawned"),
    );
}
