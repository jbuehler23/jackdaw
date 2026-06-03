//! Native traffic light positioning on macOS (transparent integrated title bar).

use std::cell::RefCell;
use std::sync::OnceLock;

use bevy::ecs::entity::Entity;
use bevy::winit::WINIT_WINDOWS;
use objc2::rc::Retained;
use objc2::runtime::NSObject;
use objc2::{ClassType, DeclaredClass, declare_class, msg_send_id, mutability, sel};
use objc2_app_kit::{
    NSButton, NSView, NSWindow, NSWindowButton, NSWindowDidResizeNotification, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSObjectProtocol, NSPoint,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Title-bar metrics needed to align native traffic lights with the custom header.
#[derive(Clone, Copy)]
pub(crate) struct MacChromeMetrics {
    pub header_height: f64,
    pub traffic_light_position_x: f64,
}

static LAYOUT_METRICS: OnceLock<MacChromeMetrics> = OnceLock::new();

/// Stores the title-bar layout metrics used when repositioning traffic lights.
pub(crate) fn set_layout_metrics(metrics: MacChromeMetrics) {
    let _ = LAYOUT_METRICS.set(metrics);
}

fn layout_metrics() -> MacChromeMetrics {
    return LAYOUT_METRICS.get().copied().unwrap_or(MacChromeMetrics {
        header_height: 36.0,
        traffic_light_position_x: 12.0,
    });
}

thread_local! {
    static TRAFFIC_LIGHT_RESIZE_OBSERVER: RefCell<Option<Retained<TrafficLightResizeObserver>>> =
        const { RefCell::new(None) };
}

struct TrafficLightResizeObserverIvars {
    window_entity_bits: u64,
}

declare_class!(
    struct TrafficLightResizeObserver;

    unsafe impl ClassType for TrafficLightResizeObserver {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "BevyWindowChromeTrafficLightResizeObserver";
    }

    impl DeclaredClass for TrafficLightResizeObserver {
        type Ivars = TrafficLightResizeObserverIvars;
    }

    unsafe impl TrafficLightResizeObserver {
        #[method(windowDidResize:)]
        fn window_did_resize(&self, _notification: &NSNotification) {
            let entity = Entity::from_bits(self.ivars().window_entity_bits);
            if window_fills_work_area(entity) {
                return;
            }
            reposition_traffic_lights(entity);
        }
    }

    unsafe impl NSObjectProtocol for TrafficLightResizeObserver {}
);

impl TrafficLightResizeObserver {
    fn new(window_entity: Entity) -> Retained<Self> {
        let ivars = TrafficLightResizeObserverIvars {
            window_entity_bits: window_entity.to_bits(),
        };
        let this = Self::alloc().set_ivars(ivars);
        return unsafe { msg_send_id![super(this), init] };
    }
}

const FRAME_MATCH_TOLERANCE_PX: f64 = 2.0;

/// Whether the window frame fills the display work area (green-button zoom).
///
/// Compares width to `visibleFrame` and height to `visibleFrame` or full screen frame, since
/// transparent title-bar windows often report a taller frame than `visibleFrame`.
pub fn window_fills_work_area(window_entity: Entity) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let Some(ns_window) = ns_window_for_entity(window_entity, mtm) else {
        return false;
    };
    let Some(screen) = ns_window.screen() else {
        return false;
    };
    let window_frame = ns_window.frame();
    let visible_frame = screen.visibleFrame();
    let screen_frame = screen.frame();
    if !sizes_match(
        window_frame.size.width,
        visible_frame.size.width,
        FRAME_MATCH_TOLERANCE_PX,
    ) {
        return false;
    }
    return sizes_match(
        window_frame.size.height,
        visible_frame.size.height,
        FRAME_MATCH_TOLERANCE_PX,
    ) || sizes_match(
        window_frame.size.height,
        screen_frame.size.height,
        FRAME_MATCH_TOLERANCE_PX,
    );
}

/// Hide or show standard window buttons. When expanded, macOS reveals them at the top edge on hover.
pub fn set_traffic_lights_hidden(window_entity: Entity, hidden: bool) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(buttons) = traffic_light_buttons(window_entity, mtm) else {
        return;
    };
    let (close_button, minimize_button, zoom_button) = buttons;
    for button in [&close_button, &minimize_button, &zoom_button] {
        button.setHidden(hidden);
    }
}

/// Registers a one-shot `NSWindowDidResizeNotification` observer for the primary window.
pub fn ensure_traffic_light_resize_observer(window_entity: Entity, mtm: MainThreadMarker) {
    TRAFFIC_LIGHT_RESIZE_OBSERVER.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        let Some(ns_window) = ns_window_for_entity(window_entity, mtm) else {
            return;
        };
        let observer = TrafficLightResizeObserver::new(window_entity);
        let center = unsafe { NSNotificationCenter::defaultCenter() };
        unsafe {
            center.addObserver_selector_name_object(
                &observer,
                sel!(windowDidResize:),
                Some(NSWindowDidResizeNotification),
                Some(&ns_window),
            );
        };
        *slot.borrow_mut() = Some(observer);
    });
}

pub fn reposition_traffic_lights(window_entity: Entity) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(ns_window) = ns_window_for_entity(window_entity, mtm) else {
        return;
    };
    let Some((close_button, minimize_button, zoom_button)) =
        traffic_light_buttons(window_entity, mtm)
    else {
        return;
    };

    let metrics = layout_metrics();
    let window_frame = ns_window.frame();
    let content_layout_rect = unsafe { ns_window.contentLayoutRect() };
    let titlebar_height = window_frame.size.height - content_layout_rect.size.height;

    let mut close_frame = close_button.frame();
    let minimize_frame = minimize_button.frame();
    let mut zoom_frame = zoom_button.frame();

    let button_spacing = minimize_frame.origin.x - close_frame.origin.x;
    let traffic_light_x = metrics.traffic_light_position_x;
    let button_height = close_frame.size.height;
    let header_height = metrics.header_height;
    let layout_height = titlebar_height.min(header_height);
    let y_offset_from_titlebar_top = (layout_height - button_height) / 2.0;
    let origin_y = titlebar_height - y_offset_from_titlebar_top - button_height;
    let mut origin_x = traffic_light_x;

    close_frame.origin = NSPoint::new(origin_x, origin_y);
    unsafe {
        close_button.setFrame(close_frame);
    }
    origin_x += button_spacing;

    let mut minimize_moved = minimize_frame;
    minimize_moved.origin = NSPoint::new(origin_x, origin_y);
    unsafe {
        minimize_button.setFrame(minimize_moved);
    }
    origin_x += button_spacing;

    zoom_frame.origin = NSPoint::new(origin_x, origin_y);
    unsafe {
        zoom_button.setFrame(zoom_frame);
    }
}

fn traffic_light_buttons(
    window_entity: Entity,
    mtm: MainThreadMarker,
) -> Option<(
    objc2::rc::Retained<NSButton>,
    objc2::rc::Retained<NSButton>,
    objc2::rc::Retained<NSButton>,
)> {
    let ns_window = ns_window_for_entity(window_entity, mtm)?;
    if ns_window
        .styleMask()
        .contains(NSWindowStyleMask::FullScreen)
    {
        return None;
    }
    let close_button = ns_window.standardWindowButton(NSWindowButton::NSWindowCloseButton);
    let minimize_button = ns_window.standardWindowButton(NSWindowButton::NSWindowMiniaturizeButton);
    let zoom_button = ns_window.standardWindowButton(NSWindowButton::NSWindowZoomButton);
    let (Some(close_button), Some(minimize_button), Some(zoom_button)) =
        (close_button, minimize_button, zoom_button)
    else {
        return None;
    };
    return Some((close_button, minimize_button, zoom_button));
}

fn sizes_match(a: f64, b: f64, tolerance: f64) -> bool {
    return (a - b).abs() <= tolerance;
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
        let view: objc2::rc::Retained<NSView> =
            unsafe { objc2::rc::Retained::retain(appkit.ns_view.as_ptr().cast())? };
        view.window()
    });
}
