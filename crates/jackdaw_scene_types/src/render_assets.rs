//! Load settings for the models and textures a scene draws with.
//!
//! The asset server keys a load by path and keeps whatever settings the first
//! load of that path asked for, so every place that loads a drawn texture or
//! model goes through these, and they all agree.

use bevy::asset::RenderAssetUsages;
use bevy::gltf::GltfLoaderSettings;
use bevy::image::ImageLoaderSettings;

/// Where a drawn texture's pixels live: on the GPU only.
pub const DRAWN_TEXTURE_USAGE: RenderAssetUsages = RenderAssetUsages::RENDER_WORLD;

/// Material texture slots that hold linear data rather than colour.
pub const LINEAR_TEXTURE_SLOTS: [&str; 9] = [
    "normal_map_texture",
    "foam_mask",
    "metallic_roughness_texture",
    "occlusion_texture",
    "depth_map",
    "layer_normal_map_texture",
    "layer_orm_texture",
    "detail_normal_map_texture",
    "detail_orm_texture",
];

/// Material texture slots that hold colour.
pub const COLOUR_TEXTURE_SLOTS: [&str; 4] = [
    "base_color_texture",
    "emissive_texture",
    "layer_base_color_texture",
    "detail_base_color_texture",
];

/// Settings for a model: its textures are drawn, not read back.
pub fn model_settings(settings: &mut GltfLoaderSettings) {
    settings.load_materials = DRAWN_TEXTURE_USAGE;
}

/// Settings for a texture a material draws with. A linear slot is read without
/// sRGB decoding; any other keeps the colour space its file asks for.
pub fn drawn_texture_settings(
    linear: bool,
) -> impl Fn(&mut ImageLoaderSettings) + Send + Sync + 'static {
    move |settings| {
        if linear {
            settings.is_srgb = false;
        }
        settings.asset_usage = DRAWN_TEXTURE_USAGE;
    }
}

/// Whether the material field `slot` binds a texture, and if so whether it
/// holds linear data.
pub fn texture_slot(slot: &str) -> Option<bool> {
    if LINEAR_TEXTURE_SLOTS.contains(&slot) {
        Some(true)
    } else if COLOUR_TEXTURE_SLOTS.contains(&slot) {
        Some(false)
    } else {
        None
    }
}

/// Whether a slot's value names a file rather than a catalog or scene-local
/// reference.
pub fn is_file_reference(value: &str) -> bool {
    !value.is_empty() && !value.starts_with('@') && !value.starts_with('#')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_say_whether_they_hold_linear_data() {
        assert_eq!(texture_slot("normal_map_texture"), Some(true));
        assert_eq!(texture_slot("foam_mask"), Some(true));
        assert_eq!(texture_slot("base_color_texture"), Some(false));
        assert_eq!(texture_slot("icon"), None);
    }

    #[test]
    fn references_are_not_files() {
        assert!(is_file_reference("textures/rock_a.png"));
        assert!(!is_file_reference("@rock_a"));
        assert!(!is_file_reference("#Image0"));
        assert!(!is_file_reference(""));
    }

    #[test]
    fn drawn_textures_keep_no_cpu_copy() {
        let mut settings = ImageLoaderSettings::default();
        drawn_texture_settings(true)(&mut settings);
        assert!(!settings.is_srgb);
        assert_eq!(settings.asset_usage, RenderAssetUsages::RENDER_WORLD);

        let mut settings = ImageLoaderSettings {
            is_srgb: false,
            ..Default::default()
        };
        drawn_texture_settings(false)(&mut settings);
        assert!(
            !settings.is_srgb,
            "a colour slot keeps the colour space its meta asks for"
        );

        let mut model = GltfLoaderSettings::default();
        model_settings(&mut model);
        assert_eq!(model.load_materials, RenderAssetUsages::RENDER_WORLD);
        assert_eq!(model.load_meshes, RenderAssetUsages::default());
    }
}
