//! Native traffic light positioning on macOS (same approach as Zed's `gpui_macos`).

use bevy::ecs::entity::Entity;
use bevy::winit::WINIT_WINDOWS;
use objc2::MainThreadMarker;
use objc2_app_kit::{NSView, NSWindow, NSWindowButton, NSWindowStyleMask};
use objc2_foundation::NSPoint;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::raw_window_handle::HasWindowHandle as _;

use jackdaw_feathers::tokens;

/// Re-position standard window buttons after zoom or resize.
///
/// Zed keeps `NSFullSizeContentView` enabled and calls `move_traffic_light` from
/// `windowDidResize` instead of toggling the style mask.
pub fn reposition_traffic_lights(window_entity: Entity) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(ns_window) = ns_window_for_entity(window_entity, mtm) else {
        return;
    };

    if ns_window
        .styleMask()
        .contains(NSWindowStyleMask::FullScreen)
    {
        return;
    }

    let close_button = ns_window.standardWindowButton(NSWindowButton::NSWindowCloseButton);
    let minimize_button = ns_window.standardWindowButton(NSWindowButton::NSWindowMiniaturizeButton);
    let zoom_button = ns_window.standardWindowButton(NSWindowButton::NSWindowZoomButton);
    let (Some(close_button), Some(minimize_button), Some(zoom_button)) =
        (close_button, minimize_button, zoom_button)
    else {
        return;
    };

    for button in [&close_button, &minimize_button, &zoom_button] {
        button.setHidden(false);
    }

    let window_frame = ns_window.frame();
    let content_layout_rect = ns_window.contentLayoutRect();
    let titlebar_height =
        window_frame.size.height - content_layout_rect.size.height;

    let mut close_frame = close_button.frame();
    let minimize_frame = minimize_button.frame();
    let mut zoom_frame = zoom_button.frame();

    let button_spacing = minimize_frame.origin.x - close_frame.origin.x;
    let traffic_light_y = tokens::MACOS_TRAFFIC_LIGHT_POSITION_Y as f64;
    let traffic_light_x = tokens::MACOS_TRAFFIC_LIGHT_POSITION_X as f64;
    let mut origin_y =
        titlebar_height - traffic_light_y - close_frame.size.height;
    let mut origin_x = traffic_light_x;

    close_frame.origin = NSPoint::new(origin_x, origin_y);
    close_button.setFrame(close_frame);
    origin_x += button_spacing;

    let mut minimize_moved = minimize_frame;
    minimize_moved.origin = NSPoint::new(origin_x, origin_y);
    minimize_button.setFrame(minimize_moved);
    origin_x += button_spacing;

    zoom_frame.origin = NSPoint::new(origin_x, origin_y);
    zoom_button.setFrame(zoom_frame);
}

fn ns_window_for_entity(
    window_entity: Entity,
    _mtm: MainThreadMarker,
) -> Option<objc2::rc::Retained<NSWindow>> {
    return WINIT_WINDOWS.with(|windows_cell| {
        let winit_windows = windows_cell.borrow();
        let winit_window = winit_windows.get_window(window_entity)?;
        let handle = winit_window.window_handle().ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        // SAFETY: `ns_view` comes from the live AppKit window owned by winit.
        let view = unsafe { objc2::rc::Retained::retain(appkit.ns_view.as_ptr().cast())? };
        view.window()
    });
}
