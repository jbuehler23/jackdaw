//! Jackdaw window chrome: wires the reusable [`bevy_window_chrome`] crate to jackdaw's design
//! tokens, branding, and editor entity bookkeeping.

mod icon;
mod repo_link;

pub use bevy_window_chrome::{
    WindowShellContent, WindowShellSlots, WindowTitleBarContentSlot, WindowTitleBarRoot,
};
pub use repo_link::{JackdawIcon, title_bar_repo_link};

use bevy::prelude::*;
use bevy_window_chrome::{
    CaptionFont, CaptionTheme, WindowChromeEntity, WindowChromePlugin, WindowChromeTheme,
};
use icon::WindowIconPlugin;
use jackdaw_feathers::icons::IconFont;
use jackdaw_feathers::tokens;
use time::{Month, OffsetDateTime};

use crate::EditorEntity;

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
    return date_time.month() == Month::June;
}

/// Window chrome theme built from jackdaw's design tokens.
fn jackdaw_window_chrome_theme() -> WindowChromeTheme {
    return WindowChromeTheme {
        title_bar_height: tokens::WINDOW_TITLE_BAR_HEIGHT,
        window_background: tokens::WINDOW_BG,
        caption: CaptionTheme {
            icon_color: tokens::TEXT_PRIMARY,
            button_hover_background: tokens::TOOLBAR_BUTTON_BG,
            ..CaptionTheme::default()
        },
        ..Default::default()
    };
}

pub struct WindowingPlugin;

impl Plugin for WindowingPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(WindowChromePlugin::new(jackdaw_window_chrome_theme()));
        app.add_plugins(WindowIconPlugin::new(window_icon_png_bytes()));
        app.add_plugins(repo_link::RepoLinkPlugin);
        app.add_observer(tag_chrome_entity_as_editor);
    }
}

/// Stamps `EditorEntity` onto every chrome entity so jackdaw's cleanup, hierarchy, and viewport
/// systems treat the window chrome as editor UI.
fn tag_chrome_entity_as_editor(add: On<Add, WindowChromeEntity>, mut commands: Commands) {
    commands.entity(add.event_target()).insert(EditorEntity);
}

/// Spawns the jackdaw window shell, returning title bar and body content slots.
pub fn spawn_window_shell<S: Component + Copy>(
    commands: &mut Commands,
    icon_font: &IconFont,
    #[cfg(target_os = "windows")] caption_font: &CaptionFont,
    screen: S,
) -> WindowShellSlots {
    #[cfg(target_os = "windows")]
    let _ = icon_font;
    let theme = jackdaw_window_chrome_theme();
    let caption_font = {
        #[cfg(target_os = "windows")]
        {
            caption_font.0.clone()
        }
        #[cfg(not(target_os = "windows"))]
        {
            icon_font.0.clone()
        }
    };
    return bevy_window_chrome::spawn_window_shell(commands, &theme, caption_font, screen);
}
