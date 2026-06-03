//! Platform-specific primary-window chrome strategy (hybrid native + custom UI).
//!
//! - **Windows**: borderless window with Win32 non-client hit testing for drag, resize, and caption buttons.
//! - **macOS**: native traffic lights with a transparent integrated title bar.
//! - **Linux**: client-side custom chrome by default; opt into server decorations via
//!   the decorations preference (`server`/`system` vs `client`).

use bevy::prelude::*;
use bevy::window::Window;

/// Default environment variable consulted by [`WindowChromeStyle::current`] on Linux/FreeBSD.
pub const DEFAULT_DECORATIONS_ENV: &str = "BEVY_WINDOW_DECORATIONS";

/// How the primary window integrates OS window chrome with the application's header UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Resource)]
pub enum WindowChromeStyle {
    /// Borderless window: custom caption buttons and edge resize handles.
    CustomClient,
    /// macOS: keep native traffic lights; draw the header under a transparent title bar.
    MacNativeTitlebar,
    /// OS-provided title bar (server-side decorations when available).
    SystemServer,
}

impl WindowChromeStyle {
    /// Resolve the chrome strategy for this process, reading [`DEFAULT_DECORATIONS_ENV`]
    /// for the Linux/FreeBSD client-vs-server preference.
    pub fn current() -> Self {
        let decorations = std::env::var(DEFAULT_DECORATIONS_ENV).ok();
        return Self::resolve(decorations.as_deref());
    }

    /// Resolve the chrome strategy, taking an explicit decorations preference.
    ///
    /// `decorations` only affects Linux/FreeBSD: `"server"`/`"system"` selects server-side
    /// decorations, `"client"`/`None` selects client-side chrome. Other targets ignore it.
    pub fn resolve(decorations: Option<&str>) -> Self {
        #[cfg(target_os = "macos")]
        {
            let _ = decorations;
            return WindowChromeStyle::MacNativeTitlebar;
        }

        #[cfg(any(target_os = "linux", target_os = "freebsd"))]
        {
            return match decorations
                .map(str::trim)
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("server") | Some("system") => WindowChromeStyle::SystemServer,
                Some("client") | None => WindowChromeStyle::CustomClient,
                Some(other) => {
                    bevy::log::warn!(
                        "bevy_window_chrome: unknown window decorations preference {other:?}; \
                         expected \"client\" or \"server\", using client-side chrome"
                    );
                    WindowChromeStyle::CustomClient
                }
            };
        }

        #[cfg(target_os = "windows")]
        {
            let _ = decorations;
            return WindowChromeStyle::CustomClient;
        }

        #[cfg(not(any(
            target_os = "macos",
            target_os = "windows",
            target_os = "linux",
            target_os = "freebsd"
        )))]
        {
            let _ = decorations;
            return WindowChromeStyle::SystemServer;
        }
    }

    pub fn shows_custom_window_controls(self) -> bool {
        return self == WindowChromeStyle::CustomClient;
    }

    pub fn uses_resize_edge_overlay(self) -> bool {
        return self == WindowChromeStyle::CustomClient && !self.uses_native_hit_testing();
    }

    pub fn uses_shell_corner_radius(self) -> bool {
        return self == WindowChromeStyle::CustomClient;
    }

    pub fn uses_native_hit_testing(self) -> bool {
        return match self {
            WindowChromeStyle::CustomClient => cfg!(target_os = "windows"),
            WindowChromeStyle::MacNativeTitlebar | WindowChromeStyle::SystemServer => true,
        };
    }

    pub fn uses_app_drag_handler(self) -> bool {
        return match self {
            WindowChromeStyle::MacNativeTitlebar => true,
            WindowChromeStyle::CustomClient => !cfg!(target_os = "windows"),
            WindowChromeStyle::SystemServer => false,
        };
    }

    pub fn uses_app_caption_button_handlers(self) -> bool {
        return self.shows_custom_window_controls() && !self.uses_native_hit_testing();
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
        WindowChromeStyle::SystemServer => Window {
            decorations: true,
            ..default()
        },
    };
}
