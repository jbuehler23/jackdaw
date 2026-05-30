//! Borderless primary window chrome: shell styling, header window controls, resize edges

mod controls;
mod header;
mod icon;
mod repo_link;
mod resize;

pub use controls::window_controls;
pub use header::{WindowHeaderRoot, window_header};
pub use repo_link::JackdawIcon;
pub use resize::resize_edge_overlay;

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window, WindowMode};
use bevy::winit::WINIT_WINDOWS;
use jackdaw_feathers::icons::Icon;

use controls::{WindowControlsMaximize, WindowControlsPlugin};
use header::WindowHeaderPlugin;
use resize::{WindowResizeRoot, on_resize_edge_press};

const WINDOW_SHELL_CORNER_RADIUS_PX: f32 = 8.0;

#[derive(Component)]
pub struct WindowShellRoot;

pub struct WindowingPlugin;

impl Plugin for WindowingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins((WindowControlsPlugin, WindowHeaderPlugin, repo_link::RepoLinkPlugin));
        #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
        {
            app.add_observer(on_resize_edge_press)
                .add_systems(Update, sync_window_shell_state);
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
    windows: Query<(Entity, &Window), With<PrimaryWindow>>,
    mut resize_roots: Query<&mut Node, (With<WindowResizeRoot>, Without<WindowShellRoot>)>,
    mut shell_roots: Query<&mut Node, (With<WindowShellRoot>, Without<WindowResizeRoot>)>,
    maximize_buttons: Query<&Children, With<WindowControlsMaximize>>,
    mut texts: Query<&mut Text>,
) {
    let Ok((entity, window)) = windows.single() else {
        return;
    };

    let is_fullscreen = !matches!(window.mode, WindowMode::Windowed);
    let is_maximized = primary_window_is_maximized(entity);
    let is_floating_window = !is_fullscreen && !is_maximized;

    for mut node in resize_roots.iter_mut() {
        node.display = if is_floating_window {
            // reenable edge resize
            Display::Flex
        } else {
            Display::None
        };
    }

    let shell_border_radius = if is_floating_window {
        BorderRadius::all(Val::Px(WINDOW_SHELL_CORNER_RADIUS_PX))
    } else {
        BorderRadius::ZERO
    };
    for mut node in shell_roots.iter_mut() {
        node.border_radius = shell_border_radius;
    }

    #[cfg(target_os = "windows")]
    apply_windows_corner_preference(entity, is_floating_window);

    let icon = if is_maximized {
        Icon::Minimize2
    } else {
        Icon::Maximize2
    };
    let glyph = icon.unicode().to_string();
    for children in &maximize_buttons {
        for child in children.iter() {
            let Ok(mut text) = texts.get_mut(child) else {
                continue;
            };
            if text.0 != glyph {
                text.0 = glyph.clone();
            }
        }
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
