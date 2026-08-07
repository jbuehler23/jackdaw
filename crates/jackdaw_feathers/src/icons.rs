use bevy::asset::embedded_asset;
use bevy::text::{FontSize, FontSourceTemplate};
use bevy::{asset::AssetId, prelude::*};
pub use lucide_icons::Icon;

/// Embedded-asset paths for the editor fonts, referenceable from `bsn!`
/// scenes and anywhere an `AssetServer` load path is expected.
///
/// A font is registered with [`embedded_asset!`] and then named by its
/// `embedded://` path, which the `bsn!` template machinery resolves to a
/// `Handle<Font>` via `AssetServer::load`.
///
/// The `..` segment is expected. The font files live in `fonts/` beside
/// `src/`, so `embedded_asset!`, called from `src/icons.rs`, computes the
/// path relative to the source file. The registration key and the load
/// key are byte-identical, so the `..` round-trips correctly. The
/// `embedded_font_paths_match_registration` test pins these to the value
/// `embedded_asset!` registers.
pub mod font_paths {
    /// Lucide icon font (glyphs via [`super::Icon`]`::*.unicode()`).
    pub const LUCIDE: &str = "embedded://jackdaw_feathers/../fonts/lucide.ttf";
    /// Editor body font, regular weight.
    pub const FIRA_REGULAR: &str = "embedded://jackdaw_feathers/../fonts/FiraSans-Regular.ttf";
    /// Editor body font, italic.
    pub const FIRA_ITALIC: &str = "embedded://jackdaw_feathers/../fonts/FiraSans-Italic.ttf";
}

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

        // Register the fonts as embedded assets so they can be referenced by
        // path from `bsn!` scenes; see `font_paths`. The synchronous
        // from-bytes load above still backs the handle resources. This
        // registration only adds the `embedded://` path source.
        embedded_asset!(app, "../fonts/lucide.ttf");
        embedded_asset!(app, "../fonts/FiraSans-Regular.ttf");
        embedded_asset!(app, "../fonts/FiraSans-Italic.ttf");
    }
}

/// Create a text bundle that renders a single Lucide icon glyph.
pub fn icon(icon: Icon, size: f32, font: Handle<Font>) -> impl Bundle {
    (
        Text::new(String::from(icon.unicode())),
        TextFont {
            font: font.into(),
            font_size: FontSize::Px(size),
            ..Default::default()
        },
    )
}

/// Create a text bundle for an icon with a specific color.
pub fn icon_colored(icon: Icon, size: f32, font: Handle<Font>, color: Color) -> impl Bundle {
    (
        Text::new(String::from(icon.unicode())),
        TextFont {
            font: font.into(),
            font_size: FontSize::Px(size),
            ..Default::default()
        },
        TextColor(color),
    )
}

/// A `bsn!` scene rendering a single Lucide icon glyph, for composing icon
/// content inside other scenes without threading a `Handle<Font>`.
///
/// Pass a glyph string, e.g. `Icon::Plus.unicode()`. The font is referenced
/// by its embedded path [`font_paths::LUCIDE`]; `bsn!` resolves it to a
/// `Handle<Font>` at scene-build time.
pub fn icon_scene(glyph: impl Into<String>, size: f32) -> impl Scene {
    let glyph = glyph.into();
    bsn! {
        Text(glyph)
        TextFont {
            font: FontSourceTemplate::Handle(font_paths::LUCIDE),
            font_size: FontSize::Px(size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::embedded_path;

    /// The `font_paths` constants must be the keys that `embedded_asset!`
    /// registers, otherwise `AssetServer::load` and the `bsn!` coercion would
    /// miss. `embedded_path!` runs the same path computation the registration
    /// uses, from the same source file, so this verifies and documents the
    /// embedded prefix.
    #[test]
    fn embedded_font_paths_match_registration() {
        let uri_path = |uri: &str| {
            std::path::PathBuf::from(
                uri.strip_prefix("embedded://")
                    .expect("font constant uses the embedded scheme"),
            )
        };
        assert_eq!(
            embedded_path!("../fonts/lucide.ttf"),
            uri_path(font_paths::LUCIDE),
        );
        assert_eq!(
            embedded_path!("../fonts/FiraSans-Regular.ttf"),
            uri_path(font_paths::FIRA_REGULAR),
        );
        assert_eq!(
            embedded_path!("../fonts/FiraSans-Italic.ttf"),
            uri_path(font_paths::FIRA_ITALIC),
        );
    }

    /// `icon_scene` must construct a valid `bsn!` scene. This is a
    /// compile-level check of the `TextFont` and font-path coercion.
    #[test]
    fn icon_scene_constructs() {
        let _scene = icon_scene(Icon::Plus.unicode(), 16.0);
    }
}
