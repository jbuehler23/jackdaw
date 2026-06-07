//! Native-looking custom window chrome for Bevy apps.
//!
//! Provides a borderless or integrated primary-window shell with a draggable title bar, caption
//! buttons, and resize handles. Platform behavior is fixed per target:
//!
//! - **Windows**: borderless client-side chrome; DWM rounds the HWND corner.
//! - **Linux / FreeBSD**: borderless client-side chrome; Bevy UI rounds the shell with a
//!   transparent window background.
//! - **macOS**: native traffic lights with a transparent integrated title bar.
//!
//! Colors and metrics come from a [`WindowChromeTheme`] you supply to [`WindowChromePlugin`].
//! Client-side caption buttons (Windows, Linux, FreeBSD) expect a
//! Segoe icon font on Windows and Lucide-compatible glyphs elsewhere.

mod caption_controls;
#[cfg(target_os = "macos")]
mod macos_titlebar;
mod title_bar;
mod plugin;
mod resize;
mod shell;
mod window;

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
pub use caption_controls::window_controls;
pub use caption_controls::{
    CaptionFont, WindowControlsClose, WindowControlsMaximize, WindowControlsMinimize,
};
pub use title_bar::{
    WindowTitleBarContentSlot, WindowTitleBarDragRegion, WindowTitleBarRoot, spawn_window_title_bar,
};
pub use plugin::{CaptionTheme, WindowChromePlugin, WindowChromeTheme};
pub use resize::resize_edge_overlay;
pub use shell::{WindowShellContent, WindowShellSlots, spawn_window_shell};
pub use window::{primary_window_attributes, primary_window_is_maximized};

use bevy::prelude::Component;

/// Marker added to every entity spawned by this crate's window chrome.
///
/// Host apps can react to this (for example with an `On<Add, WindowChromeEntity>` observer) to
/// stamp their own cleanup/exclusion markers onto the chrome hierarchy.
#[derive(Component, Copy, Clone, Default)]
pub struct WindowChromeEntity;

/// Outer shell column that hosts the window title bar and body slot.
#[derive(Component)]
pub struct WindowShellRoot;
