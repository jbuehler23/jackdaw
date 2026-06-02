//! Platform-specific primary-window chrome strategy (Zed-style hybrid).
//!
//! - **Windows**: borderless window with Win32 non-client hit testing for drag, resize, and caption buttons.
//! - **macOS**: native traffic lights with a transparent integrated title bar.
//! - **Linux**: client-side custom chrome by default; opt into server decorations via
//!   `JACKDAW_WINDOW_DECORATIONS=server`.

use bevy::prelude::*;
use bevy::window::Window;

/// How the primary window integrates OS window chrome with jackdaw's header UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Resource)]
pub enum WindowChromeStyle {
    /// Borderless window: custom caption buttons and edge resize handles.
    CustomClient,
    /// macOS: keep native traffic lights; draw the header under a transparent title bar.
    MacNativeTitlebar,
    /// Linux: OS-provided title bar (server-side decorations when available).
    SystemServer,
}

impl WindowChromeStyle {
    /// Resolve the chrome strategy for this process (read once at startup).
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            return WindowChromeStyle::MacNativeTitlebar;
        }

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            return match std::env::var("JACKDAW_WINDOW_DECORATIONS")
                .ok()
                .as_deref()
                .map(str::trim)
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("server") | Some("system") => WindowChromeStyle::SystemServer,
                Some("client") | None => WindowChromeStyle::CustomClient,
                Some(other) => {
                    bevy::log::warn!(
                        "jackdaw: unknown JACKDAW_WINDOW_DECORATIONS={other:?}; \
                         expected \"client\" or \"server\", using client-side chrome"
                    );
                    WindowChromeStyle::CustomClient
                }
            };
        }

        #[cfg(target_os = "windows")]
        {
            return WindowChromeStyle::CustomClient;
        }

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        {
            WindowChromeStyle::SystemServer
        }
    }

    pub fn shows_custom_window_controls(self) -> bool {
        self == WindowChromeStyle::CustomClient
    }

    pub fn uses_resize_edge_overlay(self) -> bool {
        self == WindowChromeStyle::CustomClient && !self.uses_native_hit_testing()
    }

    pub fn uses_shell_corner_radius(self) -> bool {
        self == WindowChromeStyle::CustomClient
    }

    pub fn uses_native_hit_testing(self) -> bool {
        match self {
            WindowChromeStyle::CustomClient => cfg!(target_os = "windows"),
            WindowChromeStyle::MacNativeTitlebar | WindowChromeStyle::SystemServer => true,
        }
    }

    pub fn uses_app_drag_handler(self) -> bool {
        match self {
            WindowChromeStyle::MacNativeTitlebar => true,
            WindowChromeStyle::CustomClient => !cfg!(target_os = "windows"),
            WindowChromeStyle::SystemServer => false,
        }
    }

    pub fn uses_app_caption_button_handlers(self) -> bool {
        self.shows_custom_window_controls() && !self.uses_native_hit_testing()
    }

    pub fn macos_traffic_light_inset(self) -> f32 {
        if self == WindowChromeStyle::MacNativeTitlebar {
            jackdaw_feathers::tokens::MACOS_TRAFFIC_LIGHT_INSET
        } else {
            0.0
        }
    }
}

/// Primary-window attributes for the current platform chrome strategy.
pub fn primary_window_attributes() -> Window {
    match WindowChromeStyle::current() {
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
        WindowChromeStyle::SystemServer => Window {
            decorations: true,
            ..default()
        },
    }
}
