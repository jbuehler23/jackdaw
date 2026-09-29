//! Texture import through Bevy's own processor.
//!
//! A texture whose `.meta` names [`TextureImportProcessor`] is block compressed
//! with mipmaps by Bevy's `CompressedImageSaver`. A game running Bevy's asset
//! processor converts it into its processed folder. Without the processor,
//! Bevy refuses to load a file whose meta asks for processing, so
//! [`TextureImportReader`] runs the same load and save for that one file into
//! a cache folder and serves the result in its place.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bevy::asset::AssetPath;
use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, ErasedAssetReader, PathStream, Reader,
    VecReader,
};
use bevy::asset::meta::{AssetAction, AssetMeta, AssetMetaDyn};
use bevy::asset::processor::{LoadTransformAndSave, LoadTransformAndSaveSettings};
use bevy::asset::saver::{AssetSaver, SavedAsset};
use bevy::asset::transformer::IdentityAssetTransformer;
use bevy::image::{
    CompressedImageFormats, CompressedImageSaver, ImageFormat, ImageFormatSetting, ImageLoader,
    ImageLoaderSettings, ImageType,
};
use bevy::prelude::*;
use bevy::reflect::TypePath;

/// Bevy's processor for an imported texture: load it with [`ImageLoader`] and
/// save it with Bevy's `CompressedImageSaver`.
pub type TextureImportProcessor =
    LoadTransformAndSave<ImageLoader, IdentityAssetTransformer<Image>, CompressedImageSaver>;

/// The `.meta` of an imported texture.
pub type TextureImportMeta = AssetMeta<ImageLoader, TextureImportProcessor>;

/// The meta that has Bevy import a texture, read as sRGB colour or as linear
/// data such as a normal map or a mask.
///
/// A Bevy app loads a texture carrying this meta only through Bevy's asset
/// processor or through [`TextureImportReader`]; any other app refuses it, as
/// Bevy refuses every file whose meta asks for processing.
pub fn texture_import_meta(srgb: bool) -> Vec<u8> {
    let meta = TextureImportMeta::new(AssetAction::Process {
        processor: TextureImportProcessor::type_path().to_string(),
        settings: LoadTransformAndSaveSettings {
            loader_settings: ImageLoaderSettings {
                is_srgb: srgb,
                asset_usage: jackdaw_scene_types::render_assets::DRAWN_TEXTURE_USAGE,
                ..default()
            },
            transformer_settings: (),
            saver_settings: (),
        },
    });
    AssetMetaDyn::serialize(&meta)
}

/// The loader settings a texture import meta holds, when the meta is one.
pub fn read_texture_import_meta(bytes: &[u8]) -> Option<ImageLoaderSettings> {
    let meta = TextureImportMeta::deserialize(bytes).ok()?;
    let AssetAction::Process {
        processor,
        settings,
    } = meta.asset
    else {
        return None;
    };
    let ours = processor == TextureImportProcessor::type_path()
        || processor == TextureImportProcessor::short_type_path();
    ours.then_some(settings.loader_settings)
}

/// Wrap an asset source's reader so a texture with an import `.meta` loads as
/// its processed output, processing it into `cache` the first time.
pub fn with_texture_imports(mut source: AssetSourceBuilder, cache: PathBuf) -> AssetSourceBuilder {
    let mut reader = source.reader;
    source.reader = Box::new(move || Box::new(TextureImportReader::new(reader(), cache.clone())));
    source
}

/// Serves a texture with an import `.meta` as Bevy's processor would write it,
/// and every other file as its inner reader has it.
pub struct TextureImportReader {
    inner: Box<dyn ErasedAssetReader>,
    cache: PathBuf,
    processed: Arc<Mutex<HashMap<PathBuf, PathBuf>>>,
}

impl TextureImportReader {
    /// Wrap `inner`, keeping processed textures under `cache`.
    pub fn new(inner: Box<dyn ErasedAssetReader>, cache: PathBuf) -> Self {
        Self {
            inner,
            cache,
            processed: Arc::default(),
        }
    }

    async fn read_all(&self, path: &Path, meta: bool) -> Result<Vec<u8>, AssetReaderError> {
        let mut bytes = Vec::new();
        let mut reader = if meta {
            self.inner.read_meta(path).await?
        } else {
            self.inner.read(path).await?
        };
        reader.read_to_end(&mut bytes).await?;
        Ok(bytes)
    }

    /// The processed file for `path` and the settings to load it with, from
    /// the cache when it holds one for these bytes and this meta.
    async fn process(
        &self,
        path: &Path,
        meta: &[u8],
        loader: &ImageLoaderSettings,
    ) -> Result<(PathBuf, ImageLoaderSettings), AssetReaderError> {
        let source = self.read_all(path, false).await?;
        let mut hasher = DefaultHasher::new();
        source.hash(&mut hasher);
        meta.hash(&mut hasher);
        let file = self.cache.join(format!(
            "{}.{:016x}.basis",
            path.to_string_lossy(),
            hasher.finish()
        ));
        let settings = processed_loader_settings(loader);
        if file.is_file() {
            return Ok((file, settings));
        }
        let extension = path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default();
        let image = Image::from_buffer(
            &source,
            ImageType::Extension(extension),
            CompressedImageFormats::NONE,
            loader.is_srgb,
            loader.sampler.clone(),
            loader.asset_usage,
        )
        .map_err(|err| std::io::Error::other(err.to_string()))?;
        let mut processed = bevy::tasks::futures_lite::io::Cursor::new(Vec::new());
        CompressedImageSaver
            .save(
                &mut processed,
                SavedAsset::from_asset(&image),
                &(),
                AssetPath::from_path(path),
            )
            .await
            .map_err(|err| std::io::Error::other(err.to_string()))?;
        if let Some(folder) = file.parent() {
            std::fs::create_dir_all(folder)?;
        }
        let partial = file.with_extension("basis.partial");
        std::fs::write(&partial, processed.into_inner())?;
        std::fs::rename(&partial, &file)?;
        Ok((file, settings))
    }
}

/// The settings a processed texture loads with: the Basis file Bevy's saver
/// wrote, read with the source's colour space, sampler and usage.
fn processed_loader_settings(loader: &ImageLoaderSettings) -> ImageLoaderSettings {
    ImageLoaderSettings {
        format: ImageFormatSetting::Format(ImageFormat::Basis),
        is_srgb: loader.is_srgb,
        sampler: loader.sampler.clone(),
        asset_usage: loader.asset_usage,
        ..default()
    }
}

/// A Load meta for `ImageLoader` with `settings`.
fn load_meta(settings: ImageLoaderSettings) -> Vec<u8> {
    let meta = AssetMeta::<ImageLoader, ()>::new(AssetAction::Load {
        loader: <ImageLoader as TypePath>::type_path().to_string(),
        settings,
    });
    AssetMetaDyn::serialize(&meta)
}

impl AssetReader for TextureImportReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let processed = self
            .processed
            .lock()
            .ok()
            .and_then(|processed| processed.get(path).cloned());
        let bytes = match processed.and_then(|file| std::fs::read(file).ok()) {
            Some(bytes) => bytes,
            None => self.read_all(path, false).await?,
        };
        Ok(VecReader::new(bytes))
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        let meta = self.read_all(path, true).await?;
        let Some(loader) = read_texture_import_meta(&meta) else {
            return Ok(VecReader::new(meta));
        };
        match self.process(path, &meta, &loader).await {
            Ok((file, settings)) => {
                if let Ok(mut processed) = self.processed.lock() {
                    processed.insert(path.to_path_buf(), file);
                }
                Ok(VecReader::new(load_meta(settings)))
            }
            Err(err) => {
                warn!("Texture import of {} failed: {err}", path.display());
                Ok(VecReader::new(load_meta(ImageLoaderSettings {
                    format: ImageFormatSetting::FromExtension,
                    ..loader
                })))
            }
        }
    }

    async fn read_directory<'a>(
        &'a self,
        path: &'a Path,
    ) -> Result<Box<PathStream>, AssetReaderError> {
        self.inner.read_directory(path).await
    }

    async fn is_directory<'a>(&'a self, path: &'a Path) -> Result<bool, AssetReaderError> {
        self.inner.is_directory(path).await
    }
}

/// Registers the texture import processor with Bevy's asset processor, which
/// only a game running in processed mode has.
pub(crate) fn register_texture_import_processor(app: &mut App) {
    use bevy::asset::AssetApp as _;

    app.register_asset_processor(TextureImportProcessor::from(CompressedImageSaver));
}
