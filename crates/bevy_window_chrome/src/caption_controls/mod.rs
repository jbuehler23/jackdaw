//! Minimize / maximize / close caption button markers and platform caption clusters.

mod caption;
mod caption_actions;

use bevy::prelude::*;

pub use caption::CaptionButton;
pub use caption::CaptionFont;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
pub(crate) use caption::load_caption_font;
#[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
pub use caption::window_controls;

pub(crate) fn build(app: &mut App) {
    caption_actions::register_pointer_handlers(app);

    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    {
        app.add_systems(Last, caption::sync_caption_chrome);
    }
}
