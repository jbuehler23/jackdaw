//! Minimize / maximize / close caption buttons.

#[cfg(target_os = "windows")]
pub mod windows_caption;

#[cfg(target_os = "windows")]
pub use windows_caption::{WindowsCaptionFont, window_controls_native};

#[cfg(not(target_os = "windows"))]
mod interactive;

#[cfg(not(target_os = "windows"))]
pub use interactive::window_controls_interactive;

use bevy::prelude::*;

#[derive(Component)]
pub(crate) struct WindowControlsMinimize;

#[derive(Component)]
pub(crate) struct WindowControlsMaximize;

#[derive(Component)]
pub(crate) struct WindowControlsClose;

pub struct WindowControlsPlugin;

impl Plugin for WindowControlsPlugin {
    fn build(&self, app: &mut App) {
        #[cfg(not(target_os = "windows"))]
        interactive::register(app);
        #[cfg(target_os = "windows")]
        {
            install_windows_caption_font_in_app(app);
        }
    }
}

/// Load the system caption icon font before any schedule runs (same pattern as [`IconFontPlugin`]).
#[cfg(target_os = "windows")]
pub fn install_windows_caption_font_in_app(app: &mut App) {
    let mut fonts = app.world_mut().resource_mut::<Assets<Font>>();
    let handle = windows_caption::load_windows_caption_font(&mut fonts).expect(
        "Segoe Fluent Icons or Segoe MDL2 Assets should be installed under C:\\Windows\\Fonts",
    );
    app.insert_resource(WindowsCaptionFont(handle));
}
