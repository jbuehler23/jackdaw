//! Native-looking custom window chrome for Bevy apps.
//!
//! Provides a draggable title bar, caption buttons, and resize handles with per-platform chrome:
//!
//! - **Windows**: borderless client-side chrome; DWM rounds the HWND.
//! - **Linux / FreeBSD**: borderless client-side chrome; Bevy UI rounds corners with a transparent
//!   window background.
//! - **macOS**: native traffic lights with a transparent integrated title bar.
//!
//! Colors and metrics come from a [`WindowChromeTheme`] supplied by the host application.
//! Client-side caption buttons use Segoe icon glyphs on Windows and Lucide-compatible glyphs on
//! Linux / FreeBSD.

mod controls;
mod header;
mod icon;
#[cfg(target_os = "macos")]
mod macos_titlebar;
mod plugin;
mod resize;
mod shell;
mod window;

pub use controls::{
    CaptionFont, WindowControlsClose, WindowControlsMaximize, WindowControlsMinimize,
    window_caption_controls,
};
pub use header::{
    MacosHeaderContentInset, WindowHeaderContentSlot, WindowHeaderDragRegion, WindowHeaderRoot,
    spawn_window_header,
};
pub use icon::WindowIconPlugin;
pub use plugin::{CaptionTheme, WindowChromePlugin, WindowChromeTheme};
pub use resize::{resize_edge_overlay, spawn_resize_edge_overlay_if_needed};
pub use shell::{WindowShellContent, WindowShellSlots, spawn_window_shell};
pub use window::{primary_window_attributes, primary_window_is_maximized};

use bevy::prelude::Component;

/// Marker added to every entity spawned by this crate's window chrome.
///
/// Host apps can react to this (for example with an `On<Add, WindowChromeEntity>` observer) to
/// stamp their own cleanup/exclusion markers onto the chrome hierarchy.
#[derive(Component, Copy, Clone, Default)]
pub struct WindowChromeEntity;

/// Outer shell column that hosts the window header and body slot.
#[derive(Component)]
pub struct WindowShellRoot;
