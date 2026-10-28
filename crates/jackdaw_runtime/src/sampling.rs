//! The project's 2D sampling, read by the game at startup from a file in its
//! asset folder, so it holds under Play, `cargo run` and a shipped build.

use std::path::{Path, PathBuf};

use bevy::asset::AssetPlugin;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImagePlugin, ImageSamplerDescriptor};
use serde::{Deserialize, Serialize};

/// The project settings file the game reads, inside the default asset folder.
pub const GAME_SETTINGS_FILE: &str = "jackdaw_game.json";

/// The key the 2D sampling sits under in [`GAME_SETTINGS_FILE`].
pub const SAMPLING_KEY: &str = "sampling_2d";

/// How images are filtered when drawn larger or smaller than their pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sampling {
    /// Smooth: blends neighbouring pixels.
    #[default]
    Linear,
    /// Crisp: every pixel a hard-edged block, for pixel art.
    Nearest,
}

impl Sampling {
    /// The mode `id` names, `linear` or `nearest`.
    pub fn parse(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "linear" => Some(Self::Linear),
            "nearest" => Some(Self::Nearest),
            _ => None,
        }
    }

    /// How the settings file and an operator name this mode.
    pub fn id(self) -> &'static str {
        match self {
            Self::Linear => "linear",
            Self::Nearest => "nearest",
        }
    }

    /// The filter this mode samples with.
    pub fn filter(self) -> ImageFilterMode {
        match self {
            Self::Linear => ImageFilterMode::Linear,
            Self::Nearest => ImageFilterMode::Nearest,
        }
    }

    /// The project's 2D sampling as the settings `file` names it, linear when
    /// the file or the key is missing or unreadable.
    pub fn read(file: &Path) -> Self {
        std::fs::read(file)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|settings| settings.get(SAMPLING_KEY).cloned())
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    /// `self` with a repeating sampler, as tileable materials need.
    pub fn descriptor(self) -> ImageSamplerDescriptor {
        let mut descriptor = ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            address_mode_w: ImageAddressMode::Repeat,
            ..ImageSamplerDescriptor::linear()
        };
        descriptor.set_filter(self.filter());
        descriptor
    }
}

/// The game settings file of the game rooted at `root`: in the default asset
/// folder, `assets`, whatever folder the game's `AssetPlugin` names, because
/// the game reads it before its app, and so its `AssetPlugin`, exists. The
/// editor writes it here too, under the project root.
pub fn game_settings_file(root: &Path) -> PathBuf {
    root.join(AssetPlugin::default().file_path)
        .join(GAME_SETTINGS_FILE)
}

/// The game's default sampler: repeating, filtered as the project's 2D
/// sampling says, for every image the game loads. Linear for a project that
/// never set it.
pub fn default_image_sampler() -> ImageSamplerDescriptor {
    let root = crate::asset_base(|key| std::env::var(key).ok(), std::env::current_exe().ok());
    let sampling = root.map_or(Sampling::Linear, |root| {
        Sampling::read(&game_settings_file(&root))
    });
    sampling.descriptor()
}

/// Bevy's `ImagePlugin` with [`default_image_sampler`]: what a game's
/// `main.rs` sets on `DefaultPlugins` for the project's 2D sampling to apply.
pub fn image_plugin() -> ImagePlugin {
    ImagePlugin {
        default_sampler: default_image_sampler(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asset_base;

    fn env<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |key| {
            pairs
                .iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn the_asset_base_follows_play_then_cargo_then_the_executable() {
        let exe = Some(PathBuf::from("/games/farm/farm"));
        let play = [("BEVY_ASSET_ROOT", "/play"), ("CARGO_MANIFEST_DIR", "/src")];
        assert_eq!(asset_base(env(&play), exe.clone()), Some("/play".into()));
        let cargo = [("CARGO_MANIFEST_DIR", "/src")];
        assert_eq!(asset_base(env(&cargo), exe.clone()), Some("/src".into()));
        assert_eq!(asset_base(env(&[]), exe), Some("/games/farm".into()));
    }

    #[test]
    fn the_setting_is_read_from_the_asset_folder_and_defaults_to_linear() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = game_settings_file(dir.path());
        assert_eq!(file, dir.path().join("assets").join(GAME_SETTINGS_FILE));
        assert_eq!(Sampling::read(&file), Sampling::Linear, "no file");

        std::fs::create_dir_all(dir.path().join("assets")).expect("assets");
        std::fs::write(&file, r#"{ "sampling_2d": "nearest", "other": 1 }"#).expect("write");
        assert_eq!(Sampling::read(&file), Sampling::Nearest);

        std::fs::write(&file, "not json").expect("write");
        assert_eq!(Sampling::read(&file), Sampling::Linear, "unreadable");

        assert_eq!(
            Sampling::Nearest.descriptor().min_filter,
            ImageFilterMode::Nearest
        );
        assert_eq!(
            Sampling::Linear.descriptor().address_mode_u,
            ImageAddressMode::Repeat
        );
    }
}
