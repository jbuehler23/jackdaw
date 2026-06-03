//! Win32 non-client hit testing and caption button actions.

use std::sync::{Mutex, OnceLock};

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::*;
use bevy::window::{PrimaryWindow, WindowCreated};
use bevy::winit::WINIT_WINDOWS;
use raw_window_handle::RawWindowHandle;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
use windows_sys::Win32::UI::HiDpi::GetSystemMetricsForDpi;
use windows_sys::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetClientRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCLOSE, HTLEFT, HTMAXBUTTON,
    HTMINBUTTON, HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, IsZoomed, PostMessageW, SM_CXPADDEDBORDER,
    SM_CXSIZEFRAME, SM_CYSIZEFRAME, SW_MAXIMIZE, SW_MINIMIZE, SW_NORMAL, ShowWindow, WM_CLOSE,
    WM_NCHITTEST, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP,
};
use winit::raw_window_handle::HasWindowHandle;

use super::{NativeCaptionButton, NativeHitTestRegions};

const SUBCLASS_ID: usize = 0x6A616B64; // "jack"

static REGIONS: OnceLock<Mutex<NativeHitTestRegions>> = OnceLock::new();
static CAPTION_PRESSED: OnceLock<Mutex<Option<NativeCaptionButton>>> = OnceLock::new();

pub(super) fn publish_regions(regions: &NativeHitTestRegions) {
    let slot = REGIONS.get_or_init(|| Mutex::new(NativeHitTestRegions::default()));
    if let Ok(mut locked) = slot.lock() {
        *locked = regions.clone();
    }
}

/// Client-area cursor position in physical pixels.
///
/// Bevy's `Window::physical_cursor_position` is `None` over non-client caption hits
/// (`HTMINBUTTON` / `HTMAXBUTTON` / `HTCLOSE`), so hover must read the cursor from Win32.
pub(super) fn primary_window_client_cursor(window_entity: Entity) -> Option<Vec2> {
    return WINIT_WINDOWS.with(|windows_cell| {
        let winit_windows = windows_cell.borrow();
        let backend = winit_windows.get_window(window_entity)?;
        let handle = backend.window_handle().ok()?;
        let RawWindowHandle::Win32(window_handle) = handle.as_raw() else {
            return None;
        };
        let hwnd = window_handle.hwnd.get() as HWND;
        return client_cursor_physical_position(hwnd);
    });
}

pub(super) fn client_cursor_physical_position(hwnd: HWND) -> Option<Vec2> {
    let mut point = POINT { x: 0, y: 0 };
    let ok = unsafe { GetCursorPos(&mut point) };
    if ok == 0 {
        return None;
    }
    let ok = unsafe { ScreenToClient(hwnd, &mut point) };
    if ok == 0 {
        return None;
    }
    return Some(Vec2::new(point.x as f32, point.y as f32));
}

pub(super) fn caption_button_pressed() -> Option<NativeCaptionButton> {
    return CAPTION_PRESSED
        .get()
        .and_then(|slot| slot.lock().ok())
        .and_then(|locked| *locked);
}

fn set_caption_button_pressed(pressed: Option<NativeCaptionButton>) {
    let slot = CAPTION_PRESSED.get_or_init(|| Mutex::new(None));
    if let Ok(mut locked) = slot.lock() {
        *locked = pressed;
    }
}

pub(super) fn install_primary_window_subclass(
    _main_thread: NonSendMarker,
    mut created: MessageReader<WindowCreated>,
    primary_window: Query<Entity, With<PrimaryWindow>>,
) {
    let Ok(primary) = primary_window.single() else {
        return;
    };

    for event in created.read() {
        if event.window != primary {
            continue;
        }

        WINIT_WINDOWS.with(|windows_cell| {
            let winit_windows = windows_cell.borrow();
            let Some(backend) = winit_windows.get_window(event.window) else {
                bevy::log::warn!(
                    "bevy_window_chrome: winit backend window missing when installing native hit-test subclass"
                );
                return;
            };
            let Ok(handle) = backend.window_handle() else {
                bevy::log::warn!(
                    "bevy_window_chrome: failed to read Win32 window handle for native hit testing"
                );
                return;
            };
            let RawWindowHandle::Win32(window_handle) = handle.as_raw() else {
                return;
            };
            let hwnd = window_handle.hwnd.get() as HWND;
            let result =
                unsafe { SetWindowSubclass(hwnd, Some(subclass_window_proc), SUBCLASS_ID, 0) };
            if result == 0 {
                bevy::log::warn!(
                    "bevy_window_chrome: SetWindowSubclass failed for native window hit testing"
                );
            }
        });
    }
}

unsafe extern "system" fn subclass_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _subclass_id: usize,
    _reference_data: usize,
) -> LRESULT {
    match message {
        WM_NCHITTEST => {
            if let Some(code) = hit_test(hwnd, lparam) {
                return code as LRESULT;
            }
        }
        WM_NCLBUTTONDOWN => {
            if let Some(button) = caption_button_from_hit(wparam) {
                set_caption_button_pressed(Some(button));
                return 0;
            }
        }
        WM_NCLBUTTONUP => {
            if handle_caption_button_up(hwnd, wparam) {
                set_caption_button_pressed(None);
                return 0;
            }
            set_caption_button_pressed(None);
        }
        _ => {}
    }
    return unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
}

fn caption_button_from_hit(wparam: WPARAM) -> Option<NativeCaptionButton> {
    return match wparam as u32 {
        x if x == HTMINBUTTON as u32 => Some(NativeCaptionButton::Minimize),
        x if x == HTMAXBUTTON as u32 => Some(NativeCaptionButton::Maximize),
        x if x == HTCLOSE as u32 => Some(NativeCaptionButton::Close),
        _ => None,
    };
}

fn handle_caption_button_up(hwnd: HWND, wparam: WPARAM) -> bool {
    let pressed = caption_button_pressed();
    let Some(pressed) = pressed else {
        return false;
    };
    let Some(released) = caption_button_from_hit(wparam) else {
        return false;
    };
    if pressed != released {
        return false;
    }

    match released {
        NativeCaptionButton::Minimize => {
            unsafe {
                ShowWindow(hwnd, SW_MINIMIZE);
            }
            return true;
        }
        NativeCaptionButton::Maximize => {
            let maximize = unsafe { IsZoomed(hwnd) == 0 };
            let command = if maximize { SW_MAXIMIZE } else { SW_NORMAL };
            unsafe {
                ShowWindow(hwnd, command);
            }
            return true;
        }
        NativeCaptionButton::Close => {
            unsafe {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
            return true;
        }
    }
}

fn hit_test(hwnd: HWND, lparam: LPARAM) -> Option<i32> {
    let regions = REGIONS
        .get()
        .and_then(|slot| slot.lock().ok())
        .map(|locked| locked.clone())?;

    let mut point = POINT {
        x: (lparam as u32 & 0xFFFF) as i16 as i32,
        y: ((lparam as u32 >> 16) & 0xFFFF) as i16 as i32,
    };
    unsafe {
        ScreenToClient(hwnd, &mut point);
    }

    let x = point.x as f32;
    let y = point.y as f32;

    if let Some(rect) = regions.close.filter(|rect| rect.is_valid()) {
        if rect.contains(x, y) {
            return Some(HTCLOSE as i32);
        }
    }
    if let Some(rect) = regions.maximize.filter(|rect| rect.is_valid()) {
        if rect.contains(x, y) {
            return Some(HTMAXBUTTON as i32);
        }
    }
    if let Some(rect) = regions.minimize.filter(|rect| rect.is_valid()) {
        if rect.contains(x, y) {
            return Some(HTMINBUTTON as i32);
        }
    }

    if unsafe { IsZoomed(hwnd) == 0 } {
        if let Some(code) = edge_hit_test(hwnd, x, y) {
            return Some(code);
        }
    }

    if let Some(header) = regions.header_drag.filter(|rect| rect.is_valid()) {
        if header.contains(x, y)
            && !regions
                .client_blocks
                .iter()
                .any(|block| block.contains(x, y))
        {
            return Some(windows_sys::Win32::UI::WindowsAndMessaging::HTCAPTION as i32);
        }
    }

    return None;
}

fn query_window_dpi(hwnd: HWND) -> u32 {
    use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
    return unsafe { GetDpiForWindow(hwnd) };
}

fn edge_hit_test(hwnd: HWND, x: f32, y: f32) -> Option<i32> {
    let dpi = query_window_dpi(hwnd);
    let frame_x = frame_thickness_x(dpi) as f32;
    let frame_y = frame_thickness_y(dpi) as f32;

    let mut client_rect = windows_sys::Win32::Foundation::RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    unsafe {
        GetClientRect(hwnd, &mut client_rect);
    }
    let width = (client_rect.right - client_rect.left) as f32;
    let height = (client_rect.bottom - client_rect.top) as f32;

    if y >= 0.0 && y <= frame_y {
        if x <= frame_x {
            return Some(HTTOPLEFT as i32);
        }
        if x >= width - frame_x {
            return Some(HTTOPRIGHT as i32);
        }
        return Some(HTTOP as i32);
    }
    if y >= height - frame_y {
        if x <= frame_x {
            return Some(HTBOTTOMLEFT as i32);
        }
        if x >= width - frame_x {
            return Some(HTBOTTOMRIGHT as i32);
        }
        return Some(HTBOTTOM as i32);
    }
    if x >= 0.0 && x <= frame_x {
        return Some(HTLEFT as i32);
    }
    if x >= width - frame_x {
        return Some(HTRIGHT as i32);
    }

    return None;
}

fn frame_thickness_x(dpi: u32) -> i32 {
    let resize = unsafe { GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) };
    let padding = unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) };
    return resize + padding;
}

fn frame_thickness_y(dpi: u32) -> i32 {
    let resize = unsafe { GetSystemMetricsForDpi(SM_CYSIZEFRAME, dpi) };
    let padding = unsafe { GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi) };
    return resize + padding;
}
