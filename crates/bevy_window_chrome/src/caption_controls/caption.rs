//! Client-side caption buttons (minimize / maximize / close) with Bevy-driven interaction.

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::LineHeight;
use bevy::window::PrimaryWindow;

use crate::{CaptionTheme, WindowChromeEntity, WindowChromeTheme};

const CAPTION_LUCIDE_FONT_BYTES: &[u8] = include_bytes!("../../caption-lucide.ttf");

/// Lucide glyphs use more of the em-box as padding than Segoe Fluent Icons.
const LUCIDE_CAPTION_GLYPH_SCALE: f32 = 1.65;
const LUCIDE_MAXIMIZE_RESTORE_GLYPH_SCALE: f32 = 0.85;

#[cfg(target_os = "windows")]
const SEGOE_FLUENT_ICONS_FILE: &str = "SegoeIcons.ttf";
#[cfg(target_os = "windows")]
const SEGOE_MDL2_ASSETS_FILE: &str = "segmdl2.ttf";

/// Caption icon font and glyph mapping, installed by [`crate::WindowChromePlugin`].
#[derive(Resource, Clone)]
pub struct CaptionFont {
    pub handle: Handle<Font>,
    #[cfg(target_os = "windows")]
    use_segoe_glyphs: bool,
}

/// Identifies each caption button for hover/pressed styling.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Component)]
pub enum CaptionButton {
    Minimize,
    Maximize,
    Close,
}

impl CaptionFont {
    fn glyph(&self, kind: CaptionButton, is_maximized: bool) -> String {
        #[cfg(target_os = "windows")]
        if self.use_segoe_glyphs {
            return kind.segoe_glyph(is_maximized).to_string();
        }
        kind.lucide_glyph(is_maximized).to_string()
    }

    fn glyph_scale(&self, kind: CaptionButton) -> f32 {
        #[cfg(target_os = "windows")]
        if self.use_segoe_glyphs {
            return 1.0;
        }
        match kind {
            CaptionButton::Maximize => {
                LUCIDE_MAXIMIZE_RESTORE_GLYPH_SCALE * LUCIDE_CAPTION_GLYPH_SCALE
            }
            CaptionButton::Close | CaptionButton::Minimize => LUCIDE_CAPTION_GLYPH_SCALE,
        }
    }
}

impl CaptionButton {
    fn lucide_glyph(self, is_maximized: bool) -> &'static str {
        match self {
            Self::Close => "\u{e1b2}",
            Self::Maximize => {
                if is_maximized {
                    "\u{e11b}"
                } else {
                    "\u{e113}"
                }
            }
            Self::Minimize => "\u{e11c}",
        }
    }

    #[cfg(target_os = "windows")]
    fn segoe_glyph(self, is_maximized: bool) -> &'static str {
        match self {
            Self::Close => "\u{e8bb}",
            Self::Maximize => {
                if is_maximized {
                    "\u{e923}"
                } else {
                    "\u{e922}"
                }
            }
            Self::Minimize => "\u{e921}",
        }
    }
}

pub(crate) fn load_caption_font(fonts: &mut Assets<Font>) -> CaptionFont {
    #[cfg(target_os = "windows")]
    {
        if let Some(handle) = load_windows_segoe_font(fonts) {
            return CaptionFont {
                handle,
                use_segoe_glyphs: true,
            };
        }
        bevy::log::warn!(
            "Segoe Fluent Icons and Segoe MDL2 Assets were not found; using embedded Lucide caption icons"
        );
        load_lucide_caption_font(fonts)
    }
    #[cfg(any(target_os = "linux", target_os = "freebsd"))]
    {
        load_lucide_caption_font(fonts)
    }
}

#[cfg(target_os = "windows")]
fn load_windows_segoe_font(fonts: &mut Assets<Font>) -> Option<Handle<Font>> {
    let fonts_directory = std::path::Path::new(r"C:\Windows\Fonts");
    let fluent = fonts_directory.join(SEGOE_FLUENT_ICONS_FILE);
    let mdl2 = fonts_directory.join(SEGOE_MDL2_ASSETS_FILE);

    if fluent.is_file()
        && let Ok(bytes) = std::fs::read(&fluent)
        && let Ok(font) = Font::try_from_bytes(bytes)
    {
        return Some(fonts.add(font));
    }
    if mdl2.is_file()
        && let Ok(bytes) = std::fs::read(&mdl2)
        && let Ok(font) = Font::try_from_bytes(bytes)
    {
        return Some(fonts.add(font));
    }
    None
}

fn load_lucide_caption_font(fonts: &mut Assets<Font>) -> CaptionFont {
    let font = Font::try_from_bytes(CAPTION_LUCIDE_FONT_BYTES.to_vec())
        .expect("embedded Lucide caption font should be valid");
    CaptionFont {
        handle: fonts.add(font),
        #[cfg(target_os = "windows")]
        use_segoe_glyphs: false,
    }
}

/// Visual caption buttons for the window chrome title bar.
pub fn window_controls(theme: &WindowChromeTheme, caption_font: &CaptionFont) -> impl Bundle {
    let button_width = theme.caption.button_width;
    let base_glyph_size = theme.caption.glyph_size;
    let foreground = theme.caption.icon_color;
    (
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
                caption_font,
                button_width,
                base_glyph_size,
                foreground,
                CaptionButton::Minimize,
            ),
            caption_button_bundle(
                caption_font,
                button_width,
                base_glyph_size,
                foreground,
                CaptionButton::Maximize,
            ),
            caption_button_bundle(
                caption_font,
                button_width,
                base_glyph_size,
                foreground,
                CaptionButton::Close,
            ),
        ],
    )
}

fn caption_button_bundle(
    caption_font: &CaptionFont,
    button_width: f32,
    base_glyph_size: f32,
    foreground: Color,
    kind: CaptionButton,
) -> impl Bundle {
    let glyph_size = base_glyph_size * caption_font.glyph_scale(kind);
    (
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
            Text::new(caption_font.glyph(kind, false)),
            TextFont {
                font: caption_font.handle.clone(),
                font_size: glyph_size,
                ..default()
            },
            TextColor(foreground),
            LineHeight::Px(glyph_size),
        )],
    )
}

pub(crate) fn sync_caption_chrome(
    _main_thread: bevy::ecs::system::NonSendMarker,
    theme: Res<WindowChromeTheme>,
    caption_font: Res<CaptionFont>,
    primary_window: Query<Entity, With<PrimaryWindow>>,
    mut buttons: Query<
        (
            &CaptionButton,
            &Interaction,
            &Hovered,
            &mut BackgroundColor,
            &Children,
        ),
        With<CaptionButton>,
    >,
    mut texts: Query<&mut Text>,
    mut text_colors: Query<&mut TextColor>,
) {
    let is_maximized = primary_window
        .single()
        .ok()
        .is_some_and(crate::primary_window_is_maximized);

    for (kind, interaction, hovered, mut background, children) in buttons.iter_mut() {
        let highlighted =
            hovered.0 || matches!(*interaction, Interaction::Hovered | Interaction::Pressed);
        let (background_color, foreground_color) =
            caption_colors(*kind, highlighted, &theme.caption);
        background.0 = background_color;

        let icon_label = caption_font.glyph(*kind, is_maximized);
        for child in children.iter() {
            if let Ok(mut text) = texts.get_mut(child)
                && text.0 != icon_label
            {
                text.0 = icon_label.clone();
            }
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
    match kind {
        CaptionButton::Close => (caption.close_hover_background, Color::WHITE),
        CaptionButton::Minimize | CaptionButton::Maximize => {
            (caption.button_hover_background, caption.icon_color)
        }
    }
}
