use bevy::prelude::*;
pub use lucide_icons::Icon;

/// Resource holding the loaded Lucide icon font handle.
#[derive(Resource)]
pub struct IconFont(pub Handle<Font>);

/// Resource holding the loaded editor body font (InterVariable).
#[derive(Resource)]
pub struct EditorFont(pub Handle<Font>);

pub struct IconFontPlugin;

const INTER_VARIABLE_BYTES: &[u8] = include_bytes!("../fonts/InterVariable.ttf");

impl Plugin for IconFontPlugin {
    fn build(&self, app: &mut App) {
        // Insert font resources immediately so they're available before any schedule runs.
        // Both fonts are embedded bytes, so no async loading is needed.
        let mut fonts = app.world_mut().resource_mut::<Assets<Font>>();

        let icon_font = Font::try_from_bytes(lucide_icons::LUCIDE_FONT_BYTES.to_vec())
            .expect("Failed to load Lucide icon font");
        let icon_handle = fonts.add(icon_font);

        let editor_font = Font::try_from_bytes(INTER_VARIABLE_BYTES.to_vec())
            .expect("Failed to load InterVariable font");
        let editor_font_handle = fonts.add(editor_font);

        app.insert_resource(IconFont(icon_handle));
        app.insert_resource(EditorFont(editor_font_handle));
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
