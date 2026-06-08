//! Minimize / maximize / close caption button markers and platform caption clusters.

mod caption;
mod caption_actions;

use bevy::prelude::*;

pub use caption::CaptionFont;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
pub use caption::window_controls;

#[derive(Component)]
pub struct WindowControlsMinimize;

#[derive(Component)]
pub struct WindowControlsMaximize;

#[derive(Component)]
pub struct WindowControlsClose;

pub(crate) fn build(app: &mut App) {
    caption_actions::register_pointer_handlers(app);

    #[cfg(target_os = "windows")]
    {
        install_windows_caption_font_in_app(app);
    }

    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    {
        app.add_systems(Last, caption::sync_caption_chrome);
    }
}

/// Load the system caption icon font before any schedule runs.
fn install_windows_caption_font_in_app(app: &mut App) {
    let mut fonts = app.world_mut().resource_mut::<Assets<Font>>();
    let handle = caption::load_windows_caption_font(&mut fonts).expect(
        "Segoe Fluent Icons or Segoe MDL2 Assets should be installed under C:\\Windows\\Fonts",
    );
    app.insert_resource(CaptionFont(handle));
}
