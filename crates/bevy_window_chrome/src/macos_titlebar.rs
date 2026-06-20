//! Native traffic light positioning on macOS.

use std::cell::RefCell;
use std::sync::OnceLock;

use bevy::ecs::system::NonSendMarker;
use bevy::prelude::{Commands, Component, Entity, MessageReader, Node, Query, Res, Val, With};
use bevy::window::{PrimaryWindow, WindowCreated};
use bevy::winit::WINIT_WINDOWS;

use crate::WindowChromeTheme;
use crate::title_bar::WindowTitleBarContentSlot;
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

static WINDOW_CHROME_THEME: OnceLock<WindowChromeTheme> = OnceLock::new();

/// Sets the window chrome theme for `AppKit` callbacks that cannot access `Bevy` resources.
pub(crate) fn set_theme(theme: WindowChromeTheme) {
    let _ = WINDOW_CHROME_THEME.set(theme);
}

fn cached_theme() -> WindowChromeTheme {
    WINDOW_CHROME_THEME.get().cloned().unwrap_or_default()
}

thread_local! {
    // Assumes a single, long-lived primary window; a recreated window would leave
    // this observer pointing at the old one.
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
            if is_native_fullscreen(entity) {
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
        unsafe { msg_send_id![super(this), init] }
    }
}

/// Cached native fullscreen state for the primary window.
#[derive(Component, Copy, Clone, Default)]
pub(crate) struct MacosNativeFullscreen(pub bool);

/// Registers the resize observer and initial traffic-light position when the primary window is created.
pub(crate) fn on_macos_window_created(
    _main_thread: NonSendMarker,
    mut created: MessageReader<WindowCreated>,
    mut commands: Commands,
) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    for event in created.read() {
        let window_entity = event.window;
        ensure_traffic_light_resize_observer(window_entity, mtm);
        let is_native_fullscreen = is_native_fullscreen(window_entity);
        commands
            .entity(window_entity)
            .insert(MacosNativeFullscreen(is_native_fullscreen));
        if !is_native_fullscreen {
            reposition_traffic_lights(window_entity);
        }
    }
}

/// Syncs title bar content inset with native fullscreen.
pub(crate) fn sync_macos_window_shell_state(
    _main_thread: NonSendMarker,
    theme: Res<WindowChromeTheme>,
    mut windows: Query<(Entity, &mut MacosNativeFullscreen), With<PrimaryWindow>>,
    mut title_bar_content_slots: Query<&mut Node, With<WindowTitleBarContentSlot>>,
) {
    let Ok((window_entity, mut native_fullscreen)) = windows.single_mut() else {
        return;
    };

    let current = is_native_fullscreen(window_entity);
    let content_inset = if current {
        0.0
    } else {
        theme.macos_traffic_light_inset
    };
    for mut node in title_bar_content_slots.iter_mut() {
        node.padding.left = Val::Px(content_inset);
    }
    if native_fullscreen.0 != current {
        native_fullscreen.0 = current;
        if !current {
            reposition_traffic_lights(window_entity);
        }
    }
}

/// Whether the window is in macOS native fullscreen (green-button enter full screen).
pub fn is_native_fullscreen(window_entity: Entity) -> bool {
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let Some(ns_window) = ns_window_for_entity(window_entity, mtm) else {
        return false;
    };
    ns_window
        .styleMask()
        .contains(NSWindowStyleMask::FullScreen)
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

    let theme = cached_theme();
    let window_frame = ns_window.frame();
    let content_layout_rect = unsafe { ns_window.contentLayoutRect() };
    let titlebar_height = window_frame.size.height - content_layout_rect.size.height;

    let mut close_frame = close_button.frame();
    let minimize_frame = minimize_button.frame();
    let mut zoom_frame = zoom_button.frame();

    let button_spacing = minimize_frame.origin.x - close_frame.origin.x;
    let traffic_light_x = theme.macos_traffic_light_position_x as f64;
    let button_height = close_frame.size.height;
    let title_bar_height = theme.title_bar_height as f64;
    let layout_height = titlebar_height.min(title_bar_height);
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
    Some((close_button, minimize_button, zoom_button))
}

fn ns_window_for_entity(
    window_entity: Entity,
    _mtm: MainThreadMarker,
) -> Option<objc2::rc::Retained<NSWindow>> {
    WINIT_WINDOWS.with(|windows_cell| {
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
    })
}
