//! Per-platform primary-window attributes.

use bevy::prelude::*;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use bevy::window::CompositeAlphaMode;
use bevy::window::Window;

/// Primary-window attributes for the current compilation target.
///
/// Feed the result into Bevy's `WindowPlugin { primary_window: Some(..) }`.
pub fn primary_window_attributes() -> Window {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        return Window {
            decorations: false,
            transparent: true,
            composite_alpha_mode: CompositeAlphaMode::PreMultiplied,
            ..default()
        };
    }

    #[cfg(target_os = "macos")]
    {
        return Window {
            decorations: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
            titlebar_show_title: false,
            titlebar_show_buttons: true,
            ..default()
        };
    }

    return Window {
        decorations: false,
        ..default()
    };
}
