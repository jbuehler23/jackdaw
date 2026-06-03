//! Jackdaw window chrome: wires the reusable [`bevy_window_chrome`] crate to jackdaw's design
//! tokens, branding, and editor entity bookkeeping.

#[cfg(not(target_os = "windows"))]
mod controls_interactive;
mod repo_link;

#[cfg(target_os = "windows")]
pub use bevy_window_chrome::WindowsCaptionFont;
pub use bevy_window_chrome::{
    NativeHitTestClient, WindowChromeStyle, WindowHeaderRoot, WindowShellContent,
    WindowShellHeaderSlot, WindowShellSlots, native_hit_test_client,
};
pub use repo_link::{JackdawIcon, header_repo_link};

use bevy::prelude::*;
use bevy_window_chrome::{
    CaptionTheme, WindowChromeEntity, WindowChromePlugin, WindowChromeTheme, WindowIconPlugin,
};
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

fn is_pride_month() -> bool {
    let Ok(date_time) = OffsetDateTime::now_local() else {
        return false;
    };
    return date_time.month() == Month::June;
}

/// Resolves jackdaw's window chrome style, honoring `JACKDAW_WINDOW_DECORATIONS`.
pub fn jackdaw_window_chrome_style() -> WindowChromeStyle {
    let decorations = std::env::var("JACKDAW_WINDOW_DECORATIONS").ok();
    return WindowChromeStyle::resolve(decorations.as_deref());
}

/// Primary-window attributes for jackdaw's current platform chrome strategy.
pub fn primary_window_attributes() -> Window {
    return bevy_window_chrome::primary_window_attributes(jackdaw_window_chrome_style());
}

/// Window chrome theme built from jackdaw's design tokens.
fn jackdaw_window_chrome_theme() -> WindowChromeTheme {
    return WindowChromeTheme {
        header_height: tokens::WINDOW_HEADER_HEIGHT,
        window_background: tokens::WINDOW_BG,
        shell_corner_radius: 8.0,
        macos_traffic_light_inset: tokens::MACOS_TRAFFIC_LIGHT_INSET,
        macos_traffic_light_position_x: tokens::MACOS_TRAFFIC_LIGHT_POSITION_X,
        caption: CaptionTheme {
            foreground: tokens::TEXT_PRIMARY,
            button_hover_background: tokens::TOOLBAR_BUTTON_BG,
            ..CaptionTheme::default()
        },
    };
}

pub struct WindowingPlugin;

impl Plugin for WindowingPlugin {
    fn build(&self, app: &mut App) {
        let style = jackdaw_window_chrome_style();
        app.add_plugins(WindowChromePlugin::new(
            jackdaw_window_chrome_theme(),
            style,
        ));
        app.add_plugins(WindowIconPlugin::new(window_icon_png_bytes()));
        app.add_plugins(repo_link::RepoLinkPlugin);
        app.add_observer(tag_chrome_entity_as_editor);
        #[cfg(not(target_os = "windows"))]
        controls_interactive::register(app);
    }
}

/// Stamps `EditorEntity` onto every chrome entity so jackdaw's cleanup, hierarchy, and viewport
/// systems treat the window chrome as editor UI.
fn tag_chrome_entity_as_editor(add: On<Add, WindowChromeEntity>, mut commands: Commands) {
    commands.entity(add.event_target()).insert(EditorEntity);
}

/// Spawns the jackdaw window shell, returning `(header_slot, body_slot)`.
pub fn spawn_window_shell<S: Component + Copy>(
    commands: &mut Commands,
    chrome: WindowChromeStyle,
    icon_font: &IconFont,
    #[cfg(target_os = "windows")] caption_font: &WindowsCaptionFont,
    screen: S,
) -> WindowShellSlots {
    let theme = jackdaw_window_chrome_theme();
    #[cfg(target_os = "windows")]
    let caption_controls = {
        let _ = icon_font;
        bevy_window_chrome::window_controls_native(&theme, caption_font.0.clone())
    };
    #[cfg(not(target_os = "windows"))]
    let caption_controls = controls_interactive::window_controls_interactive(icon_font.0.clone());
    return bevy_window_chrome::spawn_window_shell(
        commands,
        chrome,
        &theme,
        caption_controls,
        screen,
    );
}

/// Tag menu bar items so they stay in the client area under Win32 `WM_NCHITTEST` (Windows only).
#[cfg(target_os = "windows")]
pub fn mark_menu_bar_native_clients(world: &mut World) {
    let menu_item_entities: Vec<Entity> = world
        .query_filtered::<Entity, With<jackdaw_widgets::menu_bar::MenuBarItem>>()
        .iter(world)
        .collect();
    for entity in menu_item_entities {
        if let Ok(mut entity_commands) = world.get_entity_mut(entity) {
            entity_commands.insert(NativeHitTestClient);
        }
    }
}
