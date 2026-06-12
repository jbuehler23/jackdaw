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
//! Client-side caption buttons (Windows, Linux, FreeBSD) load their own icon font: Segoe on
//! Windows when available, otherwise a small embedded Lucide icon font.

#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
mod caption_controls;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
pub use caption_controls::{CaptionButton, CaptionFont, window_controls};
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
mod resize;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
pub use resize::resize_edge_overlay;
#[cfg(target_os = "macos")]
mod macos_titlebar;
mod plugin;
mod shell;
mod title_bar;
mod window;

pub use plugin::{CaptionTheme, WindowChromePlugin, WindowChromeTheme};
pub use shell::{WindowShellContent, WindowShellRoot, WindowShellSlots, spawn_window_shell};
pub use title_bar::{
    WindowTitleBarContentSlot, WindowTitleBarDragRegion, WindowTitleBarRoot, spawn_window_title_bar,
};
pub use window::{primary_window_attributes, primary_window_is_maximized};

use bevy::prelude::Component;

/// Marker added to every entity spawned by this crate's window chrome.
///
/// Host apps can react to this (for example with an `On<Add, WindowChromeEntity>` observer) to
/// stamp their own cleanup/exclusion markers onto the chrome hierarchy.
#[derive(Component, Copy, Clone, Default)]
pub struct WindowChromeEntity;
