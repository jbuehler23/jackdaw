//! Borderless primary window chrome: shell styling, header window controls, resize edges

mod chrome;
mod controls;
mod header;
#[cfg(target_os = "macos")]
mod macos_titlebar;
mod icon;
mod native_hit_test;
mod repo_link;
mod resize;
mod shell;

pub use chrome::{WindowChromeStyle, primary_window_attributes};
#[cfg(not(target_os = "windows"))]
pub use controls::window_controls_interactive;
#[cfg(target_os = "windows")]
pub use controls::{WindowsCaptionFont, window_controls_native};
pub use header::{
    WindowHeaderRoot, WindowShellHeaderSlot, native_hit_test_client, spawn_window_header,
};
pub use native_hit_test::NativeHitTestClient;
#[cfg(target_os = "windows")]
pub use native_hit_test::mark_menu_bar_native_clients;
pub use repo_link::JackdawIcon;
pub use resize::{resize_edge_overlay, spawn_resize_edge_overlay_if_needed};
pub use shell::{WindowShellContent, WindowShellSlots, spawn_window_shell};

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window, WindowMode};
#[cfg(target_os = "macos")]
use bevy::window::WindowResized;
use bevy::winit::WINIT_WINDOWS;

use chrome::WindowChromeStyle as ChromeStyle;
use controls::WindowControlsPlugin;
use header::WindowHeaderPlugin;
use resize::{WindowResizeRoot, on_resize_edge_press};

const WINDOW_SHELL_CORNER_RADIUS_PX: f32 = 8.0;

#[derive(Component)]
pub struct WindowShellRoot;

pub struct WindowingPlugin;

impl Plugin for WindowingPlugin {
    fn build(&self, app: &mut App) {
        let chrome = WindowChromeStyle::current();
        app.insert_resource(chrome);
        app.add_plugins((
            WindowControlsPlugin,
            WindowHeaderPlugin,
            repo_link::RepoLinkPlugin,
            native_hit_test::NativeHitTestPlugin,
        ));
        #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
        {
            if chrome.uses_resize_edge_overlay() {
                app.add_observer(on_resize_edge_press);
            }
            app.add_systems(Update, sync_window_shell_state);
        }
        #[cfg(not(target_arch = "wasm32"))]
        icon::install(app);
    }
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
pub(crate) fn primary_window_is_maximized(window_entity: Entity) -> bool {
    WINIT_WINDOWS.with(|windows_cell| {
        let winit_windows = windows_cell.borrow();
        winit_windows
            .get_window(window_entity)
            .is_some_and(|backend| backend.is_maximized())
    })
}

#[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
fn sync_window_shell_state(
    _main_thread: bevy::ecs::system::NonSendMarker,
    chrome: Res<WindowChromeStyle>,
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    mut resize_roots: Query<&mut Node, (With<WindowResizeRoot>, Without<WindowShellRoot>)>,
    mut shell_roots: Query<&mut Node, (With<WindowShellRoot>, Without<WindowResizeRoot>)>,
    #[cfg(target_os = "macos")] mut resize_events: MessageReader<WindowResized>,
    #[cfg(target_os = "macos")] mut previous_maximized: Local<Option<bool>>,
) {
    let Ok((entity, window)) = windows.single() else {
        return;
    };

    let is_fullscreen = !matches!(window.mode, WindowMode::Windowed);
    let is_maximized = primary_window_is_maximized(entity);
    let is_floating_window = !is_fullscreen && !is_maximized;

    #[cfg(target_os = "macos")]
    if *chrome == ChromeStyle::MacNativeTitlebar && !is_fullscreen {
        let window_resized = resize_events.read().any(|event| event.window == entity);
        let maximized_changed = previous_maximized.get() != Some(is_maximized);
        if window_resized || maximized_changed {
            macos_titlebar::reposition_traffic_lights(entity);
        }
        if maximized_changed {
            *previous_maximized = Some(is_maximized);
        }
    }

    if chrome.uses_resize_edge_overlay() {
        for mut node in resize_roots.iter_mut() {
            node.display = if is_floating_window {
                Display::Flex
            } else {
                Display::None
            };
        }
    }

    if chrome.uses_shell_corner_radius() {
        let shell_border_radius = if is_floating_window {
            BorderRadius::all(Val::Px(WINDOW_SHELL_CORNER_RADIUS_PX))
        } else {
            BorderRadius::ZERO
        };
        for mut node in shell_roots.iter_mut() {
            node.border_radius = shell_border_radius;
        }
    }

    #[cfg(target_os = "windows")]
    if *chrome == ChromeStyle::CustomClient {
        apply_windows_corner_preference(entity, is_floating_window);
    }
}

#[cfg(all(
    not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")),
    target_os = "windows"
))]
fn apply_windows_corner_preference(window_entity: Entity, round: bool) {
    use winit::platform::windows::{CornerPreference, WindowExtWindows};

    WINIT_WINDOWS.with(|windows_cell| {
        let winit_windows = windows_cell.borrow();
        let Some(backend) = winit_windows.get_window(window_entity) else {
            return;
        };
        let preference = if round {
            CornerPreference::Round
        } else {
            CornerPreference::DoNotRound
        };
        backend.set_corner_preference(preference);
    });
}
