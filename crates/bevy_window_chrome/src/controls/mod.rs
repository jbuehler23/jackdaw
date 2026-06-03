//! Minimize / maximize / close caption button markers and the Windows native caption cluster.
//!
//! On Windows the caption buttons are drawn here (Segoe icon font) and driven by Win32 non-client
//! hit testing. On other platforms the host application supplies its own button widgets carrying
//! the [`WindowControlsMinimize`] / [`WindowControlsMaximize`] / [`WindowControlsClose`] markers
//! and wires their interaction.

#[cfg(target_os = "windows")]
pub mod windows_caption;

#[cfg(target_os = "windows")]
pub use windows_caption::{WindowsCaptionFont, window_controls_native};

use bevy::prelude::*;

#[derive(Component)]
pub struct WindowControlsMinimize;

#[derive(Component)]
pub struct WindowControlsMaximize;

#[derive(Component)]
pub struct WindowControlsClose;

pub(crate) fn build(_app: &mut App) {
    #[cfg(target_os = "windows")]
    install_windows_caption_font_in_app(_app);
}

/// Load the system caption icon font before any schedule runs.
#[cfg(target_os = "windows")]
fn install_windows_caption_font_in_app(app: &mut App) {
    let mut fonts = app.world_mut().resource_mut::<Assets<Font>>();
    let handle = windows_caption::load_windows_caption_font(&mut fonts).expect(
        "Segoe Fluent Icons or Segoe MDL2 Assets should be installed under C:\\Windows\\Fonts",
    );
    app.insert_resource(WindowsCaptionFont(handle));
}
