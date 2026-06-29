use bevy::prelude::*;
use bevy::scene::bsn;
use bevy::text::{FontSize, FontSourceTemplate, TextFontTemplate};
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
        let mut fonts = app.world_mut().resource_mut::<Assets<Font>>();

        let icon_font = Font::from_bytes(lucide_icons::LUCIDE_FONT_BYTES.to_vec());
        let icon_handle = fonts.add(icon_font);

        let editor_font = Font::from_bytes(FIRA_SANS_BYTES.to_vec());
        let editor_font_handle = fonts.add(editor_font.clone());

        let editor_font_italic = Font::from_bytes(FIRA_SANS_ITALIC_BYTES.to_vec());
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

pub fn icon_scene(icon: Icon, size: f32, font: Handle<Font>, color: Color) -> impl Scene {
    let glyph = String::from(icon.unicode());
    let text_font = TextFontTemplate {
        font: FontSourceTemplate::Handle(font.into()),
        font_size: FontSize::Px(size),
        ..default()
    };
    bsn! {
        Text::new(glyph)
        TextColor(color)
        template_value(text_font)
    }
}
