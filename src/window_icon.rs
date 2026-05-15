use bevy::{ecs::system::NonSendMarker, prelude::*, window::WindowCreated, winit::WINIT_WINDOWS};
use winit::window::Icon;

const WINDOW_ICON_PNG: &[u8] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/bevy_icon.png"));

#[derive(Resource)]
struct WindowIconResource(Option<Icon>);

/// Adds a bevy icon to the winit window.
/// Note that this only works on Windows and x11 due to limitations of winit.
pub(crate) fn install(app: &mut App) {
    app.insert_resource(WindowIconResource(load_icon_png()));
    app.add_systems(PostUpdate, apply_window_icon_on_window_created);
}

fn load_icon_png() -> Option<Icon> {
    let image = match image::load_from_memory(WINDOW_ICON_PNG) {
        Ok(image) => image.into_rgba8(),
        Err(error) => {
            bevy::log::warn_once!(
                "jackdaw: failed to decode embedded window icon PNG: {:?}",
                error
            );
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
                "jackdaw: failed to create window icon from embedded PNG: {:?}",
                error
            );
            return None;
        }
    };
    Some(icon)
}

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
                    "jackdaw: winit backend window missing when applying decoration icon ({:?}); ignoring",
                    event.window,
                );
                return;
            };
            // This only works on Windows and x11.
            backend_window.set_window_icon(Some(icon.clone()));
        });
    }
}
