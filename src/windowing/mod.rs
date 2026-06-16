//! Jackdaw window chrome: wires the reusable [`bevy_window_chrome`] crate to jackdaw's design
//! tokens, branding, and editor entity bookkeeping.

mod icon;
mod repo_link;

pub use bevy_window_chrome::{
    WindowShellContent, WindowShellSlots, WindowTitleBarContentSlot, WindowTitleBarRoot,
    primary_window_attributes,
};
pub use repo_link::{JackdawIcon, title_bar_repo_link};

use bevy::prelude::*;
use bevy::window::{ExitCondition, WindowPlugin};
use bevy_window_chrome::{CaptionTheme, WindowChromePlugin, WindowChromeTheme};
use icon::WindowIconPlugin;
use jackdaw_feathers::tokens;
use time::{Month, OffsetDateTime};

const WINDOW_ICON_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/logo/jackdaw_icon_small.png"
));

const WINDOW_ICON_PRIDE_PNG: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/assets/logo/jackdaw_icon_pride_small.png"
));

fn window_icon_png_bytes() -> &'static [u8] {
    if is_pride_month() {
        WINDOW_ICON_PRIDE_PNG
    } else {
        WINDOW_ICON_PNG
    }
}

pub(crate) fn is_pride_month() -> bool {
    let Ok(date_time) = OffsetDateTime::now_local() else {
        return false;
    };
    date_time.month() == Month::June
}

/// Window chrome theme built from jackdaw's design tokens.
fn window_chrome_theme() -> WindowChromeTheme {
    WindowChromeTheme {
        title_bar_height: tokens::WINDOW_TITLE_BAR_HEIGHT,
        window_background: tokens::WINDOW_BG,
        caption: CaptionTheme {
            icon_color: tokens::TEXT_PRIMARY,
            button_hover_background: tokens::TOOLBAR_BUTTON_BG,
            ..CaptionTheme::default()
        },
        ..Default::default()
    }
}

/// [`WindowPlugin`] for editor binaries.
///
/// Configures jackdaw's custom chrome window and disables Bevy's default
/// close-to-exit wiring so [`crate::scenes::intercept_window_close`] can
/// show the unsaved-changes dialog before quitting.
pub fn editor_window_plugin() -> WindowPlugin {
    WindowPlugin {
        exit_condition: ExitCondition::DontExit,
        close_when_requested: false,
        primary_window: Some(primary_window_attributes()),
        ..default()
    }
}

pub struct WindowingPlugin;

impl Plugin for WindowingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(WindowChromePlugin::new(window_chrome_theme()));
        app.add_plugins(WindowIconPlugin::new(window_icon_png_bytes()));
        app.add_plugins(repo_link::RepoLinkPlugin);
    }
}
