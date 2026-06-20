//! Window attributes and native window state helpers.

use bevy::prelude::*;
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
use bevy::window::CompositeAlphaMode;
#[cfg(target_os = "windows")]
use bevy::window::WindowCreated;
use bevy::window::{PrimaryWindow, Window};
use bevy::winit::WINIT_WINDOWS;

pub fn primary_window_attributes() -> Window {
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        Window {
            decorations: false,
            transparent: true,
            composite_alpha_mode: CompositeAlphaMode::PreMultiplied,
            ..default()
        }
    }

    #[cfg(target_os = "macos")]
    {
        Window {
            decorations: true,
            titlebar_transparent: true,
            fullsize_content_view: true,
            titlebar_show_title: false,
            titlebar_show_buttons: true,
            ..default()
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "freebsd")))]
    Window {
        decorations: false,
        ..default()
    }
}

/// Toggles the primary window between maximized and restored.
pub(crate) fn toggle_primary_window_maximized(
    mut windows: Query<(Entity, &mut Window), With<PrimaryWindow>>,
) {
    let Ok((window_entity, mut window)) = windows.single_mut() else {
        return;
    };
    let next_maximized = !primary_window_is_maximized(window_entity);
    window.set_maximized(next_maximized);
}

/// Whether the primary window is currently maximized.
pub fn primary_window_is_maximized(window_entity: Entity) -> bool {
    #[cfg(any(target_arch = "wasm32", target_os = "ios", target_os = "android"))]
    return false;

    WINIT_WINDOWS.with(|windows_cell| {
        let winit_windows = windows_cell.borrow();
        let Some(backend) = winit_windows.get_window(window_entity) else {
            return false;
        };
        if backend.is_maximized() {
            return true;
        }
        #[cfg(target_os = "windows")]
        {
            win32_window_is_maximized(backend)
        }
        #[cfg(not(target_os = "windows"))]
        {
            false
        }
    })
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
    unsafe { IsZoomed(hwnd) != 0 }
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
