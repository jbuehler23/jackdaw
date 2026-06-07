//! Client-side caption buttons (minimize / maximize / close) with Bevy-driven interaction.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::LineHeight;
use bevy::window::PrimaryWindow;

use crate::{CaptionTheme, WindowChromeEntity, WindowChromeTheme};

use super::caption_actions;
use super::{WindowControlsClose, WindowControlsMaximize, WindowControlsMinimize};

#[cfg(target_os = "windows")]
const GLYPH_MINIMIZE: &str = "\u{e921}";
#[cfg(target_os = "windows")]
const GLYPH_MAXIMIZE: &str = "\u{e922}";
#[cfg(target_os = "windows")]
const GLYPH_RESTORE: &str = "\u{e923}";
#[cfg(target_os = "windows")]
const GLYPH_CLOSE: &str = "\u{e8bb}";

#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const GLYPH_MINIMIZE: &str = "\u{e11c}";
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const GLYPH_MAXIMIZE: &str = "\u{e113}";
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const GLYPH_RESTORE: &str = "\u{e11b}";
#[cfg(any(target_os = "linux", target_os = "freebsd"))]
const GLYPH_CLOSE: &str = "\u{e1b2}";

#[cfg(target_os = "windows")]
const SEGOE_FLUENT_ICONS_FILE: &str = "SegoeIcons.ttf";
#[cfg(target_os = "windows")]
const SEGOE_MDL2_ASSETS_FILE: &str = "segmdl2.ttf";

/// Caption icon font for the current platform.
///
/// On Windows this is Segoe Fluent Icons / Segoe MDL2 Assets loaded from the system font
/// directory. On Linux and FreeBSD the host application supplies a Lucide (or compatible) font
/// handle when spawning caption controls.
#[derive(Resource, Clone)]
pub struct CaptionFont(pub Handle<Font>);

/// Identifies each caption button for hover/pressed styling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Component)]
pub enum CaptionButton {
    Minimize,
    Maximize,
    Close,
}

/// Loads the Segoe caption icon font from the system font directory.
#[cfg(target_os = "windows")]
pub fn load_windows_caption_font(fonts: &mut Assets<Font>) -> Option<Handle<Font>> {
    let fonts_directory = std::path::Path::new(r"C:\Windows\Fonts");
    let fluent = fonts_directory.join(SEGOE_FLUENT_ICONS_FILE);
    let mdl2 = fonts_directory.join(SEGOE_MDL2_ASSETS_FILE);

    if fluent.is_file() {
        if let Ok(bytes) = std::fs::read(&fluent) {
            if let Ok(font) = Font::try_from_bytes(bytes) {
                return Some(fonts.add(font));
            }
        }
    }
    if mdl2.is_file() {
        if let Ok(bytes) = std::fs::read(&mdl2) {
            if let Ok(font) = Font::try_from_bytes(bytes) {
                return Some(fonts.add(font));
            }
        }
    }
    return None;
}

pub(crate) fn register(app: &mut App) {
    caption_actions::register_pointer_handlers(app);
    app.add_systems(Last, sync_caption_chrome);
}

/// Visual caption buttons for the window chrome header.
pub fn window_controls(theme: &WindowChromeTheme, caption_font: Handle<Font>) -> impl Bundle {
    let button_width = theme.caption.button_width;
    let glyph_size = theme.caption.glyph_size;
    let foreground = theme.caption.icon_color;
    return (
        WindowChromeEntity,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Stretch,
            flex_shrink: 0.0,
            column_gap: px(0.0),
            ..default()
        },
        Pickable::IGNORE,
        children![
            caption_button_bundle(
                caption_font.clone(),
                button_width,
                glyph_size,
                foreground,
                CaptionButton::Minimize,
                WindowControlsMinimize,
            ),
            caption_button_bundle(
                caption_font.clone(),
                button_width,
                glyph_size,
                foreground,
                CaptionButton::Maximize,
                WindowControlsMaximize,
            ),
            caption_button_bundle(
                caption_font,
                button_width,
                glyph_size,
                foreground,
                CaptionButton::Close,
                WindowControlsClose,
            ),
        ],
    );
}

fn caption_button_bundle(
    caption_font: Handle<Font>,
    button_width: f32,
    glyph_size: f32,
    foreground: Color,
    kind: CaptionButton,
    marker: impl Bundle,
) -> impl Bundle {
    return (
        marker,
        kind,
        WindowChromeEntity,
        Interaction::default(),
        Hovered::default(),
        Node {
            width: px(button_width),
            height: percent(100),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        },
        BackgroundColor(Color::NONE),
        children![(
            Text::new(caption_glyph(kind).to_string()),
            TextFont {
                font: caption_font,
                font_size: glyph_size,
                ..default()
            },
            TextColor(foreground),
            LineHeight::Px(glyph_size),
        )],
    );
}

fn caption_glyph(kind: CaptionButton) -> &'static str {
    return match kind {
        CaptionButton::Minimize => GLYPH_MINIMIZE,
        CaptionButton::Maximize => GLYPH_MAXIMIZE,
        CaptionButton::Close => GLYPH_CLOSE,
    };
}

fn maximize_caption_label(is_maximized: bool) -> String {
    return if is_maximized {
        GLYPH_RESTORE.to_string()
    } else {
        GLYPH_MAXIMIZE.to_string()
    };
}

fn sync_caption_chrome(
    _main_thread: bevy::ecs::system::NonSendMarker,
    theme: Res<WindowChromeTheme>,
    primary_window: Query<Entity, With<PrimaryWindow>>,
    mut buttons: Query<
        (
            &CaptionButton,
            &Interaction,
            &Hovered,
            &mut BackgroundColor,
            &Children,
        ),
        Or<(
            With<WindowControlsMinimize>,
            With<WindowControlsMaximize>,
            With<WindowControlsClose>,
        )>,
    >,
    maximize_buttons: Query<&Children, With<WindowControlsMaximize>>,
    mut texts: Query<&mut Text>,
    mut text_colors: Query<&mut TextColor>,
) {
    let is_maximized = primary_window
        .single()
        .ok()
        .is_some_and(|entity| crate::primary_window_is_maximized(entity));
    let maximize_label = maximize_caption_label(is_maximized);

    for children in maximize_buttons.iter() {
        for child in children.iter() {
            if let Ok(mut text) = texts.get_mut(child) {
                if text.0 != maximize_label {
                    text.0 = maximize_label.clone();
                }
            }
        }
    }

    for (kind, interaction, hovered, mut background, children) in buttons.iter_mut() {
        let highlighted =
            hovered.0 || matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
        let (background_color, foreground_color) =
            caption_colors(*kind, highlighted, &theme.caption);
        background.0 = background_color;

        for child in children.iter() {
            let Ok(mut text_color) = text_colors.get_mut(child) else {
                continue;
            };
            text_color.0 = foreground_color;
        }
    }
}

fn caption_colors(
    kind: CaptionButton,
    highlighted: bool,
    caption: &CaptionTheme,
) -> (Color, Color) {
    if !highlighted {
        return (Color::NONE, caption.icon_color);
    }
    return match kind {
        CaptionButton::Close => (caption.close_hover_background, Color::WHITE),
        CaptionButton::Minimize | CaptionButton::Maximize => {
            (caption.button_hover_background, caption.icon_color)
        }
    };
}
