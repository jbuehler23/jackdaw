//! Platform-specific primary-window chrome strategy (hybrid native + custom UI).
//!
//! - **Windows / Linux / FreeBSD**: borderless client-side chrome with Bevy-driven drag, resize,
//!   and caption buttons.
//! - **macOS**: native traffic lights with a transparent integrated title bar.

use bevy::prelude::*;
use bevy::window::Window;

/// How the primary window integrates OS window chrome with the application's header UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Resource)]
pub enum WindowChromeStyle {
    /// Borderless window: custom caption buttons and edge resize handles.
    CustomClient,
    /// macOS: keep native traffic lights; draw the header under a transparent title bar.
    MacNativeTitlebar,
}

impl WindowChromeStyle {
    /// Platform-default chrome strategy for this process.
    pub fn platform_default() -> Self {
        #[cfg(target_os = "macos")]
        {
            return WindowChromeStyle::MacNativeTitlebar;
        }

        #[cfg(not(target_os = "macos"))]
        {
            return WindowChromeStyle::CustomClient;
        }
    }

    pub fn shows_custom_window_controls(self) -> bool {
        return self == WindowChromeStyle::CustomClient;
    }

    pub fn uses_resize_edge_overlay(self) -> bool {
        return self == WindowChromeStyle::CustomClient;
    }

    pub fn uses_shell_corner_radius(self) -> bool {
        return self == WindowChromeStyle::CustomClient;
    }

    pub fn uses_app_caption_button_handlers(self) -> bool {
        return self.shows_custom_window_controls();
    }
}

/// Primary-window attributes for the given platform chrome strategy.
///
/// Feed the result into Bevy's `WindowPlugin { primary_window: Some(..) }`.
pub fn primary_window_attributes(style: WindowChromeStyle) -> Window {
    return match style {
        WindowChromeStyle::CustomClient => Window {
            decorations: false,
            ..default()
        },
        WindowChromeStyle::MacNativeTitlebar => Window {
            decorations: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
            titlebar_show_title: false,
            titlebar_show_buttons: true,
            ..default()
        },
    };
}
