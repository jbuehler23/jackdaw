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

/// `JACKDAW_WINDOW_SIZE=<width>x<height>` overrides the primary window's
/// initial resolution (physical pixels), e.g. `1920x1080` to pin the
/// window for scripted screenshot runs. Read once at startup, like
/// [`crate::project::ENV_OPEN_PROJECT`] and [`crate::screenshot::ENV_SHOT`].
/// Unset in interactive launches, which keeps the platform/WM default.
pub const ENV_WINDOW_SIZE: &str = "JACKDAW_WINDOW_SIZE";

fn window_size_override() -> Option<bevy::window::WindowResolution> {
    parse_window_size(&std::env::var(ENV_WINDOW_SIZE).ok()?)
}

/// Parse a `<width>x<height>` [`ENV_WINDOW_SIZE`] value. Malformed values
/// (missing `x`, non-numeric halves) parse to `None` rather than failing the
/// launch.
fn parse_window_size(raw: &str) -> Option<bevy::window::WindowResolution> {
    let (w, h) = raw.split_once('x')?;
    let width: u32 = w.trim().parse().ok()?;
    let height: u32 = h.trim().parse().ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some(bevy::window::WindowResolution::new(width, height))
}

/// [`WindowPlugin`] for editor binaries.
///
/// Configures jackdaw's custom chrome window and disables Bevy's default
/// close-to-exit wiring so [`crate::scenes::intercept_window_close`] can
/// show the unsaved-changes dialog before quitting.
pub fn editor_window_plugin() -> WindowPlugin {
    let mut window = primary_window_attributes();
    if let Some(resolution) = window_size_override() {
        window.resolution = resolution;
    }
    WindowPlugin {
        exit_condition: ExitCondition::DontExit,
        close_when_requested: false,
        primary_window: Some(window),
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

#[cfg(test)]
mod window_size_tests {
    use super::*;

    #[test]
    fn a_well_formed_size_parses() {
        let resolution = parse_window_size("1920x1080").unwrap();
        assert_eq!(resolution.physical_width(), 1920);
        assert_eq!(resolution.physical_height(), 1080);
    }

    #[test]
    fn whitespace_around_either_half_is_tolerated() {
        let resolution = parse_window_size(" 1920 x 1080 ").unwrap();
        assert_eq!(resolution.physical_width(), 1920);
        assert_eq!(resolution.physical_height(), 1080);
    }

    #[test]
    fn missing_separator_parses_to_nothing() {
        assert!(parse_window_size("1920").is_none());
    }

    #[test]
    fn non_numeric_halves_parse_to_nothing() {
        assert!(parse_window_size("wideXtall").is_none());
        assert!(parse_window_size("1920xtall").is_none());
    }

    #[test]
    fn a_zero_half_parses_to_nothing() {
        assert!(parse_window_size("0x1080").is_none());
        assert!(parse_window_size("1920x0").is_none());
        assert!(parse_window_size("0x0").is_none());
    }
}
