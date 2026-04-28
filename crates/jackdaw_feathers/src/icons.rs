use bevy::{asset::AssetId, prelude::*};
pub use lucide_icons::Icon;

/// Resource holding the loaded Lucide icon font handle.
#[derive(Resource, Deref, DerefMut)]
pub struct IconFont(pub Handle<Font>);

/// Resource holding the loaded editor body font (Fira Sans).
#[derive(Resource)]
pub struct EditorFont(pub Handle<Font>);

/// Italic variant of the editor body font. Used by surfaces that
/// want to mark content as "transient" or "runtime"; today the
/// hierarchy italicises rows for entities spawned during PIE Play.
#[derive(Resource)]
pub struct EditorFontItalic(pub Handle<Font>);

pub struct IconFontPlugin;

const FIRA_SANS_BYTES: &[u8] = include_bytes!("../fonts/FiraSans-Regular.ttf");
const FIRA_SANS_ITALIC_BYTES: &[u8] = include_bytes!("../fonts/FiraSans-Italic.ttf");

impl Plugin for IconFontPlugin {
    fn build(&self, app: &mut App) {
        // Insert font resources immediately so they're available before any schedule runs.
        // Both fonts are embedded bytes, so no async loading is needed.
        let mut fonts = app.world_mut().resource_mut::<Assets<Font>>();

        let icon_font = Font::try_from_bytes(lucide_icons::LUCIDE_FONT_BYTES.to_vec())
            .expect("Failed to load Lucide icon font");
        let icon_handle = fonts.add(icon_font);

        let editor_font =
            Font::try_from_bytes(FIRA_SANS_BYTES.to_vec()).expect("Failed to load FiraSans font");
        let editor_font_handle = fonts.add(editor_font.clone());

        let editor_font_italic = Font::try_from_bytes(FIRA_SANS_ITALIC_BYTES.to_vec())
            .expect("Failed to load FiraSans Italic font");
        let editor_font_italic_handle = fonts.add(editor_font_italic);

        // Also override Bevy's default font (AssetId::default()) so that ALL Text nodes
        // that don't specify an explicit font handle use FiraSans instead of FiraMono.
        // This ensures ThemedText and any other Text without `font:` use our editor font.
        let _ = fonts.insert(AssetId::default(), editor_font);

        app.insert_resource(IconFont(icon_handle));
        app.insert_resource(EditorFont(editor_font_handle));
        app.insert_resource(EditorFontItalic(editor_font_italic_handle));
    }
}

/// Create a text bundle that renders a single Lucide icon glyph.
pub fn icon(icon: Icon, size: f32, font: Handle<Font>) -> impl Bundle {
    (
        Text::new(String::from(icon.unicode())),
        TextFont {
            font,
            font_size: size,
            ..Default::default()
        },
    )
}

/// Create a text bundle for an icon with a specific color.
pub fn icon_colored(icon: Icon, size: f32, font: Handle<Font>, color: Color) -> impl Bundle {
    (
        Text::new(String::from(icon.unicode())),
        TextFont {
            font,
            font_size: size,
            ..Default::default()
        },
        TextColor(color),
    )
}
