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
mod resize;
mod shell;

pub use controls::{
    CaptionFont, WindowControlsClose, WindowControlsMaximize, WindowControlsMinimize,
    window_caption_controls,
};
pub use header::{
    MacosHeaderContentInset, WindowHeaderContentSlot, WindowHeaderDragRegion, WindowHeaderRoot,
    spawn_window_header,
};
pub use icon::WindowIconPlugin;
pub use resize::{resize_edge_overlay, spawn_resize_edge_overlay_if_needed};
pub use shell::{WindowShellContent, WindowShellSlots, spawn_window_shell};

use bevy::prelude::*;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use bevy::window::CompositeAlphaMode;
use bevy::window::{PrimaryWindow, Window, WindowCreated, WindowMode};
use bevy::winit::WINIT_WINDOWS;

use resize::WindowResizeRoot;

/// Marker added to every entity spawned by this crate's window chrome.
///
/// Host apps can react to this (for example with an `On<Add, WindowChromeEntity>` observer) to
/// stamp their own cleanup/exclusion markers onto the chrome hierarchy.
#[derive(Component, Copy, Clone, Default)]
pub struct WindowChromeEntity;

/// Outer shell column that hosts the window header and body slot.
#[derive(Component)]
pub struct WindowShellRoot;

/// Colors and metrics used to style the window chrome.
///
/// Supply this to [`WindowChromePlugin`] and the spawn helpers so the chrome matches the host
/// application's look. [`WindowChromeTheme::default`] provides neutral, native-ish values.
#[derive(Resource, Clone, Debug)]
pub struct WindowChromeTheme {
    /// Height of the title-bar header row, in logical pixels.
    pub header_height: f32,
    /// Background color of the shell and header.
    pub window_background: Color,
    /// Left inset reserved for macOS traffic lights when the title bar is integrated.
    pub macos_traffic_light_inset: f32,
    /// Horizontal origin of the macOS traffic lights within the title bar.
    pub macos_traffic_light_position_x: f32,
    /// Corner radius for the window shell and header on Linux / FreeBSD, in logical pixels.
    pub linux_corner_radius: f32,
    /// Styling for client-side caption buttons.
    pub caption: CaptionTheme,
}

/// Styling for client-side caption buttons.
#[derive(Clone, Debug)]
pub struct CaptionTheme {
    /// Foreground (glyph) color for minimize / maximize / close.
    pub foreground: Color,
    /// Hover/pressed background for the minimize and maximize buttons.
    pub button_hover_background: Color,
    /// Hover background for the close button.
    pub close_hover_background: Color,
    /// Pressed background for the close button.
    pub close_active_background: Color,
    /// Width of each caption button, in logical pixels.
    pub button_width: f32,
    /// Glyph font size, in logical pixels.
    pub glyph_size: f32,
}

impl Default for CaptionTheme {
    fn default() -> Self {
        return Self {
            foreground: Color::srgb(0.925, 0.925, 0.925),
            button_hover_background: Color::srgb(0.165, 0.165, 0.180),
            close_hover_background: Color::srgb(232.0 / 255.0, 17.0 / 255.0, 32.0 / 255.0),
            close_active_background: Color::srgba(232.0 / 255.0, 17.0 / 255.0, 32.0 / 255.0, 0.8),
            button_width: 36.0,
            glyph_size: 10.0,
        };
    }
}

impl Default for WindowChromeTheme {
    fn default() -> Self {
        return Self {
            header_height: 36.0,
            window_background: Color::srgb(0.122, 0.122, 0.141),
            macos_traffic_light_inset: 78.0,
            macos_traffic_light_position_x: 12.0,
            linux_corner_radius: 8.0,
            caption: CaptionTheme::default(),
        };
    }
}

/// Installs the window chrome: shell-state sync, caption controls, and drag/resize handlers.
///
/// The window itself must be created with [`primary_window_attributes`], typically by feeding
/// it into Bevy's `WindowPlugin`.
pub struct WindowChromePlugin {
    pub theme: WindowChromeTheme,
}

impl WindowChromePlugin {
    pub fn new(theme: WindowChromeTheme) -> Self {
        return Self { theme };
    }
}

impl Plugin for WindowChromePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.theme.clone());

        #[cfg(target_os = "macos")]
        macos_titlebar::set_layout_metrics(macos_titlebar::MacChromeMetrics {
            header_height: self.theme.header_height as f64,
            traffic_light_position_x: self.theme.macos_traffic_light_position_x as f64,
        });

        controls::build(app);

        app.add_observer(header::on_drag_region_press);

        #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
        {
            #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
            {
                app.add_observer(resize::on_resize_edge_press);
            }
            app.add_systems(PostUpdate, sync_window_shell_state);
        }

        #[cfg(target_os = "windows")]
        {
            app.add_systems(PostUpdate, apply_windows_corner_round);
        }
    }
}

#[cfg(target_os = "windows")]
fn apply_windows_corner_round(
    _main_thread: bevy::ecs::system::NonSendMarker,
    mut created: MessageReader<WindowCreated>,
) {
    use winit::platform::windows::{CornerPreference, WindowExtWindows};

    for event in created.read() {
        WINIT_WINDOWS.with(|windows_cell| {
            let winit_windows = windows_cell.borrow();
            let Some(backend) = winit_windows.get_window(event.window) else {
                return;
            };
            backend.set_corner_preference(CornerPreference::Round);
        });
    }
}

/// Whether the primary window is currently maximized.
#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub fn primary_window_is_maximized(window_entity: Entity) -> bool {
    return WINIT_WINDOWS.with(|windows_cell| {
        let winit_windows = windows_cell.borrow();
        let Some(backend) = winit_windows.get_window(window_entity) else {
            return false;
        };
        if backend.is_maximized() {
            return true;
        }
        #[cfg(target_os = "windows")]
        {
            return win32_window_is_maximized(backend);
        }
        #[cfg(not(target_os = "windows"))]
        {
            return false;
        }
    });
}

#[cfg(target_os = "windows")]
fn win32_window_is_maximized(backend: &winit::window::Window) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::IsZoomed;

    let Ok(handle) = backend.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(window_handle) = handle.as_raw() else {
        return false;
    };
    let hwnd = window_handle.hwnd.get() as HWND;
    return unsafe { IsZoomed(hwnd) != 0 };
}

/// Whether the primary window is currently maximized (false on platforms without winit windows).
#[cfg(any(target_arch = "wasm32", target_os = "ios", target_os = "android"))]
pub fn primary_window_is_maximized(_window_entity: Entity) -> bool {
    return false;
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
fn sync_window_shell_state(
    _main_thread: bevy::ecs::system::NonSendMarker,
    #[cfg(target_os = "macos")] theme: Res<WindowChromeTheme>,
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    mut shell_nodes: ParamSet<(
        Query<&mut Node, With<WindowResizeRoot>>,
        Query<&mut Node, With<MacosHeaderContentInset>>,
    )>,
    #[cfg(target_os = "macos")] mut previous_fills_work_area: Local<Option<bool>>,
) {
    let Ok((entity, window)) = windows.single() else {
        return;
    };

    let is_fullscreen = !matches!(window.mode, WindowMode::Windowed);
    let is_maximized = primary_window_is_maximized(entity);
    let is_floating_window = !is_fullscreen && !is_maximized;

    #[cfg(target_os = "macos")]
    macos_titlebar::sync_window_shell_state(
        entity,
        is_fullscreen,
        theme.macos_traffic_light_inset,
        &mut shell_nodes.p1(),
        &mut previous_fills_work_area,
    );

    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    {
        for mut node in shell_nodes.p0().iter_mut() {
            node.display = if is_floating_window {
                Display::Flex
            } else {
                Display::None
            };
        }
    }
}

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
