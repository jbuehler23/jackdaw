//! Win32-style caption buttons (Segoe icon font + NC hit-test regions).

use bevy::prelude::*;
use bevy::text::LineHeight;
use bevy::window::PrimaryWindow;
use jackdaw_feathers::tokens;

use crate::EditorEntity;

use super::super::native_hit_test::{NativeCaptionButton, NativeCaptionHover};
use super::{WindowControlsClose, WindowControlsMaximize, WindowControlsMinimize};

const CAPTION_BUTTON_WIDTH: f32 = 36.0;
const CAPTION_GLYPH_SIZE: f32 = 10.0;

const GLYPH_MINIMIZE: &str = "\u{e921}";
const GLYPH_MAXIMIZE: &str = "\u{e922}";
const GLYPH_RESTORE: &str = "\u{e923}";
const GLYPH_CLOSE: &str = "\u{e8bb}";

const CLOSE_HOVER_BACKGROUND: Color = Color::srgb(232.0 / 255.0, 17.0 / 255.0, 32.0 / 255.0);
const CLOSE_ACTIVE_BACKGROUND: Color = Color::srgba(232.0 / 255.0, 17.0 / 255.0, 32.0 / 255.0, 0.8);

const SEGOE_FLUENT_ICONS_FILE: &str = "SegoeIcons.ttf";
const SEGOE_MDL2_ASSETS_FILE: &str = "segmdl2.ttf";

/// Segoe caption icon font loaded from the system font directory.
#[derive(Resource, Clone)]
pub struct WindowsCaptionFont(pub Handle<Font>);

/// Installed from [`super::install_windows_caption_font_in_app`] during plugin build.
pub fn load_windows_caption_font(fonts: &mut Assets<Font>) -> Option<Handle<Font>> {
    for path in windows_caption_font_paths() {
        let bytes = std::fs::read(&path).ok()?;
        let font = Font::try_from_bytes(bytes).ok()?;
        return Some(fonts.add(font));
    }
    return None;
}

/// Prefer Segoe Fluent Icons on Windows 11+, Segoe MDL2 Assets on older releases (same as Zed).
fn windows_caption_font_paths() -> Vec<std::path::PathBuf> {
    let fonts_directory = std::path::Path::new(r"C:\Windows\Fonts");
    let fluent = fonts_directory.join(SEGOE_FLUENT_ICONS_FILE);
    let mdl2 = fonts_directory.join(SEGOE_MDL2_ASSETS_FILE);
    let ordered = if is_windows_11_or_later() {
        [fluent, mdl2]
    } else {
        [mdl2, fluent]
    };
    return ordered.into_iter().filter(|path| path.is_file()).collect();
}

fn is_windows_11_or_later() -> bool {
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

    let mut version = OSVERSIONINFOW {
        dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOW>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    let status = unsafe { RtlGetVersion(&mut version) };
    if status != 0 {
        return true;
    }
    return version.dwBuildNumber >= 22000;
}

/// Visual caption buttons; interaction is handled via Win32 non-client hit testing.
pub fn window_controls_native(caption_font: Handle<Font>) -> impl Bundle {
    (
        EditorEntity,
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Stretch,
            flex_shrink: 0.0,
            column_gap: px(0.0),
            ..default()
        },
        Pickable::IGNORE,
        children![
            caption_minimize_button(caption_font.clone()),
            caption_maximize_button(caption_font.clone()),
            caption_close_button(caption_font),
        ],
    )
}

fn caption_minimize_button(caption_font: Handle<Font>) -> impl Bundle {
    caption_button_bundle(
        caption_font,
        NativeCaptionButton::Minimize,
        WindowControlsMinimize,
    )
}

fn caption_maximize_button(caption_font: Handle<Font>) -> impl Bundle {
    caption_button_bundle(
        caption_font,
        NativeCaptionButton::Maximize,
        WindowControlsMaximize,
    )
}

fn caption_close_button(caption_font: Handle<Font>) -> impl Bundle {
    caption_button_bundle(
        caption_font,
        NativeCaptionButton::Close,
        WindowControlsClose,
    )
}

fn caption_button_bundle(
    caption_font: Handle<Font>,
    kind: NativeCaptionButton,
    marker: impl Bundle,
) -> impl Bundle {
    return (
        marker,
        kind,
        EditorEntity,
        Pickable::IGNORE,
        Node {
            width: px(CAPTION_BUTTON_WIDTH),
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
                font_size: CAPTION_GLYPH_SIZE,
                ..default()
            },
            TextColor(tokens::TEXT_PRIMARY),
            LineHeight::Px(CAPTION_GLYPH_SIZE),
        )],
    );
}

fn caption_glyph(kind: NativeCaptionButton) -> &'static str {
    match kind {
        NativeCaptionButton::Minimize => GLYPH_MINIMIZE,
        NativeCaptionButton::Maximize => GLYPH_MAXIMIZE,
        NativeCaptionButton::Close => GLYPH_CLOSE,
    }
}

fn maximize_caption_label(is_maximized: bool) -> String {
    return if is_maximized {
        GLYPH_RESTORE.to_string()
    } else {
        GLYPH_MAXIMIZE.to_string()
    };
}

pub fn sync_windows_caption_chrome(
    _main_thread: bevy::ecs::system::NonSendMarker,
    hover: Res<NativeCaptionHover>,
    primary_window: Query<Entity, With<PrimaryWindow>>,
    mut buttons: Query<
        (
            &NativeCaptionButton,
            &mut BackgroundColor,
            &Children,
            Option<&WindowControlsMaximize>,
        ),
        Or<(
            With<WindowControlsMinimize>,
            With<WindowControlsMaximize>,
            With<WindowControlsClose>,
        )>,
    >,
    mut texts: Query<&mut Text>,
    mut text_colors: Query<&mut TextColor>,
) {
    let is_maximized = primary_window
        .single()
        .ok()
        .is_some_and(|entity| super::super::primary_window_is_maximized(entity));
    let maximize_label = maximize_caption_label(is_maximized);

    for (kind, mut background, children, is_maximize_control) in buttons.iter_mut() {
        let (background_color, foreground_color) = caption_colors(*kind, &hover);
        background.0 = background_color;

        for child in children.iter() {
            if is_maximize_control.is_some() {
                if let Ok(mut text) = texts.get_mut(child) {
                    if text.0 != maximize_label {
                        text.0 = maximize_label.clone();
                    }
                }
            }
            let Ok(mut text_color) = text_colors.get_mut(child) else {
                continue;
            };
            text_color.0 = foreground_color;
        }
    }
}

fn caption_colors(kind: NativeCaptionButton, hover: &NativeCaptionHover) -> (Color, Color) {
    let highlighted = hover.hovered == Some(kind) || hover.pressed == Some(kind);
    if !highlighted {
        return (Color::NONE, tokens::TEXT_PRIMARY);
    }
    match kind {
        NativeCaptionButton::Close => {
            let background = if hover.pressed == Some(kind) {
                CLOSE_ACTIVE_BACKGROUND
            } else {
                CLOSE_HOVER_BACKGROUND
            };
            (background, Color::WHITE)
        }
        NativeCaptionButton::Minimize | NativeCaptionButton::Maximize => {
            (tokens::TOOLBAR_BUTTON_BG, tokens::TEXT_PRIMARY)
        }
    }
}
