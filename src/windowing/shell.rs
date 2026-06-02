//! Shared primary-window shell: chrome root, header, body slot, resize overlay.

use bevy::prelude::*;
use jackdaw_feathers::icons::IconFont;
use jackdaw_feathers::tokens;

use super::WindowShellRoot;
#[cfg(target_os = "windows")]
use super::WindowsCaptionFont;
use super::chrome::WindowChromeStyle;
use super::header::spawn_window_header;
use super::repo_link::JackdawIcon;
use super::resize::spawn_resize_edge_overlay_if_needed;

/// Unstyled flex column that fills the area below the window header.
#[derive(Component)]
pub struct WindowShellContent;

/// Header and body entities returned by [`spawn_window_shell`].
pub type WindowShellSlots = (Entity, Entity);

/// Spawns a UI camera, the window shell, and returns `(header_slot, body_slot)` for screen content.
pub fn spawn_window_shell<S: Component + Copy>(
    commands: &mut Commands,
    chrome: WindowChromeStyle,
    #[allow(unused_variables)] icon_font: &IconFont,
    jackdaw_icon: &JackdawIcon,
    #[cfg(target_os = "windows")] caption_font: &WindowsCaptionFont,
    screen: S,
) -> WindowShellSlots {
    commands.spawn((Camera2d, screen));
    let mut header_slot = None::<Entity>;
    let mut body_slot = None::<Entity>;
    commands
        .spawn((
            screen,
            WindowShellRoot,
            BackgroundColor(tokens::WINDOW_BG),
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                overflow: Overflow::clip(),
                ..Default::default()
            },
        ))
        .with_children(|shell| {
            header_slot = Some({
                #[cfg(target_os = "windows")]
                {
                    spawn_window_header(
                        shell,
                        caption_font.0.clone(),
                        jackdaw_icon.0.clone(),
                        chrome,
                    )
                }
                #[cfg(not(target_os = "windows"))]
                {
                    spawn_window_header(
                        shell,
                        icon_font.0.clone(),
                        jackdaw_icon.0.clone(),
                        chrome,
                    )
                }
            });
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
                            ..Default::default()
                        },
                    ))
                    .id(),
            );
            #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
            spawn_resize_edge_overlay_if_needed(shell, chrome);
        });
    return (
        header_slot.expect("window shell header slot spawned"),
        body_slot.expect("window shell body slot spawned"),
    );
}
