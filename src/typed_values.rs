//! Plain text into the value a reflected field holds.
//!
//! Two field shapes have no useful JSON spelling and so cannot be set from a
//! caller that only has strings: a `Handle<T>`, which names an asset by path,
//! and a `Color`, which a person writes as channels, hex or a name. Both are
//! resolved here, against the field's own type, and everything else is left to
//! the JSON paths.
//!
//! A handle field is resolved against the references the project already
//! holds before the asset server is asked, so a path names the file the editor
//! loaded rather than a second copy of it.

use std::any::TypeId;

use bevy::asset::{AssetServer, ReflectHandle, UntypedHandle};
use bevy::color::palettes::css;
use bevy::prelude::*;
use bevy::reflect::{PartialReflect, TypeInfo, TypeRegistry, enums::VariantInfo};
use jackdaw_bsn::{BsnApplyAssets, BsnValue, bsn_value_to_reflect};
use path_slash::PathExt as _;

/// Whether a field of this type names its value by asset path.
pub fn takes_asset_path(registry: &TypeRegistry, type_id: TypeId) -> bool {
    if registry.get_type_data::<ReflectHandle>(type_id).is_some() {
        return true;
    }
    let Some(registration) = registry.get(type_id) else {
        return false;
    };
    let TypeInfo::Enum(info) = registration.type_info() else {
        return false;
    };
    info.type_path().starts_with("core::option::Option<")
        && matches!(info.variant("Some"), Some(VariantInfo::Tuple(variant))
        if variant.field_at(0).is_some_and(|field| {
            registry.get_type_data::<ReflectHandle>(field.type_id()).is_some()
        }))
}

/// The colour a string spells: `r,g,b`, `r,g,b,a`, a hex code, or one of the
/// names below. Channels are sRGB, so they name the same colour a hex code of
/// the same value does.
pub fn parse_color(text: &str) -> Option<Color> {
    let text = text.trim();
    if let Some(color) = named_color(&text.to_ascii_lowercase()) {
        return Some(color);
    }
    if let Ok(srgba) = Srgba::hex(text.trim_start_matches('#')) {
        return Some(Color::Srgba(srgba));
    }
    let channels: Vec<f32> = text
        .split(',')
        .map(|part| part.trim().parse::<f32>())
        .collect::<Result<_, _>>()
        .ok()?;
    match channels[..] {
        [r, g, b] => Some(Color::srgb(r, g, b)),
        [r, g, b, a] => Some(Color::srgba(r, g, b, a)),
        _ => None,
    }
}

fn named_color(name: &str) -> Option<Color> {
    let srgba = match name {
        "black" => css::BLACK,
        "white" => css::WHITE,
        "red" => css::RED,
        "green" => css::GREEN,
        "blue" => css::BLUE,
        "yellow" => css::YELLOW,
        "cyan" => css::AQUA,
        "magenta" => css::FUCHSIA,
        "orange" => css::ORANGE,
        "purple" => css::PURPLE,
        "gray" | "grey" => css::GRAY,
        _ => return None,
    };
    Some(Color::Srgba(srgba))
}

/// The references a field's text is resolved against: what the project holds
/// under that spelling, keyed by path and by the bare name older files use.
pub type References = bevy::platform::collections::HashMap<String, UntypedHandle>;

/// The value `text` stands for in a field of `type_id`, or `None` when the
/// field's type takes its value some other way.
pub fn text_value_for_field(
    registry: &TypeRegistry,
    server: Option<&AssetServer>,
    references: Option<&References>,
    type_id: TypeId,
    text: &str,
) -> Option<Box<dyn PartialReflect>> {
    if takes_asset_path(registry, type_id) {
        let assets = server.map(|server| BsnApplyAssets {
            server,
            local: references,
        });
        return bsn_value_to_reflect(
            &BsnValue::String(text.to_string()),
            type_id,
            registry,
            assets.as_ref(),
        );
    }
    if type_id == TypeId::of::<Color>() {
        return parse_color(text).map(|color| Box::new(color) as Box<dyn PartialReflect>);
    }
    None
}

/// The path a `Handle<T>` or `Option<Handle<T>>` field points at, as the JSON
/// string that sets it again. `None` when the field takes no asset path.
pub fn asset_path_json(
    registry: &TypeRegistry,
    server: Option<&AssetServer>,
    index: Option<&crate::asset_index::AssetIndex>,
    field: &dyn PartialReflect,
) -> Option<serde_json::Value> {
    if !takes_asset_path(registry, field.get_represented_type_info()?.type_id()) {
        return None;
    }
    let handle = handle_of(registry, field);
    let indexed = handle.as_ref().and_then(|handle| {
        index
            .and_then(|index| index.path_of_id(handle.id()))
            .map(|path| path.to_slash_lossy().into_owned())
    });
    let path = indexed
        .or_else(|| {
            handle
                .and_then(|handle| server.and_then(|server| server.get_path(handle.id())))
                .map(|path| path.to_string())
        })
        .unwrap_or_default();
    Some(serde_json::Value::String(path))
}

/// The type path of the asset a field names, for the rows and pickers that
/// offer the files holding it. `None` when the field takes no asset path.
pub fn asset_type_path(registry: &TypeRegistry, type_id: TypeId) -> Option<String> {
    let handle_type = handle_type_id(registry, type_id)?;
    let asset_type = registry
        .get_type_data::<ReflectHandle>(handle_type)?
        .asset_type_id();
    Some(
        registry
            .get(asset_type)?
            .type_info()
            .type_path()
            .to_string(),
    )
}

/// The `Handle<T>` a field's type is, reaching through an `Option` to find it.
fn handle_type_id(registry: &TypeRegistry, type_id: TypeId) -> Option<TypeId> {
    if registry.get_type_data::<ReflectHandle>(type_id).is_some() {
        return Some(type_id);
    }
    if !takes_asset_path(registry, type_id) {
        return None;
    }
    let TypeInfo::Enum(info) = registry.get(type_id)?.type_info() else {
        return None;
    };
    let VariantInfo::Tuple(variant) = info.variant("Some")? else {
        return None;
    };
    Some(variant.field_at(0)?.type_id())
}

/// The handle a field holds, reaching through an `Option` to find it.
fn handle_of(
    registry: &TypeRegistry,
    field: &dyn PartialReflect,
) -> Option<bevy::asset::UntypedHandle> {
    let type_id = field.get_represented_type_info()?.type_id();
    if registry.get_type_data::<ReflectHandle>(type_id).is_some() {
        return handle_from_reflect(registry, field);
    }
    if !takes_asset_path(registry, type_id) {
        return None;
    }
    let bevy::reflect::ReflectRef::Enum(option) = field.reflect_ref() else {
        return None;
    };
    handle_from_reflect(registry, option.field_at(0)?)
}

fn handle_from_reflect(
    registry: &TypeRegistry,
    field: &dyn PartialReflect,
) -> Option<bevy::asset::UntypedHandle> {
    let value = field.try_as_reflect()?;
    registry
        .get_type_data::<ReflectHandle>(value.type_id())?
        .downcast_handle_untyped(value.as_any())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_hex_and_names_all_read_as_colours() {
        assert_eq!(parse_color("0.1,0.2,0.4"), Some(Color::srgb(0.1, 0.2, 0.4)));
        assert_eq!(
            parse_color("0.5, 0.25, 0.125, 0.5"),
            Some(Color::srgba(0.5, 0.25, 0.125, 0.5))
        );
        assert_eq!(parse_color("#1a2b3c"), parse_color("1a2b3c"));
        assert_eq!(parse_color("#ff0000"), Some(Color::Srgba(css::RED)));
        assert_eq!(parse_color("Red"), Some(Color::Srgba(css::RED)));
        assert!(parse_color("not a colour").is_none());
        assert!(parse_color("1,2").is_none());
        assert!(parse_color("").is_none());
    }

    #[test]
    fn channels_and_a_hex_code_of_the_same_colour_agree() {
        let hex = parse_color("#ff8000").expect("a hex colour");
        let channels = parse_color("1,0.501961,0").expect("a channel colour");
        let (hex, channels) = (hex.to_linear(), channels.to_linear());
        assert!((hex.red - channels.red).abs() < 1e-4, "{hex:?}");
        assert!((hex.green - channels.green).abs() < 1e-4, "{hex:?}");
        assert!((hex.blue - channels.blue).abs() < 1e-4, "{hex:?}");
    }
}
