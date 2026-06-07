//! Window attributes and native window state helpers.

use bevy::prelude::*;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use bevy::window::CompositeAlphaMode;
use bevy::window::{Window, WindowCreated};
use bevy::winit::WINIT_WINDOWS;

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

#[cfg(target_os = "windows")]
pub(crate) fn apply_windows_corner_round(
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
