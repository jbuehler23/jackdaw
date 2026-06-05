//! Minimize / maximize / close caption button markers and platform caption clusters.

mod caption;
mod caption_actions;

pub use caption::CaptionFont;

use bevy::prelude::*;

use crate::WindowChromeStyle;

#[derive(Component)]
pub struct WindowControlsMinimize;

#[derive(Component)]
pub struct WindowControlsMaximize;

#[derive(Component)]
pub struct WindowControlsClose;

/// Caption minimize / maximize / close cluster for the current platform.
pub fn window_caption_controls(
    theme: &crate::WindowChromeTheme,
    caption_font: Handle<Font>,
) -> impl Bundle {
    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    {
        return caption::window_controls(theme, caption_font);
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
    {
        let _ = (theme, caption_font);
        return caption_controls_placeholder();
    }
}

/// macOS and other platforms without client-side caption buttons.
#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "freebsd")))]
fn caption_controls_placeholder() -> impl Bundle {
    return (crate::WindowChromeEntity, Pickable::IGNORE);
}

pub(crate) fn build(app: &mut App, style: WindowChromeStyle) {
    #[cfg(target_os = "windows")]
    {
        install_windows_caption_font_in_app(app);
    }

    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    if style.uses_app_caption_button_handlers() {
        caption::register(app);
    }
}

/// Load the system caption icon font before any schedule runs.
#[cfg(target_os = "windows")]
fn install_windows_caption_font_in_app(app: &mut App) {
    let mut fonts = app.world_mut().resource_mut::<Assets<Font>>();
    let handle = caption::load_windows_caption_font(&mut fonts).expect(
        "Segoe Fluent Icons or Segoe MDL2 Assets should be installed under C:\\Windows\\Fonts",
    );
    app.insert_resource(CaptionFont(handle));
}
