//! Applies an application-provided window icon to the winit backend window.

use bevy::prelude::*;
#[cfg(not(target_arch = "wasm32"))]
use bevy::{ecs::system::NonSendMarker, window::WindowCreated, winit::WINIT_WINDOWS};
#[cfg(not(target_arch = "wasm32"))]
use winit::window::Icon;

/// Sets the primary window's icon from PNG bytes supplied by the host application.
///
/// Note that winit only honors a window icon on Windows and X11; the plugin is a no-op elsewhere.
pub struct WindowIconPlugin {
    bytes: &'static [u8],
}

impl WindowIconPlugin {
    /// Creates the plugin from the raw PNG bytes of the desired window icon.
    pub fn new(png_bytes: &'static [u8]) -> Self {
        Self { bytes: png_bytes }
    }
}

impl Plugin for WindowIconPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            app.insert_resource(WindowIconResource(load_icon_png(self.bytes)));
            app.add_systems(PostUpdate, apply_window_icon_on_window_created);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Resource)]
struct WindowIconResource(Option<Icon>);

#[cfg(not(target_arch = "wasm32"))]
fn load_icon_png(png_bytes: &[u8]) -> Option<Icon> {
    let image = match image::load_from_memory(png_bytes) {
        Ok(image) => image.into_rgba8(),
        Err(error) => {
            bevy::log::warn_once!("jackdaw: failed to decode window icon PNG: {:?}", error);
            return None;
        }
    };
    let width = image.width();
    let height = image.height();
    let rgba = image.into_raw();
    let icon = match Icon::from_rgba(rgba, width, height) {
        Ok(icon) => icon,
        Err(error) => {
            bevy::log::warn_once!(
                "jackdaw: failed to create window icon from PNG: {:?}",
                error
            );
            return None;
        }
    };
    Some(icon)
}

#[cfg(not(target_arch = "wasm32"))]
fn apply_window_icon_on_window_created(
    _main_thread: NonSendMarker,
    icon_state: Res<WindowIconResource>,
    mut created: MessageReader<WindowCreated>,
) {
    let Some(icon) = icon_state.0.as_ref() else {
        return;
    };
    for event in created.read() {
        WINIT_WINDOWS.with(|windows_cell| {
            let winit_windows = windows_cell.borrow();
            let Some(backend_window) = winit_windows.get_window(event.window) else {
                bevy::log::warn_once!(
                    "jackdaw: winit backend window missing when applying icon ({:?}); ignoring",
                    event.window,
                );
                return;
            };
            // This only works on Windows and x11.
            backend_window.set_window_icon(Some(icon.clone()));
        });
    }
}
