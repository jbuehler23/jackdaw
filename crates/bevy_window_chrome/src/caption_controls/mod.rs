//! Minimize / maximize / close caption buttons for borderless windows.

mod caption;
mod caption_actions;

use bevy::prelude::*;

pub use caption::{CaptionButton, CaptionFont, window_controls};
pub(crate) use caption::{load_caption_font, sync_caption_chrome};

pub(crate) fn build(app: &mut App) {
    caption_actions::register_pointer_handlers(app);
    app.add_systems(Last, sync_caption_chrome);
}
