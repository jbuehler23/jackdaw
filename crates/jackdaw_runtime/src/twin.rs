//! Resolving a document reference through the asset server to whichever of its
//! two forms is on disk.
//!
//! A reference is written as a `.bsn` path and stays that way after an export
//! has rewritten the tree as `.bsb`, so the reader behind an asset source
//! retries the twin when the path as written is not there. The rule is
//! `jackdaw_bsn::existing_form`'s: the path as written first, then the other
//! form.

use std::path::{Path, PathBuf};

use bevy::asset::io::{
    AssetReader, AssetReaderError, AssetSourceBuilder, AssetSourceId, ErasedAssetReader,
    PathStream, Reader, VecReader,
};
use bevy::asset::{AssetApp, AssetMode, AssetPlugin};
use bevy::prelude::*;

/// Present once an asset source has been registered with twin resolution.
#[derive(Resource)]
pub(crate) struct DocumentTwins;

/// Registers the default asset source so a document reference resolves to
/// whichever of its two forms is on disk.
///
/// Add it before `DefaultPlugins`: an asset source is only read while
/// `AssetPlugin` is being built. Its three fields shadow the `AssetPlugin`
/// fields of the same names and must be given the same values, because the
/// source registered here is the one `AssetPlugin` then keeps.
pub struct JackdawAssetSourcePlugin {
    /// The unprocessed asset folder, as `AssetPlugin::file_path` names it.
    pub file_path: String,
    /// The processed asset folder, as `AssetPlugin::processed_file_path` names
    /// it.
    pub processed_file_path: String,
    /// Whether assets are processed, as `AssetPlugin::mode` names it.
    pub mode: AssetMode,
}

impl Default for JackdawAssetSourcePlugin {
    fn default() -> Self {
        let assets = AssetPlugin::default();
        Self {
            file_path: assets.file_path,
            processed_file_path: assets.processed_file_path,
            mode: assets.mode,
        }
    }
}

impl Plugin for JackdawAssetSourcePlugin {
    fn build(&self, app: &mut App) {
        let processed = (!matches!(self.mode, AssetMode::Unprocessed))
            .then_some(self.processed_file_path.as_str());
        let source = AssetSourceBuilder::platform_default(&self.file_path, processed);
        app.register_asset_source(AssetSourceId::Default, with_document_twins(source));
        app.insert_resource(DocumentTwins);
    }
}

/// Wrap one asset source's readers so a document reference resolves to
/// whichever of its two forms is on disk.
///
/// A game registering a source of its own passes the builder through this and
/// keeps the same resolution the default source has.
pub fn with_document_twins(mut source: AssetSourceBuilder) -> AssetSourceBuilder {
    let mut reader = source.reader;
    source.reader = Box::new(move || Box::new(DocumentTwinReader::new(reader())));
    if let Some(mut processed) = source.processed_reader.take() {
        source.processed_reader = Some(Box::new(move || {
            Box::new(DocumentTwinReader::new(processed()))
        }));
    }
    source
}

/// Reads the twin of a document path when the path as written is not there.
pub struct DocumentTwinReader {
    inner: Box<dyn ErasedAssetReader>,
}

impl DocumentTwinReader {
    /// Wrap `inner`, leaving every path that is not a document to it.
    pub fn new(inner: Box<dyn ErasedAssetReader>) -> Self {
        Self { inner }
    }

    async fn read_twin<'a>(
        &'a self,
        path: &'a Path,
        meta: bool,
        missing: AssetReaderError,
    ) -> Result<Box<dyn Reader + 'a>, AssetReaderError> {
        let Some(twin) = twin_path(path) else {
            return Err(missing);
        };
        let mut bytes = Vec::new();
        {
            let read = if meta {
                self.inner.read_meta(&twin).await
            } else {
                self.inner.read(&twin).await
            };
            let Ok(mut reader) = read else {
                return Err(missing);
            };
            reader.read_to_end(&mut bytes).await?;
        }
        Ok(Box::new(VecReader::new(bytes)))
    }
}

impl AssetReader for DocumentTwinReader {
    async fn read<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        match self.inner.read(path).await {
            Ok(reader) => Ok(reader),
            Err(missing @ AssetReaderError::NotFound(_)) => {
                self.read_twin(path, false, missing).await
            }
            Err(err) => Err(err),
        }
    }

    async fn read_meta<'a>(&'a self, path: &'a Path) -> Result<impl Reader + 'a, AssetReaderError> {
        match self.inner.read_meta(path).await {
            Ok(reader) => Ok(reader),
            Err(missing @ AssetReaderError::NotFound(_)) => {
                self.read_twin(path, true, missing).await
            }
            Err(err) => Err(err),
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

/// The other form of a document path, for a path that names a document at all.
fn twin_path(path: &Path) -> Option<PathBuf> {
    if !jackdaw_bsn::is_document_path(path) {
        return None;
    }
    Some(if jackdaw_bsn::is_binary_path(path) {
        jackdaw_bsn::text_twin(path)
    } else {
        jackdaw_bsn::binary_twin(path)
    })
}
